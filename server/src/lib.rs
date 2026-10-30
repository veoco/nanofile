//! # server
//!
//! Application layer for nanofile: handlers, services, routes, AppState.
//! Re-exports `base` and `infra` so `crate::module` paths keep resolving.

#![allow(clippy::too_many_arguments)]

pub mod app;
pub mod body_limit;
pub mod desktop;
pub mod domain;
pub mod filters;
pub mod fs;
pub mod handler;
pub mod i18n;
pub mod indexer;
pub mod middleware;
pub mod notification;
pub mod repository;
pub mod restart;
pub mod routes;
pub mod sandbox;
pub mod serve;
pub mod service;
pub mod settings;
pub mod setup;
pub mod static_assets;
pub mod tasks;
pub mod thumbnail_util;
pub mod ui;
pub mod webdav;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use sha2::Digest;
use tokio_util::sync::CancellationToken;

use crate::handler::web::temp_file::TempFileManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::service::auth::access_token::AccessTokenManager;
use crate::service::auth::pow::PowStore;
use crate::service::auth::rate_limit::AuthRateLimiters;
use crate::settings::{RuntimeConfig, SettingsLayers, SettingsService};
use crate::tasks::TaskSystem;
use infra::config::Config;
use infra::crypto::password_manager::PasswordManager;
use infra::storage::DynBlockStorage;

/// Decode the block-encryption master key: 64 hex chars become 32 raw bytes
/// (AES-256), any other length is used verbatim. Invalid hex panics — never a weak key.
pub fn decode_master_key(key: &str) -> Vec<u8> {
    if key.len() == 64 {
        match hex::decode(key) {
            Ok(bytes) => bytes,
            Err(e) => panic!("NANOFILE_STORAGE_ENCRYPTION_KEY is 64 chars but not valid hex: {e}"),
        }
    } else {
        key.as_bytes().to_vec()
    }
}

/// The media helper the sandbox should run, from the configured setting.
///
/// The configured value is resolved here, once, so the child's command line and
/// the ACL the container needs are both written for the same absolute path. A
/// blank setting resolves to an empty path: that is how the caller says "no helper".
fn configured_helper(ffmpeg_path: &str) -> std::path::PathBuf {
    let configured = ffmpeg_path.trim();
    if configured.is_empty() {
        std::path::PathBuf::new()
    } else {
        crate::sandbox::worker::resolve_helper(configured)
    }
}

/// Unified application state injected into all axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseConnection>,
    /// The current configuration. Read it through [`AppState::config`], which returns
    /// the snapshot in force for this request; a saved setting replaces the snapshot.
    pub config: RuntimeConfig,
    /// Block storage backend — default is filesystem-based.
    pub block_store: DynBlockStorage,
    pub block_dir: Arc<PathBuf>,
    /// Web access token manager for `/upload-api/` and `/update-api/`.
    pub token_manager: Arc<AccessTokenManager>,
    /// WebSocket notification manager. `None` if the notification feature is disabled.
    pub notification_manager: Option<NotificationManager>,
    /// Full-text search indexer. `None` when indexing is disabled in config.
    pub indexer: Option<TextIndexer>,
    pub repos: Arc<crate::repository::Repositories>,
    /// Aggregated authentication rate limiters: login, reset, registration, TOTP, 2FA-disable.
    pub auth_limiters: Arc<AuthRateLimiters>,
    /// One-time proof-of-work challenges for the anonymous web forms. Process-local:
    /// a restart drops them and the visitor reloads the form.
    pub pow: Arc<PowStore>,
    /// Server-wide secret for CSRF token generation.
    pub csrf_secret: Arc<Vec<u8>>,
    /// Domain-separated AEAD key for encrypting repository sync tokens at rest.
    pub token_cipher: Arc<infra::crypto::token_encryption::TokenCipher>,
    pub temp_file_manager: TempFileManager,
    /// Cancellation token for graceful shutdown; triggered from main.rs after axum drains.
    pub shutdown_token: CancellationToken,
    pub password_manager: Arc<PasswordManager>,
    /// The job system: every periodic, manual and submitted background job, plus its
    /// run history.
    ///
    /// Process-lifetime, so a run submitted before an in-place restart is still there.
    pub tasks: Arc<TaskSystem>,
    /// In-memory progress of background full-text reindex tasks, keyed by task_id. Stored here
    /// because `admin_service()` builds a new `AdminService` per request.
    pub reindex_tasks: Arc<std::sync::Mutex<HashMap<String, ReindexProgress>>>,
    /// Repos with a running reindex task, keyed by repo_id, to prevent concurrent reindex.
    pub reindex_running: Arc<std::sync::Mutex<HashMap<String, ()>>>,
    /// Per-user TTL cache of the left-panel repo list (web UI).
    pub left_panel_cache: Arc<crate::ui::left_panel_cache::LeftPanelRepoCache>,
    /// Outbound mail: settings, queue and transport. Always present; whether it
    /// can actually send is decided by config and the saved settings.
    pub mail: Arc<crate::service::mail::Mailer>,
    /// The layered settings: saved rows, effective values, and the single point every
    /// admin save goes through.
    pub settings: Arc<SettingsService>,
    /// Set by an administrator asking for a restart at `/sysadmin/settings/`;
    /// the run loop in `main.rs` waits on it and rebuilds the server in place.
    pub restart: Arc<crate::restart::RestartSignal>,
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
    /// Build from a configuration that is already fully resolved: the config file is
    /// then the only layer, which is what the test harness and callers with no
    /// settings table have.
    pub fn new(db: DatabaseConnection, config: Config, temp_file_manager: TempFileManager) -> Self {
        let layers = SettingsLayers::plain(&config);
        Self::build(db, layers, temp_file_manager, None)
    }

    /// Build with the real layers: the config file plus the saved settings.
    ///
    /// The saved rows are read here, so every setting's value is decided in one place.
    pub fn new_with_layers(
        db: DatabaseConnection,
        layers: SettingsLayers,
        temp_file_manager: TempFileManager,
    ) -> Self {
        Self::build(db, layers, temp_file_manager, None)
    }

    /// Build with a task system that outlives this generation: the binary passes the
    /// process-lifetime instance, so run history and schedule state survive an in-place
    /// restart; tests and callers without a process loop get a private one.
    pub fn new_with_layers_and_tasks(
        db: DatabaseConnection,
        layers: SettingsLayers,
        temp_file_manager: TempFileManager,
        tasks: Arc<TaskSystem>,
    ) -> Self {
        Self::build(db, layers, temp_file_manager, Some(tasks))
    }

    fn build(
        db: DatabaseConnection,
        layers: SettingsLayers,
        temp_file_manager: TempFileManager,
        tasks: Option<Arc<TaskSystem>>,
    ) -> Self {
        let db = Arc::new(db);
        // The ciphers are derived from `secret_key`, a read-only setting: it can only come
        // from the environment or the config file, so the base layer already holds the value.
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

        // Every layer is applied inside the service: environment and config file as `base`,
        // saved rows on top. `config` below is what every startup-time capture reads.
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
        // Adopted from the process-lifetime task system, so a restart keeps the run table.
        let tasks = tasks.unwrap_or_else(|| {
            TaskSystem::new(
                crate::tasks::TaskLimits {
                    max_active_total: config.tasks.max_active_tasks as usize,
                    max_active_per_user: config.tasks.max_active_per_user as usize,
                    runs: crate::tasks::store::RunLimits {
                        max_retained_bytes: config.tasks.max_retained_bytes as usize,
                        ..Default::default()
                    },
                },
                shutdown_token.child_token(),
            )
        });

        let notification_manager =
            if config.notification.enabled && !config.notification.private_key.is_empty() {
                Some(NotificationManager::new(
                    config.notification.max_connections,
                    config.notification.max_connections_per_ip,
                    repos.clone(),
                ))
            } else {
                None
            };

        let auth_limiters = AuthRateLimiters::new(&config.auth);
        let pow = Arc::new(PowStore::new());

        // A migration-free feature can still be misconfigured into never firing;
        // say so before the operator concludes it is broken.
        if config.auth.captcha_mode == infra::config::CaptchaMode::Adaptive {
            if config.auth.lockout_duration_secs == 0 && config.auth.captcha_trigger_after > 0 {
                tracing::warn!(
                    "auth.captcha_mode is adaptive but auth.lockout_duration_secs is 0; \
                     the failure window is empty, so the challenge can never trigger"
                );
            } else if config.auth.max_login_attempts != 0
                && config.auth.captcha_trigger_after >= config.auth.max_login_attempts
            {
                tracing::warn!(
                    "auth.captcha_mode is adaptive but captcha_trigger_after ({}) is not below \
                     max_login_attempts ({}); the lockout fires first",
                    config.auth.captcha_trigger_after,
                    config.auth.max_login_attempts
                );
            }
        }

        let mut hasher = sha2::Sha256::new();
        hasher.update(b"csrf-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        let csrf_secret = Arc::new(hasher.finalize().to_vec());

        let password_manager = Arc::new(PasswordManager::new());

        // Sync tokens stay recoverable (clients re-present them), so they are encrypted at
        // rest with a key domain-separated from the server secret.

        // Apply sync-protocol hardening knobs (process-wide).
        crate::fs::core::traversal::configure(
            config.sync.max_tree_depth,
            config.sync.max_tree_visits,
        );
        crate::service::sync::configure_fs_object_verification(&config.sync.verify_fs_objects);
        // Upload-route body limit; the JSON default is applied in main.rs.
        crate::body_limit::configure(config.server.max_upload_size_mb);

        // The sandbox requirement and the media helper are process-global: resolve them
        // once, before any request is handed to a child. The parser limits are set inside
        // the child, the only process that parses. Both are set here rather than in the
        // indexer branch: thumbnails are generated whether or not the index is enabled.
        crate::sandbox::worker::configure_requirement(crate::sandbox::Requirement::new(
            config.sandbox.enabled,
        ));
        // Called even when the setting is empty, because empty is how an administrator says
        // there is no helper; skipping it would leave the media profile on the last helper.
        crate::sandbox::worker::configure_helper(configured_helper(&config.storage.ffmpeg_path));

        // Full-text indexer (its commit task is registered below with the other jobs).
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

        // Outbound mail: settings, outbox and SMTP transport. The mailer keeps the *handle*,
        // not a snapshot, so a saved SMTP setting is used by the very next delivery.
        let mail = Arc::new(crate::service::mail::Mailer::new(
            repos.clone(),
            token_cipher.clone(),
            runtime.clone(),
        ));
        // The startup diagnostics read the settings row, so they run as a task: `new` is not async.
        {
            let mail = mail.clone();
            tokio::spawn(async move { mail.log_startup_diagnostics() });
        }

        // Bind this generation's job bodies and start their intervals; the policies come
        // from the catalog and the bodies capture the resources built above.
        if let Err(e) = crate::tasks::setup::install_default_jobs(
            &tasks,
            shutdown_token.child_token(),
            &repos,
            &db,
            notification_manager.as_ref(),
            &password_manager,
            &config.gc,
            &block_store,
            indexer.as_ref(),
            config.index.backfill_enabled,
            &temp_file_manager,
            config.storage.temp_upload_ttl_hours,
            enc_mode,
            block_dir.as_ref(),
            Some(&mail),
        ) {
            // A mis-declared job is a programming error caught by the catalog tests; starting
            // with half a job set would be worse than refusing.
            panic!("the job catalog is invalid: {e}");
        }
        tasks.start_schedules();

        // In Lazy mode, run the one-shot legacy-block conversion at startup as a job.
        if enc_mode == infra::storage::encrypting_block_store::BlockEncryptionMode::Lazy {
            let tasks = tasks.clone();
            tokio::spawn(async move {
                // A startup pass: the row names the job from its slug, so no summary is passed.
                if let Err(e) = tasks
                    .submit_system(
                        crate::tasks::Origin::Startup,
                        crate::tasks::spec::JobKey::BlockEncryptionConvert,
                        serde_json::Value::Null,
                        String::new(),
                    )
                    .await
                {
                    tracing::warn!("could not start the legacy block conversion: {e}");
                }
            });
        }

        Self {
            repos,
            db,
            config: runtime,
            settings,
            block_store,
            block_dir,
            token_manager: Arc::new(AccessTokenManager::new()),
            notification_manager,
            indexer,
            auth_limiters,
            pow,
            csrf_secret,
            token_cipher,
            temp_file_manager,
            shutdown_token,
            password_manager,
            tasks,
            reindex_tasks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            reindex_running: Arc::new(std::sync::Mutex::new(HashMap::new())),
            left_panel_cache: Arc::new(crate::ui::left_panel_cache::LeftPanelRepoCache::default()),
            mail,
            restart: Arc::new(crate::restart::RestartSignal::new()),
        }
    }

    /// Push a saved setting into the long-lived object that captured it at startup.
    ///
    /// Only settings the catalog marks [`infra::settings::Apply::LiveWithHook`] need
    /// this: everything else is read from the snapshot per request, or is restart-only.
    pub fn apply_settings_hooks(&self, hooks: &std::collections::BTreeSet<infra::settings::Hook>) {
        use infra::settings::Hook;
        let config = self.config();
        for hook in hooks {
            match hook {
                Hook::RateLimits => self.auth_limiters.apply(&config.auth),
                Hook::TaskSystem => self.tasks.set_limits(crate::tasks::TaskLimits {
                    max_active_total: config.tasks.max_active_tasks as usize,
                    max_active_per_user: config.tasks.max_active_per_user as usize,
                    runs: crate::tasks::store::RunLimits {
                        max_retained_bytes: config.tasks.max_retained_bytes as usize,
                        ..Default::default()
                    },
                }),
                Hook::TaskLoad => {
                    self.tasks.configure_load(
                        config.tasks.load_aware,
                        config.tasks.load_sample_interval_secs,
                    );
                }
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
                // The outbox drainer is always registered and checks the live switch itself.
                Hook::MailDrain => {}
                Hook::Sandbox => {
                    crate::sandbox::worker::configure_requirement(
                        crate::sandbox::Requirement::new(config.sandbox.enabled),
                    );
                    // Re-resolve the helper when `storage.ffmpeg_path` is saved, so the
                    // next media request runs the helper the admin just configured; an
                    // emptied setting clears it, for the same reason it is cleared at startup.
                    crate::sandbox::worker::configure_helper(configured_helper(
                        &config.storage.ffmpeg_path,
                    ));
                }
            }
        }
    }

    /// Keep the running configuration in step with the settings table.
    ///
    /// A change made on another instance (or restored from a backup) reaches this
    /// process without a restart. The environment and the config file are deliberately
    /// *not* re-read: a live process silently changing one would be a surprise, not an update.
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
            // The first tick completes immediately; skip it so a start-up read is not duplicated.
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
    /// One call per request is enough: the returned `Arc` keeps that view consistent while held.
    pub fn config(&self) -> Arc<Config> {
        self.config.get()
    }

    /// Where a file mutation submits the index work it implies. Disabled when there is no
    /// indexer, so a call site never has to ask.
    pub fn index_scheduler(&self) -> crate::service::index::IndexScheduler {
        crate::service::index::IndexScheduler::new(self.tasks.clone(), self.indexer.is_some())
    }

    pub fn file_service(&self) -> crate::service::fs::file::FileService {
        crate::service::fs::file::FileService::new(
            self.repos.clone(),
            self.db.clone(),
            self.block_store.clone(),
            self.indexer.clone(),
            self.index_scheduler(),
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
            self.index_scheduler(),
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
            self.indexer.clone(),
            self.index_scheduler(),
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
            self.index_scheduler(),
        )
    }
}
