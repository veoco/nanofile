pub mod api;
pub mod api_v1;
pub mod api_v21;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod db;
pub mod entity;
pub mod error;
pub mod indexer;
pub mod notification;
pub mod serialization;
pub mod static_assets;
pub mod storage;
pub mod sync;
pub mod ui;
pub mod web;

use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::api_v21::task_manager::TaskManager;
use crate::auth::access_token::AccessTokenManager;
use crate::auth::rate_limit::LoginRateLimiter;
use crate::config::Config;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::storage::DynBlockStorage;
use crate::storage::path_cache::PathCache;

/// Unified application state injected into all axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    pub config: Config,
    /// Block storage backend — default is filesystem-based.
    pub block_store: DynBlockStorage,
    /// Path to the block storage directory (convenience for FileOps).
    pub block_dir: Arc<PathBuf>,
    /// In-memory path → fs_id cache (avoids tree traversal for hot paths).
    pub path_cache: Arc<PathCache>,
    /// Web access token manager for `/upload-api/` and `/update-api/`.
    pub token_manager: Arc<AccessTokenManager>,
    /// In-memory task manager for async copy/move operations.
    pub task_manager: Arc<TaskManager>,
    /// WebSocket notification manager for real-time repo change notifications.
    /// `None` if the notification feature is disabled.
    pub notification_manager: Option<NotificationManager>,
    /// Full-text search indexer. `None` when indexing is disabled in config.
    pub indexer: Option<TextIndexer>,
    /// Login rate limiter (tracks failed attempts per IP).
    pub login_rate_limiter: Arc<LoginRateLimiter>,
    /// Server-wide secret for CSRF token generation.
    pub csrf_secret: Arc<Vec<u8>>,
}

impl AppState {
    pub fn new(db: DatabaseConnection, config: Config) -> Self {
        let block_dir = Arc::new(PathBuf::from(&config.storage.block_dir));
        let block_store = crate::storage::new_block_store(&block_dir);
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
            tokio::spawn(async move {
                mgr.start_event_listener().await;
            });
        }
        // Start the background JWT token expiry checker that sends jwt-expired
        // notifications when subscription tokens expire. Runs hourly.
        if let Some(ref mgr) = notification_manager {
            let mgr = mgr.clone();
            tokio::spawn(async move {
                mgr.start_token_expiry_checker().await;
            });
        }
        let indexer = if config.index.enabled {
            match TextIndexer::new(&config.index.index_dir) {
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

        let login_rate_limiter = Arc::new(LoginRateLimiter::new(
            config.auth.max_login_attempts,
            config.auth.lockout_duration_secs,
        ));

        // Generate a random 32-byte CSRF secret at startup.
        let mut csrf_raw = [0u8; 32];
        rand::Rng::fill(&mut rand::thread_rng(), &mut csrf_raw);
        let csrf_secret = Arc::new(csrf_raw.to_vec());

        Self {
            db: Arc::new(db),
            config,
            block_store,
            block_dir,
            path_cache: Arc::new(PathCache::default()),
            token_manager: Arc::new(AccessTokenManager::new()),
            task_manager: Arc::new(TaskManager::new()),
            notification_manager,
            indexer,
            login_rate_limiter,
            csrf_secret,
        }
    }
}
