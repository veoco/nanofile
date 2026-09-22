//! # server
//!
//! Application layer for nanofile: handlers, services, routes, AppState.
//!
//! Re-exports `base` and `infra` crates so that existing
//! `crate::module` references within the server crate continue to resolve.

#![allow(clippy::too_many_arguments)]

// ── Server crate modules ────────────────────────────────────────────────────
pub mod app;
pub mod body_limit;
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
pub mod serve;
pub mod service;
pub mod settings;
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
use crate::settings::{RuntimeConfig, SettingsLayers, SettingsService};
use infra::config::Config;
use infra::crypto::password_manager::PasswordManager;
use infra::storage::DynBlockStorage;

/// Decode the configured block-encryption master key. A 64-char hex string is
/// treated as 32 raw bytes (AES-256); any other length is used verbatim. The
/// caller already validated the key is present; an invalid hex string here is a
/// configuration error and must not silently produce a weak key.
fn decode_master_key(key: &str) -> Vec<u8> {
    if key.len() == 64 {
        match hex::decode(key) {
            Ok(bytes) => bytes,
            Err(e) => panic!("NANOFILE_STORAGE_ENCRYPTION_KEY is 64 chars but not valid hex: {e}"),
        }
    } else {
        key.as_bytes().to_vec()
    }
}

/// Unified application state injected into all axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    /// The current configuration. Read it through [`AppState::config`], which
    /// returns the snapshot in force for this request; a saved setting replaces
    /// the snapshot, so no reader has to be told about the change.
    pub config: RuntimeConfig,
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
    /// Domain-separated AEAD key for encrypting repository sync tokens at rest.
    pub token_cipher: Arc<infra::crypto::token_encryption::TokenCipher>,
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
    /// Repos that currently have a running reindex task, to prevent
    /// concurrent reindex of the same repo. Keyed by repo_id.
    pub reindex_running: Arc<std::sync::Mutex<HashMap<String, ()>>>,
    /// Per-user TTL cache of the left-panel repo list (web UI).
    pub left_panel_cache: Arc<crate::ui::left_panel_cache::LeftPanelRepoCache>,
    /// Outbound mail: settings, queue and transport. Always present; whether it
    /// can actually send is decided by config and the saved settings.
    pub mail: Arc<crate::service::mail::Mailer>,
    /// The layered settings: the saved rows, the effective value of every
    /// catalog key and where each came from, and the single point every admin
    /// save goes through.
    pub settings: Arc<SettingsService>,
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
    /// Unix timestamp when the task reached a terminal state. None while running.
    pub finished_at: Option<i64>,
    /// User ID of the task creator (for progress query authorization).
    pub creator_id: i32,
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
    /// Build from a configuration that is already fully resolved.
    ///
    /// Used by the test harness and by any caller that has no settings table to
    /// read: the config file is then the only layer.
    pub fn new(db: DatabaseConnection, config: Config, temp_file_manager: TempFileManager) -> Self {
        let layers = SettingsLayers::plain(&config);
        Self::build(db, layers, temp_file_manager)
    }

    /// Build with the real layers: the config file plus the saved settings.
    ///
    /// The saved rows are read here, so the value of every setting is decided in
    /// exactly one place — the same place the admin page reads.
    pub fn new_with_layers(
        db: DatabaseConnection,
        layers: SettingsLayers,
        temp_file_manager: TempFileManager,
    ) -> Self {
        Self::build(db, layers, temp_file_manager)
    }

    fn build(
        db: DatabaseConnection,
        layers: SettingsLayers,
        temp_file_manager: TempFileManager,
    ) -> Self {
        let db = Arc::new(db);
        // The ciphers are derived from `secret_key`, which is a read-only
        // setting: it can only come from the environment or the config file, so
        // the base layer already holds the value the process runs with.
        let secret = layers.base.server.secret_key.clone();
        let token_cipher = Arc::new(
            infra::crypto::token_encryption::TokenCipher::from_master_key(secret.as_bytes()),
        );
        let totp_cipher = Arc::new(infra::crypto::totp_encryption::TotpCipher::from_master_key(
            secret.as_bytes(),
        ));
        let repos = Arc::new(crate::repository::Repositories::new(
            db.clone(),
            token_cipher.clone(),
            totp_cipher,
        ));

        // Every layer is applied inside the service: the environment and the
        // config file as `base`, the saved rows on top. `config` below is the
        // result, and it is what every startup-time capture reads.
        let settings = Arc::new(SettingsService::new(
            repos.settings.clone(),
            token_cipher.clone(),
            layers,
        ));
        let config = settings.startup().clone();
        let runtime = settings.runtime().clone();

        let block_dir = Arc::new(PathBuf::from(&config.storage.block_dir));
        let mut block_store: infra::storage::DynBlockStorage =
            infra::storage::new_block_store(&block_dir);
        // Wrap the raw store in the at-rest encryption decorator when enabled.
        let enc_mode = config.storage.block_encryption_mode();
        if enc_mode != infra::storage::encrypting_block_store::BlockEncryptionMode::Off {
            let master_key = config.storage.encryption_key.as_deref().unwrap_or_else(|| {
                panic!(
                    "Block at-rest encryption is enabled (mode {:?}) but \
                         NANOFILE_STORAGE_ENCRYPTION_KEY(_FILE) is not set",
                    config.storage.block_encryption_mode
                )
            });
            let cipher = infra::crypto::block_encryption::BlockCipher::from_master_key(
                &decode_master_key(master_key),
            );
            block_store = Arc::new(
                infra::storage::encrypting_block_store::EncryptingBlockStore::new(
                    block_store,
                    cipher,
                    enc_mode,
                ),
            );
        }
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

        // Sync tokens stay recoverable (clients re-present them), so they are
        // encrypted at rest with a key domain-separated from the server secret;
        // the cipher itself was built above, before the settings were read.

        // Apply sync-protocol hardening knobs (process-wide).
        crate::fs::core::traversal::configure(
            config.sync.max_tree_depth,
            config.sync.max_tree_visits,
        );
        crate::service::sync::configure_fs_object_verification(&config.sync.verify_fs_objects);
        // Upload-route body limit; the JSON default is applied in main.rs.
        crate::body_limit::configure(config.server.max_upload_size_mb);

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

        // Outbound mail: settings (the layered email values), the outbox and the
        // SMTP transport. The mailer keeps the *handle*, not a snapshot, so a
        // saved SMTP setting is used by the very next delivery.
        let mail = Arc::new(crate::service::mail::Mailer::new(
            repos.clone(),
            token_cipher.clone(),
            runtime.clone(),
        ));
        // The startup diagnostics have to read the settings row, so they run as
        // a task instead of blocking `new` (which is not async).
        {
            let mail = mail.clone();
            tokio::spawn(async move { mail.log_startup_diagnostics() });
        }

        // Register all background tasks (event listener, token expiry, cache
        // cleanup, share/upload link cleanup, gc, index commit, mail delivery).
        // The interval and switch values are read once here; changing them needs
        // a restart, which the settings catalog states for each of them.
        crate::scheduler_setup::register_default_tasks(
            &scheduler,
            &repos,
            &db,
            notification_manager.as_ref(),
            &password_manager,
            &config.gc,
            &block_store,
            indexer.as_ref(),
            &temp_file_manager,
            config.storage.temp_upload_ttl_hours,
            enc_mode,
            block_dir.as_ref(),
            Some(&mail),
        );

        // In Lazy mode, run the one-shot legacy-block conversion once at startup.
        if enc_mode == infra::storage::encrypting_block_store::BlockEncryptionMode::Lazy {
            let scheduler = scheduler.clone();
            tokio::spawn(async move {
                scheduler.trigger_now("block encryption convert").await;
            });
        }

        // Built from the startup snapshot: the cap is captured by the manager,
        // so a change to it is pushed through the settings hook.
        let task_manager = Arc::new(TaskManager::new(config.tasks.max_active_tasks));

        Self {
            repos,
            db,
            config: runtime,
            settings,
            block_store,
            block_dir,
            token_manager: Arc::new(AccessTokenManager::new()),
            task_manager,
            notification_manager,
            indexer,
            auth_limiters,
            csrf_secret,
            token_cipher,
            temp_file_manager,
            shutdown_token,
            password_manager,
            scheduler,
            reindex_tasks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            reindex_running: Arc::new(std::sync::Mutex::new(HashMap::new())),
            left_panel_cache: Arc::new(crate::ui::left_panel_cache::LeftPanelRepoCache::default()),
            mail,
        }
    }

    // ── Service factory methods ─────────────────────────────────────────

    /// Push a saved setting into the long-lived object that captured it at
    /// startup.
    ///
    /// Only the settings the catalog marks [`infra::settings::Apply::LiveWithHook`]
    /// need this: everything else is read from the snapshot on each request, or
    /// is explicitly restart-only.
    pub fn apply_settings_hooks(&self, hooks: &std::collections::BTreeSet<infra::settings::Hook>) {
        use infra::settings::Hook;
        let config = self.config();
        for hook in hooks {
            match hook {
                Hook::RateLimits => self.auth_limiters.apply(&config.auth),
                Hook::TaskManager => self
                    .task_manager
                    .set_max_active_tasks(config.tasks.max_active_tasks),
                Hook::NotificationManager => {
                    if let Some(manager) = &self.notification_manager {
                        manager.set_connection_limits(
                            config.notification.max_connections,
                            config.notification.max_connections_per_ip,
                        );
                    }
                }
                Hook::SyncStatics => {
                    crate::fs::core::traversal::configure(
                        config.sync.max_tree_depth,
                        config.sync.max_tree_visits,
                    );
                    crate::service::sync::configure_fs_object_verification(
                        &config.sync.verify_fs_objects,
                    );
                }
                // The outbox drainer is registered unconditionally and checks
                // the live switch itself, so flipping it needs no push.
                Hook::MailDrain => {}
            }
        }
    }

    /// Keep the running configuration in step with the settings table.
    ///
    /// A change made on another instance (or restored from a backup) reaches
    /// this process without a restart. The environment and the config file are
    /// deliberately *not* re-read: they are process-level inputs, and a running
    /// process quietly getting a different value for one of them would be a
    /// surprise rather than an update.
    pub fn spawn_settings_refresh(self: &Arc<Self>) {
        let interval = self.settings.refresh_interval_secs();
        if interval == 0 {
            return;
        }
        let state = self.clone();
        let shutdown = self.shutdown_token.child_token();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval));
            // A slow read must not make the following ticks fire in a burst.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick completes immediately; skip it so a start-up read
            // is not duplicated right after the service was built from the table.
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        match state.settings.reload().await {
                            Ok(outcome) if !outcome.is_empty() => {
                                tracing::info!(
                                    keys = ?outcome.changed.iter().map(String::as_str).collect::<Vec<_>>(),
                                    "settings changed in the database; applying them now"
                                );
                                state.apply_settings_hooks(&outcome.hooks);
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!("could not re-read the settings: {e}"),
                        }
                    }
                }
            }
        });
    }

    /// The configuration snapshot in force right now.
    ///
    /// One call per request (or per service call) is enough: the returned `Arc`
    /// keeps that view consistent for as long as the caller holds it, so a
    /// setting saved mid-request cannot make one handler see two values.
    pub fn config(&self) -> Arc<Config> {
        self.config.get()
    }

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
            self.block_store.clone(),
            self.config.clone(),
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
            Arc::new(self.config().storage.thumbnail_dir.clone()),
            Arc::new(self.config().storage.temp_dir.clone()),
            Arc::new(self.config().storage.ffmpeg_path.clone()),
        )
    }

    pub fn exif_service(&self) -> crate::service::fs::exif::ExifService {
        crate::service::fs::exif::ExifService::new(self.repos.clone(), self.block_store.clone())
    }

    pub fn avatar_service(&self) -> crate::service::user::AvatarService {
        crate::service::user::AvatarService::new(
            self.repos.clone(),
            Arc::new(self.config().storage.avatar_dir.clone()),
        )
    }

    pub fn login_service(&self) -> crate::service::auth::login::LoginService {
        crate::service::auth::login::LoginService::new(
            self.repos.clone(),
            self.config().auth.password_hash_iterations,
            self.config().auth.api_token_ttl_days,
            self.auth_limiters.login.clone(),
        )
    }

    pub fn sso_service(&self) -> crate::service::auth::sso::SsoService {
        crate::service::auth::sso::SsoService::new(
            self.repos.clone(),
            self.config().auth.api_token_ttl_days,
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
