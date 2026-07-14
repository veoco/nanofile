//! # server
//!
//! Application layer for nanofile: handlers, services, routes, AppState.
//!
//! Re-exports `base` and `infra` crates so that existing
//! `crate::module` references within the server crate continue to resolve.

#![allow(clippy::too_many_arguments)]

// ── Server crate modules ────────────────────────────────────────────────────
pub mod domain;
pub mod fs;
pub mod handler;
pub mod indexer;
pub mod middleware;
pub mod notification;
pub mod repository;
pub mod routes;
pub mod scheduler;
pub mod sdoc;
pub mod service;
pub mod static_assets;
pub mod thumbnail_util;
pub mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use sha2::Digest;
use tokio_util::sync::CancellationToken;

use crate::fs::core::gc::GcManager;
use crate::fs::task_manager::TaskManager;
use crate::handler::web::temp_file::TempFileManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::scheduler::{Scheduler, TaskOutput};
use crate::service::auth::access_token::AccessTokenManager;
use infra::config::Config;
use infra::crypto::password_manager::PasswordManager;
use infra::rate_limit::{GenericRateLimiter, LoginRateLimiter};
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
    /// Login rate limiter (tracks failed attempts per IP).
    pub login_rate_limiter: Arc<LoginRateLimiter>,
    /// Password reset rate limiter (per IP).
    pub password_reset_limiter: Arc<GenericRateLimiter>,
    /// User registration rate limiter (per IP).
    pub registration_limiter: Arc<GenericRateLimiter>,
    /// TOTP verification rate limiter (per user+IP).
    pub totp_limiter: Arc<GenericRateLimiter>,
    /// 2FA disable rate limiter (per user, password brute-force prevention).
    pub disable_2fa_limiter: Arc<GenericRateLimiter>,
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
                Some(NotificationManager::new())
            } else {
                None
            };

        // Continuous: event listener (forwards repo-update events to WebSocket subscribers).
        if let Some(ref mgr) = notification_manager {
            let mgr = mgr.clone();
            scheduler.spawn_continuous("event listener", move |token| async move {
                mgr.run_event_listener(token).await;
            });
        }

        // Periodic: JWT token expiry check (hourly).
        if let Some(ref mgr) = notification_manager {
            let mgr = mgr.clone();
            scheduler.spawn_periodic("token expiry check", 3600, move || {
                let mgr = mgr.clone();
                async move {
                    mgr.check_expired_tokens().await;
                    TaskOutput::success("ok", None)
                }
            });
        }

        let login_rate_limiter = Arc::new(LoginRateLimiter::new(
            config.auth.max_login_attempts,
            config.auth.lockout_duration_secs,
        ));

        let password_reset_limiter = Arc::new(GenericRateLimiter::new(
            config.auth.password_reset_max_per_hour.max(1),
            3600,
        ));
        let registration_limiter = Arc::new(GenericRateLimiter::new(
            config.auth.registration_max_per_hour.max(1),
            3600,
        ));
        let totp_limiter = Arc::new(GenericRateLimiter::new(
            config.auth.totp_max_attempts.max(1),
            300,
        ));
        let disable_2fa_limiter = Arc::new(GenericRateLimiter::new(
            config.auth.totp_max_attempts.max(1),
            300,
        ));

        // Derive the CSRF secret from the server-wide secret_key via SHA-256.
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"csrf-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        let csrf_secret = Arc::new(hasher.finalize().to_vec());

        // Periodic: password cache cleanup (every 5 minutes).
        let password_manager = Arc::new(PasswordManager::new());
        {
            let pm = password_manager.clone();
            scheduler.spawn_periodic("password cache cleanup", 300, move || {
                let pm = pm.clone();
                async move {
                    let count = pm.cleanup_expired_once().await;
                    if count > 0 {
                        TaskOutput::success(
                            format!("Evicted {count} expired password cache entries"),
                            Some(count),
                        )
                    } else {
                        TaskOutput::success("no expired entries", None)
                    }
                }
            });
        }

        let db = Arc::new(db);
        let repos = Arc::new(crate::repository::Repositories::new(db.clone()));

        // Periodic: expired share link cleanup (hourly).
        {
            let repos = repos.clone();
            scheduler.spawn_periodic("share link cleanup", 3600, move || {
                let repos = repos.clone();
                async move {
                    let now = chrono::Utc::now().timestamp();
                    match repos.share_link.delete_expired(now).await {
                        Ok(count) if count > 0 => TaskOutput::success(
                            format!("Cleaned up {count} expired share links"),
                            Some(count),
                        ),
                        Ok(_) => TaskOutput::success("no expired share links", None),
                        Err(e) => {
                            TaskOutput::error(format!("Failed to clean expired share links: {e}"))
                        }
                    }
                }
            });
        }

        // Periodic: expired upload link cleanup (hourly).
        {
            let repos = repos.clone();
            scheduler.spawn_periodic("upload link cleanup", 3600, move || {
                let repos = repos.clone();
                async move {
                    let now = chrono::Utc::now().timestamp();
                    match repos.upload_link.delete_expired(now).await {
                        Ok(count) if count > 0 => TaskOutput::success(
                            format!("Cleaned up {count} expired upload links"),
                            Some(count),
                        ),
                        Ok(_) => TaskOutput::success("no expired upload links", None),
                        Err(e) => {
                            TaskOutput::error(format!("Failed to clean expired upload links: {e}"))
                        }
                    }
                }
            });
        }

        // Periodic: garbage collection (configurable interval).
        let gc_config = config.gc.clone();
        if gc_config.enabled {
            let repos_for_gc = repos.clone();
            scheduler.spawn_periodic("gc", gc_config.interval_hours * 3600, move || {
                let repos = repos_for_gc.clone();
                async move {
                    match GcManager::garbage_collect(&repos, gc_config.keep_commits).await {
                        Ok(count) if count > 0 => TaskOutput::success(
                            format!("GC removed {count} unreferenced FS objects"),
                            Some(count),
                        ),
                        Ok(_) => TaskOutput::success("GC completed: nothing to remove", None),
                        Err(e) => TaskOutput::error(format!("GC failed: {e}")),
                    }
                }
            });
        }

        // Periodic: index background committer (every 30 seconds).
        let indexer = if config.index.enabled {
            match TextIndexer::new(&config.index.index_dir, Some(repos.clone())) {
                Ok(idx) => {
                    tracing::info!(
                        "Full-text indexer initialized at {:?}",
                        config.index.index_dir
                    );
                    {
                        let idx = idx.clone();
                        scheduler.spawn_periodic("index commit", 30, move || {
                            let idx = idx.clone();
                            async move {
                                match idx.commit() {
                                    Ok(()) => TaskOutput::success("index committed", None),
                                    Err(e) => TaskOutput::error(format!(
                                        "Background index commit failed: {e}"
                                    )),
                                }
                            }
                        });
                    }
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

        Self {
            repos,
            db,
            config: Arc::new(config),
            block_store,
            block_dir,
            token_manager: Arc::new(AccessTokenManager::new()),
            task_manager: Arc::new(TaskManager::new()),
            notification_manager,
            indexer,
            login_rate_limiter,
            password_reset_limiter,
            registration_limiter,
            totp_limiter,
            disable_2fa_limiter,
            csrf_secret,
            temp_file_manager,
            shutdown_token,
            password_manager,
            scheduler,
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
        crate::service::fs::metadata::MetadataService::new(self.repos.clone())
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
            self.block_dir.clone(),
        )
    }

    pub fn exif_service(&self) -> crate::service::fs::exif::ExifService {
        crate::service::fs::exif::ExifService::new(self.repos.clone(), self.block_store.clone())
    }

    pub fn login_service(&self) -> crate::service::auth::login::LoginService {
        crate::service::auth::login::LoginService::new(
            self.repos.clone(),
            self.config.auth.password_hash_iterations,
            self.config.auth.api_token_ttl_days,
            self.login_rate_limiter.clone(),
        )
    }

    pub fn sso_service(&self) -> crate::service::auth::sso::SsoService {
        crate::service::auth::sso::SsoService::new(self.repos.clone())
    }

    pub fn two_factor_service(&self) -> crate::service::auth::two_factor::TwoFactorService {
        crate::service::auth::two_factor::TwoFactorService::new(
            self.repos.clone(),
            self.config.auth.password_hash_iterations,
            self.disable_2fa_limiter.clone(),
        )
    }

    pub fn admin_user_service(&self) -> crate::service::admin::AdminUserService {
        crate::service::admin::AdminUserService::new(self.repos.clone())
    }

    pub fn admin_service(&self) -> crate::service::admin::AdminService {
        crate::service::admin::AdminService::new(self.repos.clone())
    }

    pub fn device_service(&self) -> crate::service::user::DeviceService {
        crate::service::user::DeviceService::new(self.repos.clone())
    }

    pub fn invitation_service(&self) -> crate::service::user::InvitationService {
        crate::service::user::InvitationService::new(self.repos.clone())
    }

    pub fn sdoc_service(&self) -> crate::service::sdoc::SdocService {
        crate::service::sdoc::SdocService::new(self.repos.clone())
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
