// Windows tray builds are GUI-subsystem binaries: the tray icon is the whole
// UI, so no console window is ever shown (double-click, Run-key autostart, or
// terminal). Logs go to a file instead (see logging.rs); non-tray builds keep
// the console subsystem for headless server use.
#![cfg_attr(
    all(target_os = "windows", feature = "tray"),
    windows_subsystem = "windows"
)]

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use rand::Rng;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use sea_orm_migration::MigratorTrait;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use infra::config::Config;
use infra::db::establish_connection;
use server::AppState;

mod console;
mod logging;

#[cfg(feature = "tray")]
mod tray;

/// Nanofile — a Seafile-compatible sync server
#[derive(Parser)]
#[command(name = "nanofile", version, about)]
struct Cli {
    /// Path to config.toml (overrides NANOFILE_CONFIG and the default).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Parser)]
enum Command {
    /// Start the HTTP server (default)
    Server,
    /// Create a new user account (admin by default, use --regular for non-admin)
    Adduser {
        /// Email address (also used as login name)
        #[arg(long)]
        email: Option<String>,
        /// Password. Visible in shell history and in `ps` output on this host,
        /// so prefer the interactive prompt, --password-stdin or
        /// --password-file.
        #[arg(long)]
        password: Option<String>,
        /// Read the password from the first line of standard input
        #[arg(
            long,
            default_value_t = false,
            conflicts_with_all = ["password", "password_file"]
        )]
        password_stdin: bool,
        /// Read the password from this file (leading/trailing whitespace is
        /// trimmed, matching NANOFILE_ADMIN_INIT_PASSWORD_FILE)
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with_all = ["password", "password_stdin"]
        )]
        password_file: Option<PathBuf>,
        /// Create a regular (non-admin) user
        #[arg(long, default_value_t = false)]
        regular: bool,
    },
    /// Migrate the legacy flat block layout (`blocks/<2hex>/<id>`) to the
    /// per-library layout. Normally runs automatically at startup before the
    /// server serves its first request; use this to pre-flight the copy volume
    /// (`--dry-run`) or to run it explicitly with the server stopped.
    MigrateBlocks {
        /// Only report what would be copied; touch nothing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

/// Commands sent from the optional system tray menu to the server task.
/// Defined unconditionally so `run_server` keeps a stable signature; the
/// variant is only ever constructed by the tray module.
#[allow(dead_code)]
enum TrayCommand {
    Quit,
}

type TrayCmdReceiver = std::sync::mpsc::Receiver<TrayCommand>;

async fn health_check() -> impl IntoResponse {
    StatusCode::OK
}

/// Path segments immediately preceding a capability token.
///
/// Routes such as `/f/{token}`, `/zip/{token}` and `/download-api/{token}` put
/// the credential itself in the path, so logging the raw URI would write a
/// usable token into the access log. That token is deliberately not paired with
/// a session: whoever reads the log can replay it.
const TOKEN_PATH_PREFIXES: &[&str] = &[
    "f",
    "d",
    "u",
    "zip",
    "blks",
    "download-api",
    "upload-api",
    "upload-aj",
    "update-api",
    "update-aj",
    "upload-blks-api",
    "upload-raw-blks-api",
    "client-sso",
    "client-login",
    "client-sso-link",
];

/// Replace capability-token path segments with `{token}` for logging.
///
/// The query string is dropped by the caller; it may carry a share-link
/// password or other credentials.
fn redact_request_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let mut out = String::with_capacity(path.len());
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        let prev = if i > 0 { segs[i - 1] } else { "" };
        if !seg.is_empty() && TOKEN_PATH_PREFIXES.contains(&prev) {
            out.push_str("{token}");
        } else {
            out.push_str(seg);
        }
    }
    out
}

/// Whether a configured secret has adequate length/entropy.
///
/// A 64-character value is treated as raw hex (matching `decode_master_key`);
/// anything else must provide at least 32 bytes of raw material.
fn secret_is_strong(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.len() == 64 {
        return s.chars().all(|c| c.is_ascii_hexdigit());
    }
    s.len() >= 32
}

/// Whether a secret that passed [`secret_is_strong`] still looks like a
/// placeholder or a single repeated character.
///
/// A 30+ byte `"aaaa…"` or `"changeme-…"` satisfies the length check while
/// carrying almost no entropy, and every key in the system (sessions, CSRF,
/// sync-token encryption, at-rest blocks, notification JWTs) is derived from
/// this value. Warn rather than refuse: refusing would break an existing
/// deployment on upgrade, which is the operator's call.
fn secret_looks_low_entropy(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    if chars.all(|c| c == first) {
        return true;
    }
    let lowered = s.to_ascii_lowercase();
    ["changeme", "secret", "password", "example", "nanofile"]
        .iter()
        .any(|placeholder| lowered.contains(placeholder))
}

/// Whether an explicitly configured notification signing key is too weak.
///
/// An empty value (or the legacy placeholder) is *not* weak: it has already
/// been replaced by a 256-bit key derived from `server.secret_key`, which is
/// itself required to be strong in release builds. Only a key the operator
/// set by hand can be weak here.
fn notification_key_is_weak(key: &str) -> bool {
    !key.is_empty() && key != "nanofile-notification-secret" && !secret_is_strong(key)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut config = match &cli.config {
        Some(path) => Config::load_from(path)?,
        None => Config::load()?,
    };

    // Same resolution order as `Config::load()` — the tray's auto-start
    // entries pass it on and the log target persistence uses it.
    let config_path: PathBuf = match &cli.config {
        Some(path) => path.clone(),
        None => std::env::var(infra::config::CONFIG_PATH_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(infra::config::DEFAULT_CONFIG_PATH)),
    };

    // Panics must reach the log file even in GUI-subsystem Windows builds
    // (no console shows them), so install the hook before anything else.
    logging::install_panic_hook();

    // ── Decide the run mode first: the log target depends on it ────────
    let command = cli.command.unwrap_or(Command::Server);
    #[cfg(feature = "tray")]
    let (tray_mode, headless_reason) = match command {
        Command::Server => match tray::run_mode(&config) {
            tray::RunMode::Tray => (true, None),
            tray::RunMode::Headless(reason) => (false, Some(reason)),
        },
        _ => (false, None),
    };
    #[cfg(not(feature = "tray"))]
    let (tray_mode, headless_reason): (bool, Option<&'static str>) = (false, None);

    // CLI subcommands may be interactive; GUI-subsystem builds have no
    // console, so reattach to the launching terminal before any output.
    if !matches!(command, Command::Server) {
        console::attach_parent_console();
    }

    logging::init(
        &config,
        if matches!(command, Command::Server) {
            logging::Kind::Server
        } else {
            logging::Kind::Cli
        },
        tray_mode,
        &config_path,
    );
    if let Some(reason) = headless_reason {
        tracing::info!("{reason}");
    }

    // ── Absolute-URL host trust ────────────────────────────────────────
    // While `site_url` is unconfigured the request Host is echoed into
    // download/block URLs so LAN clients get a reachable address. That value is
    // client-supplied, so an attacker who can make a client use their hostname
    // (DNS rebinding, wildcard vhost) receives the client's capability token.
    if config.server.site_url_is_default() && config.server.allowed_hosts.is_empty() {
        if config.server.trust_request_host {
            tracing::warn!(
                "site_url is still the built-in default and server.allowed_hosts is empty: \
                 download/block URLs will echo a syntactically valid request Host header, \
                 which is attacker-influenced behind a wildcard vhost or a proxy that \
                 forwards arbitrary Host values. Set server.site_url to the address \
                 clients use, restrict server.allowed_hosts, or disable the echo with \
                 server.trust_request_host = false."
            );
        } else {
            tracing::info!(
                "site_url is still the built-in default; download/block URLs will use it \
                 verbatim (server.trust_request_host = false)."
            );
        }
    }

    // ── Server secret key ──────────────────────────────────────────────
    // Release builds require an explicit, high-entropy secret: the storage
    // encryption master key, CSRF/notification JWT keys and the sync-token
    // encryption key are all derived from it, so an ephemeral secret silently
    // makes encrypted data unreadable after a restart and rotates sessions.
    // Debug builds keep the zero-config auto-generated behaviour for local
    // development. Only `nanofile server` needs the secret; admin CLI
    // subcommands (adduser, --version, ...) must not be blocked by it.
    let is_dev = cfg!(debug_assertions);
    let allow_ephemeral = std::env::var("NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let needs_secret = matches!(command, Command::Server);

    if !secret_is_strong(&config.server.secret_key) {
        if is_dev || allow_ephemeral || !needs_secret {
            // An ephemeral key is regenerated on every start, which silently
            // invalidates every key derived from it. For sessions and CSRF that
            // is an inconvenience; with at-rest block encryption it is
            // irreversible data loss (the stored blocks can never be decrypted
            // again). Refuse instead of starting a deployment that will destroy
            // its own data on the next restart.
            if needs_secret
                && config.storage.block_encryption_mode()
                    != infra::storage::encrypting_block_store::BlockEncryptionMode::Off
            {
                anyhow::bail!(
                    "storage encryption is enabled but the server secret key is not set: an \
                     ephemeral key would be regenerated on every start, making every stored \
                     block permanently unreadable. Set NANOFILE_SERVER_SECRET_KEY (or \
                     [server] secret_key) to a strong, persisted value."
                );
            }
            if config.server.secret_key.is_empty()
                || config.server.secret_key == "nanofile-server-secret"
            {
                let mut key = [0u8; 32];
                rand::rng().fill_bytes(&mut key);
                config.server.secret_key = hex::encode(key);
            }
            tracing::warn!(
                "Server secret key is not explicitly configured with a strong value; \
                 set NANOFILE_SERVER_SECRET_KEY (or [server] secret_key) to persist it."
            );
        } else {
            anyhow::bail!(
                "NANOFILE_SERVER_SECRET_KEY / [server] secret_key must be set to a \
                 high-entropy value of at least 32 bytes (64 hex chars is recommended) \
                 in release builds; generate one with `openssl rand -hex 32`. \
                 Set NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1 to override for \
                 local/CI use."
            );
        }
    }

    if secret_is_strong(&config.server.secret_key)
        && secret_looks_low_entropy(&config.server.secret_key)
        && needs_secret
        && !is_dev
        && !allow_ephemeral
    {
        anyhow::bail!(
            "server secret_key passes the length check but looks like a placeholder or a \
             repeated character. Every session, CSRF, sync-token and at-rest key is derived \
             from it; generate one with `openssl rand -hex 32`."
        );
    }
    if secret_is_strong(&config.server.secret_key)
        && secret_looks_low_entropy(&config.server.secret_key)
    {
        tracing::warn!(
            "server secret_key passes the length/format check but looks low-entropy              (repeated characters or a placeholder). Every session, CSRF, sync-token \
             and at-rest key is derived from it; generate one with `openssl rand -hex 32`."
        );
    }

    // An explicitly configured at-rest encryption key must also be strong.
    if let Some(key) = &config.storage.encryption_key
        && !secret_is_strong(key)
    {
        if is_dev || allow_ephemeral || !needs_secret {
            tracing::warn!(
                "NANOFILE_STORAGE_ENCRYPTION_KEY is shorter than 32 bytes; \
                 use `openssl rand -hex 32` for adequate strength."
            );
        } else {
            anyhow::bail!(
                "NANOFILE_STORAGE_ENCRYPTION_KEY(_FILE) must be at least 32 bytes \
                 of high entropy (64 hex chars); generate one with `openssl rand -hex 32`."
            );
        }
    }

    // ── Token lifetimes ────────────────────────────────────────────────
    // `0` is not "use the default" for these two: on the API login path it means
    // the token never expires, so it silently turns into a permanent bearer
    // credential. The web-UI path reads `0` the opposite way, which makes this
    // easy to misread — say so out loud rather than letting it pass silently.
    if config.auth.api_token_ttl_days == 0 {
        tracing::warn!(
            "auth.api_token_ttl_days = 0: account tokens issued by POST /api2/auth-token/ \
             will NEVER expire (they are only revoked by a password change/reset or by \
             deactivating the account). Set a positive value unless that is intended."
        );
    }
    if config.auth.sync_token_ttl_days == 0 {
        tracing::warn!("auth.sync_token_ttl_days = 0: repository sync tokens will NEVER expire.");
    }

    // Orphan blocks are no longer a quota bypass — every block write is charged
    // as an uncommitted write (`service::fs::quota::reserve_block_bytes`) — but
    // with GC off nothing ever reclaims blocks that no commit references (an
    // upload abandoned before its final chunk, a sync whose branch update
    // failed). Say so rather than letting the disk watermark grow silently.
    if !config.gc.enabled {
        tracing::warn!(
            "gc.enabled = false: blocks and FS objects that no commit references are \
             never reclaimed. Per-user quota still bounds how much one user may write, \
             but total disk usage only grows. Enable [gc] to reclaim them."
        );
    }

    // ── Derive notification private key from secret_key if not set ─────
    if config.notification.private_key.is_empty()
        || config.notification.private_key == "nanofile-notification-secret"
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"notify-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        config.notification.private_key = hex::encode(hasher.finalize());
    }

    // An explicitly configured notification signing key must also be strong.
    // Guessing it lets an attacker mint subscription JWTs for any repository
    // (and event JWTs, when `accept_legacy_event_tokens` is on).
    if notification_key_is_weak(&config.notification.private_key) {
        if is_dev || allow_ephemeral || !needs_secret {
            tracing::warn!(
                "[notification] private_key is shorter than 32 bytes; use \
                 `openssl rand -hex 32`, or leave it empty to derive one from \
                 the server secret key."
            );
        } else {
            anyhow::bail!(
                "[notification] private_key / NANOFILE_NOTIFICATION_PRIVATE_KEY must \
                 be at least 32 bytes of high entropy (64 hex chars is recommended); \
                 generate one with `openssl rand -hex 32`, or leave it empty to \
                 derive it from the server secret key."
            );
        }
    }

    // ── Derive storage encryption key from secret_key if not set ────────
    let enc_mode = config.storage.block_encryption_mode();
    if enc_mode != infra::storage::encrypting_block_store::BlockEncryptionMode::Off
        && config.storage.encryption_key.is_none()
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"storage-encrypt-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        config.storage.encryption_key = Some(hex::encode(hasher.finalize()));
        tracing::info!(
            "Storage encryption key auto-derived from server secret key. \
             Set NANOFILE_STORAGE_ENCRYPTION_KEY(_FILE) to use a dedicated key."
        );
    }

    match command {
        Command::Server => {
            // With the tray feature compiled in and a desktop session
            // present, the platform tray event loop owns the main thread and
            // the server runs on background tokio workers instead.
            #[cfg(feature = "tray")]
            if tray_mode {
                tray::run(config, config_path);
            }

            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_server_flow(config, None))
        }
        Command::Adduser {
            email,
            password,
            password_stdin,
            password_file,
            regular,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let db = establish_connection(&config.database).await?;
                migration::Migrator::up(&db, None).await?;
                adduser(
                    db,
                    config,
                    email,
                    password,
                    password_stdin,
                    password_file,
                    regular,
                )
                .await
            })
        }
        Command::MigrateBlocks { dry_run } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let db = establish_connection(&config.database).await?;
                migration::Migrator::up(&db, None).await?;
                infra::common::util::ensure_private_dir(&config.storage.block_dir)?;
                let mode = if dry_run {
                    server::fs::core::block_migration::MigrationMode::DryRun
                } else {
                    server::fs::core::block_migration::MigrationMode::Apply
                };
                let report = server::fs::core::block_migration::BlockLayoutMigration::run(
                    &db,
                    &config.storage.block_dir,
                    mode,
                )
                .await?;
                println!("{}", report.summary());
                if !report.missing_block_ids.is_empty() {
                    println!(
                        "missing blocks (first {}): {}",
                        report.missing_block_ids.len(),
                        report.missing_block_ids.join(", ")
                    );
                }
                anyhow::Ok(())
            })
        }
    }
}

/// DB setup plus server startup — shared by the headless path and the tray
/// path (`tray::run` spawns this onto its background runtime).
async fn run_server_flow(config: Config, tray_cmd: Option<TrayCmdReceiver>) -> anyhow::Result<()> {
    let db = establish_connection(&config.database).await?;
    migration::Migrator::up(&db, None).await?;
    run_server(db, config, tray_cmd).await
}

async fn run_server(
    db: DatabaseConnection,
    config: Config,
    tray_cmd: Option<TrayCmdReceiver>,
) -> anyhow::Result<()> {
    tracing::info!(
        "starting nanofile server on {}:{}",
        config.server.addr,
        config.server.port
    );

    // Ensure every private data directory exists with owner-only permissions.
    // `ensure_private_dir` is also used for the log directory (which is created
    // earlier, during `logging::init`), so all of them are covered regardless
    // of the operator's umask.
    for dir in [
        &config.storage.block_dir,
        &config.storage.temp_dir,
        &config.storage.thumbnail_dir,
        &config.storage.avatar_dir,
        &config.index.index_dir,
    ] {
        infra::common::util::ensure_private_dir(dir)?;
    }

    // ── Block layout migration (blocks are stored per library) ─────────
    // The legacy flat layout (`blocks/<2hex>/<id>`) has no read path in the
    // running server, so the migration must complete *before* anything serves
    // a request: starting with the legacy tree still present would 404 every
    // download. It is a pure copy, resumable, and removes the legacy tree only
    // after every referenced block is in its new place.
    {
        let report = server::fs::core::block_migration::BlockLayoutMigration::run(
            &db,
            &config.storage.block_dir,
            server::fs::core::block_migration::MigrationMode::Apply,
        )
        .await?;
        if report.already_migrated {
            tracing::debug!("{}", report.summary());
        } else {
            tracing::info!("{}", report.summary());
        }
        if report.missing_sources > 0 {
            tracing::warn!(
                missing = report.missing_sources,
                sample = ?report.missing_block_ids,
                "referenced blocks were already missing from the legacy block store; \
                 their files will report missing blocks until restored from a backup"
            );
        }
    }

    let temp_file_manager = server::handler::web::temp_file::TempFileManager::new(
        config.storage.temp_dir.clone(),
        config.storage.max_temp_uploads,
        config.storage.max_temp_upload_bytes,
    )
    .await;

    let state = Arc::new(AppState::new(db, config.clone(), temp_file_manager));

    // Rewrite any legacy plaintext sync tokens as AEAD ciphertext. This is
    // non-disruptive — clients keep presenting the same raw token — and must
    // happen before the HTTP server starts serving requests.
    server::repository::sync_token::encrypt_legacy_sync_tokens(
        state.db.as_ref(),
        &state.token_cipher,
    )
    .await?;

    // ── Auto-create admin user from config/env on first startup ──────
    if let (Some(admin_email), Some(admin_password)) = (
        &state.config.admin_init.email,
        &state.config.admin_init.password,
    ) {
        let count = infra::entity::user::Entity::find()
            .count(state.db.as_ref())
            .await?;
        if count == 0 {
            tracing::info!("No users found; creating initial admin user");
            let password_hash = server::service::auth::password::hash_password(
                admin_password,
                state.config.auth.password_hash_iterations,
            );
            let now = chrono::Utc::now().timestamp();
            let model = infra::entity::user::ActiveModel {
                id: sea_orm::NotSet,
                email: Set(admin_email.clone()),
                password_hash: Set(password_hash),
                is_active: Set(true),
                is_admin: Set(true),
                created_at: Set(now),
                last_login_at: Set(None),
                invited_by: Set(None),
                storage_quota: sea_orm::NotSet,
                name: sea_orm::NotSet,
                display_name: sea_orm::NotSet,
                language: sea_orm::NotSet,
            };
            model.insert(state.db.as_ref()).await?;
            tracing::info!("Admin user '{}' created", admin_email);
        } else {
            tracing::debug!(
                "Users already exist (count={}), skipping admin auto-creation",
                count
            );
        }
    }

    let cors = {
        // `cors_origins()` returns `[site_url_origin()]` when the
        // configured list is empty, so this always allows the same-origin
        // site (and any explicitly configured origins).
        let origins = state.config.server.cors_origins();

        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins.into_iter().filter_map(|o| {
                o.parse()
                    .map_err(|e| tracing::warn!("Skipping invalid CORS origin '{}': {:?}", o, e))
                    .ok()
            })))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::HeaderName::from_static("x-requested-with"),
                header::HeaderName::from_static("x-seafile-otp"),
                header::HeaderName::from_static("x-seafile-s2fa"),
                header::HeaderName::from_static("x-seafile-sharelink-password"),
                header::HeaderName::from_static("x-seafile-2fa-trust-device"),
            ])
            .max_age(std::time::Duration::from_secs(
                state.config.server.cors_max_age_secs,
            ))
    };

    // Upload-capable route groups accept bodies up to `max_upload_size_mb`.
    // The app-wide default below is the much smaller JSON/Form cap.
    let upload_body_limit = server::body_limit::upload_limit();
    let sync_routes =
        server::handler::sync::sync_routes().layer(DefaultBodyLimit::max(upload_body_limit));
    let web_routes =
        server::handler::web::web_routes().layer(DefaultBodyLimit::max(upload_body_limit));
    let ui_routes = server::ui::ui_routes();
    let notification_routes = server::notification::notification_routes();
    let webdav_routes =
        server::webdav::webdav_routes().layer(DefaultBodyLimit::max(upload_body_limit));

    // CORS is applied only to the REST API routes. It must not wrap
    // the WebDAV endpoints: tower-http's CorsLayer answers OPTIONS
    // requests itself, which would shadow the `DAV:`/`Allow:` response
    // WebDAV clients expect. WebDAV clients are not browsers, so CORS
    // does not apply to them.
    let api_with_cors = server::routes::api_routes().layer(cors);

    let app = Router::new()
        .route("/health", get(health_check))
        .merge(api_with_cors)
        .merge(sync_routes)
        .merge(web_routes)
        .merge(ui_routes)
        .merge(notification_routes)
        .merge(webdav_routes)
        .merge(server::handler::avatar::image_routes())
        .route("/static/{*path}", get(server::static_assets::serve_static))
        .layer(DefaultBodyLimit::max(
            (config.server.max_json_body_mb * 1024 * 1024) as usize,
        ))
        .layer(RequestBodyLimitLayer::new(
            (config.server.max_upload_size_mb * 1024 * 1024) as usize,
        ))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                // Log a redacted path (no query string, token segments masked)
                // rather than the default full URI, which would put share,
                // upload, download and SSO capability tokens into the log.
                .make_span_with(|req: &axum::http::Request<axum::body::Body>| {
                    tracing::info_span!(
                        "request",
                        method = %req.method(),
                        path = %redact_request_path(req.uri().path()),
                        latency = tracing::field::Empty,
                        status = tracing::field::Empty,
                    )
                })
                .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::INFO))
                .on_response(
                    tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO),
                )
                .on_failure(tower_http::trace::DefaultOnFailure::new().level(tracing::Level::WARN)),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(config.server.request_timeout_secs),
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            server::middleware::security_headers,
        ))
        .with_state(state.clone());

    // Optionally bound how long a client may take to send the request body.
    // Off by default: `request_timeout_secs` already caps the whole handler,
    // including body reads.
    let app = if config.server.body_timeout_secs > 0 {
        app.layer(tower_http::timeout::RequestBodyTimeoutLayer::new(
            std::time::Duration::from_secs(config.server.body_timeout_secs),
        ))
    } else {
        app
    };

    let addr = format!("{}:{}", config.server.addr, config.server.port);
    tracing::info!("listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // ── Start server with graceful shutdown via oneshot ─────────────
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // ConnectInfo exposes the TCP peer address to handlers so rate
    // limiting can use the real client IP instead of the spoofable
    // X-Forwarded-For header.
    let header_read_timeout = match config.server.header_read_timeout_secs {
        0 => None,
        secs => Some(std::time::Duration::from_secs(secs)),
    };
    let server_handle = tokio::spawn(async move {
        server::serve::serve_with_timeouts(listener, app, shutdown_rx, header_read_timeout).await
    });

    // ── Wait for Ctrl+C, SIGTERM or a tray quit request ─────────────
    let ctrl_c = tokio::signal::ctrl_c();
    let terminate = async {
        #[cfg(unix)]
        {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler")
                .recv()
                .await;
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    };
    // Bridge the tray menu's std channel into async: a blocking thread parks
    // on `recv()` until the tray sends a command (or the channel closes,
    // which cannot happen while the tray loop is alive).
    let tray_quit = async {
        if let Some(rx) = tray_cmd
            && let Ok(Ok(TrayCommand::Quit)) = tokio::task::spawn_blocking(move || rx.recv()).await
        {
            return;
        }
        std::future::pending::<()>().await;
    };

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received SIGINT (Ctrl+C)"),
        _ = terminate => tracing::info!("Received SIGTERM"),
        _ = tray_quit => tracing::info!("Quit requested from tray"),
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown...");

    // ── Signal the server to drain, bounded to 25 seconds ──────────
    let _ = shutdown_tx.send(());
    match tokio::time::timeout(std::time::Duration::from_secs(25), server_handle).await {
        Ok(Ok(Ok(()))) => tracing::info!("Server finished normally"),
        Ok(Ok(Err(e))) => tracing::error!("Server error: {e}"),
        Ok(Err(e)) => tracing::error!("Server task panicked: {e}"),
        Err(_) => tracing::warn!("Drain timed out after 25s, proceeding with cleanup"),
    }

    // ── Graceful shutdown sequence ──────────────────────────────────
    tracing::info!("Stopping background tasks...");
    state.shutdown_token.cancel();

    // Close WebSocket connections cleanly.
    if let Some(ref mgr) = state.notification_manager {
        mgr.shutdown().await;
    }

    // Commit Tantivy indexer.
    if let Some(ref indexer) = state.indexer
        && let Err(e) = indexer.commit()
    {
        tracing::error!("Failed to commit indexer during shutdown: {e}");
    }

    // DB connection is dropped when `state` goes out of scope;
    // the OS handles final file descriptor cleanup.

    tracing::info!("Server shutdown complete");

    Ok(())
}

/// Read a password from the first line of `reader`.
///
/// Whitespace is trimmed to match how `NANOFILE_ADMIN_INIT_PASSWORD_FILE` and
/// `NANOFILE_STORAGE_ENCRYPTION_KEY_FILE` are read, so a trailing newline (or
/// a CRLF line ending) never becomes part of the password.
fn read_password_line<R: std::io::BufRead>(reader: R) -> anyhow::Result<String> {
    let mut line = String::new();
    let mut reader = reader;
    reader.read_line(&mut line)?;
    let password = line.trim();
    if password.is_empty() {
        anyhow::bail!("no password provided on standard input");
    }
    Ok(password.to_owned())
}

async fn adduser(
    db: DatabaseConnection,
    config: Config,
    email: Option<String>,
    password: Option<String>,
    password_stdin: bool,
    password_file: Option<PathBuf>,
    regular: bool,
) -> anyhow::Result<()> {
    use infra::entity::user;

    let email = match email {
        Some(e) => e,
        None => {
            print!("email: ");
            std::io::stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            input.trim().to_owned()
        }
    };

    let password = match (password, password_stdin, password_file) {
        // A command-line password is readable by every local user through
        // `ps` and lands in the shell history; keep it working for scripts
        // that already rely on it, but steer the operator elsewhere.
        (Some(p), _, _) => {
            eprintln!(
                "warning: --password is visible in shell history and process listings; \
                 prefer the interactive prompt, --password-stdin or --password-file"
            );
            p
        }
        (None, true, _) => read_password_line(std::io::stdin().lock())?,
        (None, false, Some(path)) => {
            let file = std::fs::File::open(&path).map_err(|e| {
                anyhow::anyhow!("cannot read password file '{}': {e}", path.display())
            })?;
            read_password_line(std::io::BufReader::new(file))?
        }
        (None, false, None) => rpassword::prompt_password("password: ")?,
    };

    let exists = user::Entity::find()
        .filter(user::Column::Email.eq(&email))
        .one(&db)
        .await?;

    if exists.is_some() {
        anyhow::bail!("user '{}' already exists", email);
    }

    let password_hash = server::service::auth::password::hash_password(
        &password,
        config.auth.password_hash_iterations,
    );
    let now = chrono::Utc::now().timestamp();

    let is_admin = !regular;
    let model = user::ActiveModel {
        id: sea_orm::NotSet,
        email: Set(email.clone()),
        password_hash: Set(password_hash),
        is_active: Set(true),
        is_admin: Set(is_admin),
        created_at: Set(now),
        last_login_at: Set(None),
        invited_by: Set(None),
        storage_quota: sea_orm::NotSet,
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
        language: sea_orm::NotSet,
    };

    model.insert(&db).await?;
    println!("user '{}' created successfully", email);
    Ok(())
}

#[cfg(test)]
mod cli_password_tests {
    use super::read_password_line;

    #[test]
    fn reads_first_line_and_trims_line_ending() {
        let pw = read_password_line(std::io::Cursor::new(b"secret123\n")).unwrap();
        assert_eq!(pw, "secret123");
        // CRLF (e.g. a file written on Windows) must not keep the CR.
        let pw = read_password_line(std::io::Cursor::new(b"secret123\r\n")).unwrap();
        assert_eq!(pw, "secret123");
        // Only the first line is used, and surrounding whitespace is trimmed
        // exactly like the *_FILE environment variables.
        let pw = read_password_line(std::io::Cursor::new(b"  secret123  \nignored\n")).unwrap();
        assert_eq!(pw, "secret123");
    }

    #[test]
    fn rejects_empty_input() {
        assert!(read_password_line(std::io::Cursor::new(b"")).is_err());
        assert!(read_password_line(std::io::Cursor::new(b"\n")).is_err());
        assert!(read_password_line(std::io::Cursor::new(b"   \n")).is_err());
    }
}

#[cfg(test)]
mod secret_strength_tests {
    use super::{notification_key_is_weak, secret_is_strong};

    #[test]
    fn rejects_empty_and_placeholder() {
        assert!(!secret_is_strong(""));
        assert!(!secret_is_strong("nanofile-server-secret"));
        assert!(!secret_is_strong("short-secret"));
    }

    #[test]
    fn accepts_64_hex_and_long_raw() {
        assert!(secret_is_strong(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(secret_is_strong("a-32-byte-plus-raw-secret-value!!!"));
    }

    #[test]
    fn rejects_non_hex_64_char_value() {
        // 64 chars that are not hex would panic in `decode_master_key`.
        assert!(!secret_is_strong(&"z".repeat(64)));
    }

    #[test]
    fn notification_key_only_weak_when_explicitly_set() {
        // Derived (empty) and legacy placeholder values are replaced at
        // startup by a 256-bit derived key, so they are not reportable here.
        assert!(!notification_key_is_weak(""));
        assert!(!notification_key_is_weak("nanofile-notification-secret"));
        // An explicitly configured strong key passes.
        assert!(!notification_key_is_weak(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!notification_key_is_weak(
            "a-32-byte-plus-raw-secret-value!!!"
        ));
        // Short, non-hex-64 or placeholder-length values are weak.
        assert!(notification_key_is_weak("abc"));
        assert!(notification_key_is_weak("nanofile-notify"));
        assert!(notification_key_is_weak(&"z".repeat(64)));
    }
}
