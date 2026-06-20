//! # nanofile-server
//!
//! Application layer for nanofile: handlers, services, routes, AppState.
//!
//! Re-exports `nanofile-domain` and `nanofile-infra` crates so that existing
//! `crate::module` references within the server crate continue to resolve.

// ── Domain crate re-exports ─────────────────────────────────────────────────
pub use nanofile_domain::error;
pub use nanofile_domain::sanitize;

// ── Infra crate re-exports ──────────────────────────────────────────────────
pub use nanofile_infra::common;
pub use nanofile_infra::config;
pub use nanofile_infra::crypto;
pub use nanofile_infra::db;
pub use nanofile_infra::entity;
pub use nanofile_infra::activity_log;
pub use nanofile_infra::events;
pub use nanofile_infra::permission;
pub use nanofile_infra::rate_limit;
pub use nanofile_infra::serialization;
pub use nanofile_infra::storage;

// ── Server crate modules ────────────────────────────────────────────────────
pub mod activity;
pub mod admin;
pub mod routes;

pub mod auth;
pub mod fs;
pub mod indexer;
pub mod notification;
pub mod repo;
pub mod repository;
pub mod sdoc;
pub mod sharing;
pub mod static_assets;
pub mod sync;
pub mod ui;
pub mod user;
pub mod web;

use std::path::PathBuf;
use std::sync::Arc;

use rand::Rng;
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::fs::task_manager::TaskManager;
use crate::auth::access_token::AccessTokenManager;
use crate::config::Config;
use crate::crypto::password_manager::PasswordManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::rate_limit::{GenericRateLimiter, LoginRateLimiter};
use crate::storage::DynBlockStorage;

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
    /// Cancellation token for graceful shutdown.
    /// Triggered from main.rs after axum drains in-flight requests.
    pub shutdown_token: CancellationToken,
    /// Password manager for encrypted repo key caching.
    pub password_manager: Arc<PasswordManager>,
}

impl AppState {
    pub fn new(db: DatabaseConnection, config: Config) -> Self {
        let block_dir = Arc::new(PathBuf::from(&config.storage.block_dir));
        let block_store = crate::storage::new_block_store(&block_dir);
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
        let indexer = if config.index.enabled {
            match TextIndexer::new(&config.index.index_dir) {
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

        // Generate a random 32-byte CSRF secret at startup.
        let mut csrf_raw = [0u8; 32];
        rand::rng().fill_bytes(&mut csrf_raw);
        let csrf_secret = Arc::new(csrf_raw.to_vec());

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

        Self {
            repos: Arc::new(crate::repository::Repositories::new(db.clone())),
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
            shutdown_token,
            password_manager,
        }
    }
}
