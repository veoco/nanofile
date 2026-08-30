//! # server
//!
//! Application layer for nanofile: handlers, services, routes, AppState.
//!
//! Re-exports `base` and `infra` crates so that existing
//! `crate::module` references within the server crate continue to resolve.

#![allow(clippy::too_many_arguments)]

// ── Server crate modules ────────────────────────────────────────────────────
pub mod domain;
pub mod filters;
pub mod fs;
pub mod handler;
pub mod i18n;
pub mod indexer;
pub mod middleware;
pub mod notification;
pub mod repository;
pub mod routes;
pub mod scheduler;
pub mod scheduler_setup;
pub mod service;
pub mod static_assets;
pub mod thumbnail_util;
pub mod ui;
pub mod webdav;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use sha2::Digest;
use tokio_util::sync::CancellationToken;

use crate::fs::task_manager::TaskManager;
use crate::handler::web::temp_file::TempFileManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::scheduler::Scheduler;
use crate::service::auth::access_token::AccessTokenManager;
use crate::service::auth::rate_limit::AuthRateLimiters;
use infra::config::Config;
use infra::crypto::password_manager::PasswordManager;
use infra::storage::DynBlockStorage;

/// Unified application state injected into all axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    pub config: Arc<Config>,
    /// Block storage backend — default is filesystem-based.
    pub block_store: DynBlockStorage,
    /// Path to the block storage directory (convenience for FileOps).
    pub block_dir: Arc<PathBuf>,
    /// Web access token manager for `/upload-api/` and `/update-api/`.
    pub token_manager: Arc<AccessTokenManager>,
    /// In-memory task manager for async copy/move operations.
    pub task_manager: Arc<TaskManager>,
    /// WebSocket notification manager for real-time repo change notifications.
    /// `None` if the notification feature is disabled.
    pub notification_manager: Option<NotificationManager>,
    /// Full-text search indexer. `None` when indexing is disabled in config.
    pub indexer: Option<TextIndexer>,
    /// Repository interfaces for data access (wraps SeaORM entity queries).
    pub repos: Arc<crate::repository::Repositories>,
    /// Aggregated authentication rate limiters (login, reset, registration,
    /// TOTP, 2FA-disable).
    pub auth_limiters: Arc<AuthRateLimiters>,
    /// Server-wide secret for CSRF token generation.
    pub csrf_secret: Arc<Vec<u8>>,
    /// Temporary file manager for resumable/chunked uploads.
    pub temp_file_manager: TempFileManager,
    /// Cancellation token for graceful shutdown.
    /// Triggered from main.rs after axum drains in-flight requests.
    pub shutdown_token: CancellationToken,
    /// Password manager for encrypted repo key caching.
    pub password_manager: Arc<PasswordManager>,
    /// Unified scheduler for all periodic and continuous background tasks.
    pub scheduler: Arc<Scheduler>,
    /// In-memory progress of background full-text reindex tasks, keyed by
    /// task_id. Stored on AppState because `admin_service()` builds a new
    /// `AdminService` per request.
    pub reindex_tasks: Arc<std::sync::Mutex<HashMap<String, ReindexProgress>>>,
    /// Per-user TTL cache of the left-panel repo list (web UI).
    pub left_panel_cache: Arc<crate::ui::left_panel_cache::LeftPanelRepoCache>,
}

/// Progress of a background reindex task (`POST /api2/reindex/`).
#[derive(Clone, Default, serde::Serialize)]
pub struct ReindexProgress {
    /// `"running"` | `"completed"` | `"failed"`.
    pub state: String,
    pub repo_id: String,
    pub done_count: u64,
    pub total: u64,
    pub indexed: u64,
    pub skipped: u64,
    pub error: Option<String>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .field("block_dir", &self.block_dir)
            .field("indexer", &self.indexer.is_some())
            .field("notification_manager", &self.notification_manager.is_some())
            .field("db", &"...")
            .field("block_store", &"...")
            .finish_non_exhaustive()
    }
}

impl AppState {
    pub fn new(db: DatabaseConnection, config: Config, temp_file_manager: TempFileManager) -> Self {
        let block_dir = Arc::new(PathBuf::from(&config.storage.block_dir));
        let block_store = infra::storage::new_block_store(&block_dir);
        let shutdown_token = CancellationToken::new();
        let scheduler = Arc::new(Scheduler::new(shutdown_token.child_token()));

        // ── State setup (order independent of scheduler) ────────────────

        let notification_manager =
            if config.notification.enabled && !config.notification.private_key.is_empty() {
                Some(NotificationManager::new(
                    config.notification.max_connections,
                    config.notification.max_connections_per_ip,
                ))
            } else {
                None
            };

        let auth_limiters = AuthRateLimiters::new(&config.auth);

        // Derive the CSRF secret from the server-wide secret_key via SHA-256.
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"csrf-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        let csrf_secret = Arc::new(hasher.finalize().to_vec());

        let password_manager = Arc::new(PasswordManager::new());

        let db = Arc::new(db);
        let repos = Arc::new(crate::repository::Repositories::new(db.clone()));

        // Full-text indexer (its commit task is registered below alongside the
        // other background tasks).
        let indexer = if config.index.enabled {
            match TextIndexer::new(&config.index.index_dir, Some(repos.clone())) {
                Ok(idx) => {
                    tracing::info!(
                        "Full-text indexer initialized at {:?}",
                        config.index.index_dir
                    );
                    Some(idx)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to initialize full-text indexer: {e}. Search will use filename-only mode."
                    );
                    None
                }
            }
        } else {
            None
        };

        // Register all background tasks (event listener, token expiry, cache
        // cleanup, share/upload link cleanup, gc, index commit).
        crate::scheduler_setup::register_default_tasks(
            &scheduler,
            &repos,
            notification_manager.as_ref(),
            &password_manager,
            &config.gc,
            &block_store,
            indexer.as_ref(),
            &temp_file_manager,
            config.storage.temp_upload_ttl_hours,
        );

        // Built before `config` is moved into the Arc below.
        let task_manager = Arc::new(TaskManager::new(config.tasks.max_active_tasks));

        Self {
            repos,
            db,
            config: Arc::new(config),
            block_store,
            block_dir,
            token_manager: Arc::new(AccessTokenManager::new()),
            task_manager,
            notification_manager,
            indexer,
            auth_limiters,
            csrf_secret,
            temp_file_manager,
            shutdown_token,
            password_manager,
            scheduler,
            reindex_tasks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            left_panel_cache: Arc::new(crate::ui::left_panel_cache::LeftPanelRepoCache::default()),
        }
    }

    // ── Service factory methods ─────────────────────────────────────────

    pub fn file_service(&self) -> crate::service::fs::file::FileService {
        crate::service::fs::file::FileService::new(
            self.repos.clone(),
            self.db.clone(),
            self.block_store.clone(),
            self.indexer.clone(),
            self.token_manager.clone(),
            self.config.clone(),
            self.notification_manager.clone(),
        )
    }

    pub fn dir_service(&self) -> crate::service::fs::dir::DirService {
        crate::service::fs::dir::DirService::new(
            self.repos.clone(),
            self.db.clone(),
            self.indexer.clone(),
        )
    }

    pub fn metadata_service(&self) -> crate::service::fs::metadata::MetadataService {
        crate::service::fs::metadata::MetadataService::new(
            self.db.clone(),
            self.repos.clone(),
            self.config.clone(),
        )
    }

    pub fn fileops_service(&self) -> crate::service::fs::fileops::FileOpsService {
        crate::service::fs::fileops::FileOpsService::new(
            self.db.clone(),
            self.repos.clone(),
            self.block_store.clone(),
            self.indexer.clone(),
        )
    }

    pub fn starred_service(&self) -> crate::service::fs::starred::StarredService {
        crate::service::fs::starred::StarredService::new(self.repos.clone())
    }

    pub fn search_service(&self) -> crate::service::fs::search::SearchService {
        crate::service::fs::search::SearchService::new(self.repos.clone(), self.indexer.clone())
    }

    pub fn thumbnail_service(&self) -> crate::service::fs::thumbnail::ThumbnailService {
        crate::service::fs::thumbnail::ThumbnailService::new(
            self.repos.clone(),
            self.block_store.clone(),
            Arc::new(self.config.storage.thumbnail_dir.clone()),
            Arc::new(self.config.storage.temp_dir.clone()),
            Arc::new(self.config.storage.ffmpeg_path.clone()),
        )
    }

    pub fn exif_service(&self) -> crate::service::fs::exif::ExifService {
        crate::service::fs::exif::ExifService::new(self.repos.clone(), self.block_store.clone())
    }

    pub fn avatar_service(&self) -> crate::service::user::AvatarService {
        crate::service::user::AvatarService::new(
            self.repos.clone(),
            Arc::new(self.config.storage.avatar_dir.clone()),
        )
    }

    pub fn login_service(&self) -> crate::service::auth::login::LoginService {
        crate::service::auth::login::LoginService::new(
            self.repos.clone(),
            self.config.auth.password_hash_iterations,
            self.config.auth.api_token_ttl_days,
            self.auth_limiters.login.clone(),
        )
    }

    pub fn sso_service(&self) -> crate::service::auth::sso::SsoService {
        crate::service::auth::sso::SsoService::new(
            self.repos.clone(),
            self.config.auth.api_token_ttl_days,
        )
    }

    pub fn two_factor_service(&self) -> crate::service::auth::two_factor::TwoFactorService {
        crate::service::auth::two_factor::TwoFactorService::new(
            self.repos.clone(),
            self.config.auth.password_hash_iterations,
            self.auth_limiters.disable_2fa.clone(),
        )
    }

    pub fn admin_user_service(&self) -> crate::service::admin::AdminUserService {
        crate::service::admin::AdminUserService::new(self.repos.clone())
    }

    pub fn admin_service(&self) -> crate::service::admin::AdminService {
        crate::service::admin::AdminService::new(self.repos.clone())
    }

    pub fn sync_service(&self) -> crate::service::sync::SyncService {
        crate::service::sync::SyncService::new(
            self.repos.clone(),
            self.db.clone(),
            self.block_store.clone(),
            self.indexer.clone(),
        )
    }
}
