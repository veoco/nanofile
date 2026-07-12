//! # server
//!
//! Application layer for nanofile: handlers, services, routes, AppState.
//!
//! Re-exports `base` and `infra` crates so that existing
//! `crate::module` references within the server crate continue to resolve.

#![allow(clippy::too_many_arguments)]

// ── Server crate modules ────────────────────────────────────────────────────
pub mod activity;
pub mod admin;
pub mod routes;

pub mod auth;
pub mod domain;
pub mod fs;
pub mod indexer;
pub mod notification;
pub mod repo;
pub mod repository;
pub mod sdoc;
pub mod sharing;
pub mod static_assets;
pub mod sync;
pub mod thumbnail_util;
pub mod ui;
pub mod user;
pub mod web;

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use sha2::Digest;
use tokio_util::sync::CancellationToken;

use crate::auth::access_token::AccessTokenManager;
use crate::fs::task_manager::TaskManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::web::temp_file::TempFileManager;
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
}

impl AppState {
    pub fn new(db: DatabaseConnection, config: Config, temp_file_manager: TempFileManager) -> Self {
        let block_dir = Arc::new(PathBuf::from(&config.storage.block_dir));
        let block_store = infra::storage::new_block_store(&block_dir);
        let shutdown_token = CancellationToken::new();
        let notification_manager =
            if config.notification.enabled && !config.notification.private_key.is_empty() {
                Some(NotificationManager::new())
            } else {
                None
            };
        // Start the background event listener that forwards repo-update events
        // from the global broadcast channel to WebSocket subscribers.
        if let Some(ref mgr) = notification_manager {
            let mgr = mgr.clone();
            let token = shutdown_token.child_token();
            tokio::spawn(async move {
                mgr.start_event_listener(token).await;
            });
        }
        // Start the background JWT token expiry checker that sends jwt-expired
        // notifications when subscription tokens expire. Runs hourly.
        if let Some(ref mgr) = notification_manager {
            let mgr = mgr.clone();
            let token = shutdown_token.child_token();
            tokio::spawn(async move {
                mgr.start_token_expiry_checker(token).await;
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
        // This makes it deterministic across restarts so existing browser
        // sessions (sfcsrftoken cookies) survive a server restart.
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"csrf-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        let csrf_secret = Arc::new(hasher.finalize().to_vec());

        let password_manager = Arc::new(PasswordManager::new());
        // Start background password cache cleanup (evicts expired entries every 5 min).
        {
            let pm = password_manager.clone();
            let token = shutdown_token.child_token();
            tokio::spawn(async move {
                pm.cleanup_expired(token).await;
            });
        }

        let db = Arc::new(db);
        let repos = Arc::new(crate::repository::Repositories::new(db.clone()));

        // Start background expired share link cleanup (runs hourly).
        {
            let repos = repos.clone();
            start_expiry_cleaner(
                db.clone(),
                shutdown_token.child_token(),
                "share link",
                move |_conn, now| {
                    let repos = repos.clone();
                    Box::pin(async move { repos.share_link.delete_expired(now).await })
                },
            );
        }

        // Start background expired upload link cleanup (runs hourly).
        {
            let repos = repos.clone();
            start_expiry_cleaner(
                db.clone(),
                shutdown_token.child_token(),
                "upload link",
                move |_conn, now| {
                    let repos = repos.clone();
                    Box::pin(async move { repos.upload_link.delete_expired(now).await })
                },
            );
        }

        let indexer = if config.index.enabled {
            match TextIndexer::new(&config.index.index_dir, Some(repos.clone())) {
                Ok(idx) => {
                    tracing::info!(
                        "Full-text indexer initialized at {:?}",
                        config.index.index_dir
                    );
                    // Spawn the background committer so uncommitted index
                    // documents are persisted periodically.
                    idx.spawn_background_committer(shutdown_token.child_token());
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
        }
    }

    // ── Service factory methods ─────────────────────────────────────────

    pub fn file_service(&self) -> crate::fs::service::file::FileService {
        crate::fs::service::file::FileService::new(
            self.repos.clone(),
            self.db.clone(),
            self.block_store.clone(),
            self.indexer.clone(),
            self.token_manager.clone(),
            self.config.clone(),
            self.notification_manager.clone(),
        )
    }

    pub fn dir_service(&self) -> crate::fs::service::dir::DirService {
        crate::fs::service::dir::DirService::new(
            self.repos.clone(),
            self.db.clone(),
            self.indexer.clone(),
        )
    }

    pub fn metadata_service(&self) -> crate::fs::service::metadata::MetadataService {
        crate::fs::service::metadata::MetadataService::new(self.repos.clone())
    }

    pub fn fileops_service(&self) -> crate::fs::service::fileops::FileOpsService {
        crate::fs::service::fileops::FileOpsService::new(
            self.db.clone(),
            self.repos.clone(),
            self.block_store.clone(),
            self.indexer.clone(),
        )
    }

    pub fn starred_service(&self) -> crate::fs::service::starred::StarredService {
        crate::fs::service::starred::StarredService::new(self.repos.clone(), self.db.clone())
    }

    pub fn search_service(&self) -> crate::fs::service::search::SearchService {
        crate::fs::service::search::SearchService::new(
            self.repos.clone(),
            self.db.clone(),
            self.indexer.clone(),
        )
    }

    pub fn thumbnail_service(&self) -> crate::fs::service::thumbnail::ThumbnailService {
        crate::fs::service::thumbnail::ThumbnailService::new(
            self.repos.clone(),
            self.db.clone(),
            self.block_store.clone(),
            self.block_dir.clone(),
        )
    }

    pub fn exif_service(&self) -> crate::fs::service::exif::ExifService {
        crate::fs::service::exif::ExifService::new(
            self.db.clone(),
            self.repos.clone(),
            self.block_store.clone(),
        )
    }

    pub fn login_service(&self) -> crate::auth::service::login::LoginService {
        crate::auth::service::login::LoginService::new(
            self.repos.clone(),
            self.config.auth.password_hash_iterations,
            self.config.auth.api_token_ttl_days,
            self.login_rate_limiter.clone(),
        )
    }

    pub fn sso_service(&self) -> crate::auth::service::sso::SsoService {
        crate::auth::service::sso::SsoService::new(self.repos.clone())
    }

    pub fn two_factor_service(&self) -> crate::auth::service::two_factor::TwoFactorService {
        crate::auth::service::two_factor::TwoFactorService::new(
            self.repos.clone(),
            self.config.auth.password_hash_iterations,
            self.disable_2fa_limiter.clone(),
        )
    }

    pub fn admin_user_service(&self) -> crate::admin::service::AdminUserService {
        crate::admin::service::AdminUserService::new(self.repos.clone())
    }

    pub fn admin_service(&self) -> crate::admin::service::AdminService {
        crate::admin::service::AdminService::new(self.db.clone(), self.repos.clone())
    }

    pub fn device_service(&self) -> crate::user::service::DeviceService {
        crate::user::service::DeviceService::new(self.repos.clone())
    }

    pub fn invitation_service(&self) -> crate::user::service::InvitationService {
        crate::user::service::InvitationService::new(self.repos.clone())
    }
}

/// Spawn a background task that periodically deletes expired records.
///
/// Runs on an hourly interval with graceful cancellation support.
fn start_expiry_cleaner<F>(
    db: Arc<DatabaseConnection>,
    token: CancellationToken,
    name: &'static str,
    cleanup: F,
) where
    F: Fn(
            Arc<DatabaseConnection>,
            i64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, base::AppError>> + Send>>
        + Send
        + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::info!("Expired {name} cleaner stopped");
                    break;
                }
                _ = interval.tick() => {
                    let now = chrono::Utc::now().timestamp();
                    match cleanup(db.clone(), now).await {
                        Ok(count) if count > 0 => {
                            tracing::info!("Cleaned up {count} expired {name}(s)");
                        }
                        Err(e) => {
                            tracing::warn!("Failed to clean expired {name}s: {e}");
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}
