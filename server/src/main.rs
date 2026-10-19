// Windows tray builds are GUI-subsystem binaries: no console window is ever shown,
// so logs go to a file instead (see logging.rs).
#![cfg_attr(
    all(target_os = "windows", feature = "tray"),
    windows_subsystem = "windows"
)]

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

use infra::config::{Config, EnvKeys};
use infra::db::establish_connection;
use server::AppState;

mod console;
mod logging;

#[cfg(feature = "tray")]
mod tray;

/// Auto-start registrations (login entry, Windows service) the tray menu drives,
/// compiled everywhere so its helpers stay testable but reachable only on Windows.
mod startup;

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
        /// Password. Visible in shell history and in `ps` output, so prefer the
        /// interactive prompt, --password-stdin or --password-file.
        #[arg(long)]
        password: Option<String>,
        /// Read the password from the first line of standard input
        #[arg(
            long,
            default_value_t = false,
            conflicts_with_all = ["password", "password_file"]
        )]
        password_stdin: bool,
        /// Read the password from this file (whitespace trimmed, like
        /// NANOFILE_ADMIN_INIT_PASSWORD_FILE)
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
    /// per-library layout (automatic at startup; `--dry-run` pre-flights the copy).
    MigrateBlocks {
        /// Only report what would be copied; touch nothing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Verify stored blocks against their content-addressed ids and file objects
    /// against the sizes they declare. Read-only unless `--repair` is given.
    VerifyBlocks {
        /// Report only (the default); rejects `--repair`.
        #[arg(long, default_value_t = false, conflicts_with = "repair")]
        dry_run: bool,
        /// Quarantine blocks whose bytes do not hash to their id, so a client
        /// holding a good copy re-uploads them. Bytes are never deleted.
        #[arg(long, default_value_t = false)]
        repair: bool,
        /// Restrict the check to one library.
        #[arg(long, value_name = "REPO_ID")]
        repo: Option<String>,
        /// Machine-readable report on stdout.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Verify that every directory object reachable from a library's HEAD is
    /// stored in seafile's canonical (descending by name) order, which makes its
    /// `fs_id` reproducible by an official client. Read-only unless `--repair`.
    VerifyDirOrder {
        /// Report only (the default); rejects `--repair`.
        #[arg(long, default_value_t = false, conflicts_with = "repair")]
        dry_run: bool,
        /// Append one normalizing commit per affected library; history is not
        /// rewritten and no object is deleted, so clients see an ordinary update.
        #[arg(long, default_value_t = false)]
        repair: bool,
        /// Restrict the check to one library.
        #[arg(long, value_name = "REPO_ID")]
        repo: Option<String>,
        /// Machine-readable report on stdout.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Windows service control. `run` is what the registered service's binary
    /// path calls; `install`/`uninstall` need administrator rights.
    #[cfg(target_os = "windows")]
    Service {
        #[command(subcommand)]
        action: startup::service::ServiceAction,
    },
    /// Run one confined job (`sandbox::worker`).
    ///
    /// Spawned for sandbox-backed work (document extraction, image decode, EXIF
    /// and ffmpeg thumbnails) so the parser runs under the address-space, CPU and
    /// timeout limits and the platform confinement. Hidden: it reads one request
    /// from stdin, and `--selftest` reports what the sandbox could establish.
    #[command(hide = true)]
    ExtractWorker {
        /// The parent wrapped this process in the platform runner (macOS
        /// `sandbox-exec`): files, network and process creation are not its to confine.
        #[arg(long, default_value_t = false)]
        seatbelt: bool,
        /// Report the confinement this host can give, then exit.
        #[arg(long, default_value_t = false)]
        selftest: bool,
        /// Run the probe a serving process runs at startup, print what it found,
        /// and exit non-zero when the worker is unavailable.
        #[arg(long, default_value_t = false)]
        probe: bool,
        /// Which pipeline this child confines itself for.
        #[arg(
            long,
            value_name = "documents|images|media",
            default_value = "documents"
        )]
        profile: String,
        /// `sandbox.enabled` is false: refuse before reading a request.
        #[arg(long, default_value_t = false)]
        sandbox_off: bool,
        /// The configured media helper the media profile may execute.
        #[arg(long, value_name = "PATH")]
        ffmpeg: Option<String>,
        /// The one scratch source the media profile may read.
        #[arg(long, value_name = "PATH")]
        src: Option<String>,
        /// A file the parent created that no profile grants; reading it must be
        /// refused for the files layer to count. Startup probe only, never a request.
        #[arg(long, value_name = "PATH")]
        probe_path: Option<String>,
    },
}

/// Commands sent from the tray menu (and, on Windows, the service control
/// handler). Defined unconditionally so `run_server` keeps a stable signature.
#[allow(dead_code)]
enum TrayCommand {
    Quit,
}

/// A tokio channel, not std: the run loop restarts in place and must keep waiting on
/// the same receiver (`recv(&mut self)`); senders are off-runtime threads, so an
/// unbounded send never blocks them.
type TrayCmdReceiver = tokio::sync::mpsc::UnboundedReceiver<TrayCommand>;

/// Whether a configured secret has adequate length/entropy. A 64-character value
/// is raw hex (matching `decode_master_key`); anything else needs 32+ raw bytes.
fn secret_is_strong(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.len() == 64 {
        return s.chars().all(|c| c.is_ascii_hexdigit());
    }
    s.len() >= 32
}

/// Whether a secret that passed [`secret_is_strong`] still looks like a placeholder
/// or a single repeated character. Every key in the system (sessions, CSRF,
/// sync-token encryption, at-rest blocks, notification JWTs) derives from it, so warn
/// rather than refuse: refusing would break an existing deployment on upgrade.
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

/// Whether an explicitly configured notification signing key is too weak. Empty or
/// the legacy placeholder is not: it is replaced by a 256-bit key derived from
/// `server.secret_key`.
fn notification_key_is_weak(key: &str) -> bool {
    !key.is_empty() && key != "nanofile-notification-secret" && !secret_is_strong(key)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let command = cli.command.unwrap_or(Command::Server);

    // The extraction worker runs first: a child process with no state that must not
    // need a config file, a database or a log target. Confining itself is its first
    // act (`worker::run`).
    if let Command::ExtractWorker {
        seatbelt,
        selftest,
        probe,
        profile,
        sandbox_off,
        ffmpeg,
        src,
        probe_path,
    } = &command
    {
        let profile =
            server::sandbox::Profile::parse(profile).unwrap_or(server::sandbox::Profile::Documents);
        // The requirement the child judges itself by: the master switch, and nothing
        // else. The resource bound is read from its own report, so parent and child
        // apply the same rule.
        let requirement = server::sandbox::Requirement::new(!sandbox_off);
        // What the media profile may reach: these paths come from the parent on the
        // command line because the rules they become must be installed before the
        // request is read.
        let grants = server::sandbox::Grants {
            helper: ffmpeg.as_deref().map(std::path::Path::new),
            source: src.as_deref().map(std::path::Path::new),
        };
        // `--probe` runs the parent's half of the startup check and applies the
        // requirement to the report it gets back, so the pair must be known here; the
        // server takes the same path through `lib.rs`.
        server::sandbox::worker::configure_requirement(requirement);
        if let Some(helper) = &grants.helper {
            server::sandbox::worker::configure_helper(helper.to_path_buf());
        }
        if *probe {
            return server::sandbox::worker::probe_report(profile);
        }
        let job = if *selftest {
            server::sandbox::worker::Job::Selftest
        } else {
            server::sandbox::worker::Job::Extract
        };
        let external = server::sandbox::worker::External { runner: *seatbelt };
        return server::sandbox::worker::run(
            job,
            external,
            profile,
            requirement,
            grants,
            probe_path.as_deref().map(std::path::Path::new),
        );
    }
    // A service is the server without a desktop: no console, no session dialog, and
    // never a tray icon. Everything else is the ordinary headless server.
    #[cfg(target_os = "windows")]
    let service_run = matches!(
        command,
        Command::Service {
            action: startup::service::ServiceAction::Run
        }
    );
    #[cfg(not(target_os = "windows"))]
    let service_run = false;
    let serving = matches!(command, Command::Server) || service_run;

    // A config file named on the command line or by `NANOFILE_CONFIG` has to exist:
    // auto-start entries and service registrations record these paths, and falling back
    // to defaults would quietly start a *second* instance against the default database
    // and port instead of reporting the broken reference. Failing here is before the
    // subscriber exists, so the reason is delivered by hand: a log file, and a dialog
    // only when there is a desktop.
    let (loaded, resolved_config) = match Config::load_for_start_with_path(cli.config.as_deref()) {
        Ok(loaded) => loaded,
        Err(e) => {
            logging::report_startup_failure(&e, !service_run);
            return Err(e);
        }
    };
    let mut config = loaded.config;
    let mut env_keys = loaded.env_keys;

    // The path this start resolved: what an auto-start entry records, what log
    // persistence writes, and where a first-run setup writes. One source of truth.
    let config_path: PathBuf = resolved_config.path.clone();

    // Panics must reach the log file even in GUI-subsystem Windows builds, so hook first.
    logging::install_panic_hook();

    // Every `service …` action runs without a console in a GUI-subsystem build, and its
    // user-visible failure channel is a message box; the log file keeps the record.
    #[cfg(target_os = "windows")]
    let service_command = matches!(command, Command::Service { .. });
    #[cfg(not(target_os = "windows"))]
    let service_command = false;

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

    // CLI subcommands may be interactive and GUI-subsystem builds have no console, so
    // reattach before any output. A serving process and the elevated helper have no
    // parent console for `AttachConsole` to find.
    if !serving {
        console::attach_parent_console();
    }

    logging::init(
        &config,
        if serving || service_command {
            logging::Kind::Server
        } else {
            logging::Kind::Cli
        },
        // A service, its installer and a tray build have no console: all want the log.
        tray_mode || service_run || service_command,
        &config_path,
    );
    if let Some(reason) = headless_reason {
        tracing::info!("{reason}");
    }

    // A serving start with no configuration file and nothing handed to it by the
    // environment is an installation nobody has set up yet, not a deployment to report
    // an error about: it serves the setup page, writes the collected secret and state
    // paths to the per-user configuration file, and hands the listener over so the
    // server starts on the socket the page was served on. Deliberately before the
    // relative-path resolution and secret checks below.
    let mut setup_listener: Option<server::restart::BoundListener> = None;
    if matches!(command, Command::Server)
        && !service_run
        && infra::config::needs_first_run_setup(&resolved_config)
    {
        tracing::info!(
            config = %config_path.display(),
            "no configuration file and no NANOFILE_* environment variable: starting the \
             first-run setup"
        );
        let defaults = server::setup::SetupDefaults::detect(&config);
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(server::setup::run(config_path.clone(), config, defaults))? {
            server::setup::SetupOutcome::Completed { listener, .. } => {
                setup_listener = listener;
                // Everything below reads the configuration the page just wrote.
                let (reloaded, _) = Config::load_for_start_with_path(Some(&config_path))?;
                config = reloaded.config;
                env_keys = reloaded.env_keys;
            }
            server::setup::SetupOutcome::Aborted => return Ok(()),
            server::setup::SetupOutcome::Expired => {
                // A GUI-subsystem build has no console, so the log file and this box are
                // the only routes to a person; a server has nobody to dismiss a box.
                logging::show_message_box(
                    "Nanofile",
                    "Nanofile could not be set up: the configuration page was not completed in \
                     time. Start Nanofile again to get a new setup page, and see nanofile.log \
                     for the address it was served on.",
                );
                anyhow::bail!(
                    "first-run setup was not completed; start the program again to retry"
                );
            }
        }
    }

    // State paths default to relative, and a login-started instance's working directory
    // has nothing to do with the installation (a Windows `Run` value cannot carry a start
    // directory), so `data/nanofile.db` would land wherever the process happened to start.
    let state_base = infra::config::state_path_base();
    let resolutions = config.resolve_state_paths(&state_base);
    let rewritten: Vec<&str> = resolutions
        .iter()
        .filter(|resolution| resolution.rewritten)
        .map(|resolution| resolution.field)
        .collect();
    if !rewritten.is_empty() {
        tracing::info!(
            base = %state_base.display(),
            fields = ?rewritten,
            "relative state paths resolved against the installation directory"
        );
    }
    let kept: Vec<&str> = resolutions
        .iter()
        .filter(|resolution| !resolution.rewritten)
        .map(|resolution| resolution.field)
        .collect();
    if !kept.is_empty() {
        tracing::info!(
            cwd = %std::env::current_dir().unwrap_or_default().display(),
            fields = ?kept,
            "relative state paths already present in the working directory kept as configured"
        );
    }

    // Release builds require an explicit, high-entropy secret: the storage master key,
    // the CSRF/notification JWT keys and the sync-token encryption key are all derived
    // from it, so an ephemeral secret makes encrypted data unreadable after a restart
    // and rotates sessions. Debug builds keep the zero-config behaviour; only
    // `nanofile server` needs it, so CLI subcommands are not blocked.
    let is_dev = cfg!(debug_assertions);
    let allow_ephemeral = std::env::var("NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let needs_secret = matches!(command, Command::Server) || service_run;

    if !secret_is_strong(&config.server.secret_key) {
        if is_dev || allow_ephemeral || !needs_secret {
            // An ephemeral key is regenerated on every start, which invalidates every key
            // derived from it. With at-rest block encryption that is irreversible data
            // loss; refuse rather than start a deployment that destroys its own data.
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

    // The encrypted-library version and password-hash algorithm are echoed to every client
    // and taken verbatim as the `enc_version` / KDF, so a bad value breaks all creation.
    if let Err(message) = config.server.validate_encrypted_library() {
        anyhow::bail!("{message}");
    }

    if config.notification.private_key.is_empty()
        || config.notification.private_key == "nanofile-notification-secret"
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"notify-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        config.notification.private_key = hex::encode(hasher.finalize());
    }

    // An explicitly configured notification signing key must also be strong: guessing
    // it lets an attacker mint subscription JWTs for any repository (and event JWTs,
    // when `accept_legacy_event_tokens` is on).
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
            // With the tray feature compiled in and a desktop session present, the tray
            // event loop owns the main thread and the server runs on background workers.
            #[cfg(feature = "tray")]
            if tray_mode {
                tray::run(config, env_keys, config_path, setup_listener);
            }

            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_server_flow(config, env_keys, None, setup_listener))
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
        Command::VerifyBlocks {
            dry_run: _,
            repair,
            repo,
            json,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let db = establish_connection(&config.database).await?;
                migration::Migrator::up(&db, None).await?;
                infra::common::util::ensure_private_dir(&config.storage.block_dir)?;
                let store = server::fs::core::block_verify::open_block_store(&config)?;
                let report = server::fs::core::block_verify::BlockVerify::run(
                    &db,
                    &store,
                    &config.storage.block_dir,
                    repo.as_deref(),
                    repair,
                )
                .await?;
                report.print(json);
                if report.has_findings() {
                    // Non-zero so a monitoring wrapper notices without parsing the report.
                    std::process::exit(1);
                }
                anyhow::Ok(())
            })
        }
        Command::VerifyDirOrder {
            dry_run: _,
            repair,
            repo,
            json,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let db = establish_connection(&config.database).await?;
                migration::Migrator::up(&db, None).await?;
                let report =
                    server::fs::core::dir_order::DirOrder::run(&db, repo.as_deref(), repair)
                        .await?;
                report.print(json);
                if report.has_findings() {
                    // Non-zero so a monitoring wrapper notices without parsing the report.
                    std::process::exit(1);
                }
                anyhow::Ok(())
            })
        }
        // `run` never returns on its own: the SCM owns this process from
        // `StartServiceCtrlDispatcherW`, and the server runs inside `ServiceMain`.
        #[cfg(target_os = "windows")]
        Command::Service { action } => {
            startup::service::run_cli(action, &config, &config_path, env_keys)
        }
        // Handled by the early return above, before any configuration is read.
        Command::ExtractWorker { .. } => Ok(()),
    }
}

/// DB setup plus server startup — shared by the headless, tray (`tray::run` spawns
/// this onto its background runtime) and Windows service paths.
///
/// An administrator restart from `/sysadmin/settings/` does *not* return: the server
/// is torn down through the normal shutdown and rebuilt from the settings table,
/// keeping the listening socket across the gap (see `server::restart`).
///
/// `listener` is the socket a completed first-run setup was already serving on:
/// handing it in keeps the port, which Windows refuses to rebind while the setup
/// page's connection sits in `TIME_WAIT`.
async fn run_server_flow(
    config: Config,
    env_keys: EnvKeys,
    mut tray_cmd: Option<TrayCmdReceiver>,
    mut listener: Option<server::restart::BoundListener>,
) -> anyhow::Result<()> {
    // Built once, outside the generation loop, for the same reason the listener is: an
    // in-place restart rebuilds `AppState`, and a run submitted before it must still
    // be pollable after it.
    let tasks = server::tasks::TaskSystem::new(
        server::tasks::TaskLimits::default(),
        tokio_util::sync::CancellationToken::new(),
    );
    let mut first = true;
    loop {
        // Every generation gets a new number, which `/health` reports and the
        // browser's "restarting" page waits to change.
        let generation = server::restart::next_generation();
        tracing::info!(generation, "starting a server generation");
        let db = establish_connection(&config.database).await?;
        migration::Migrator::up(&db, None).await?;
        let reason = run_server(
            db,
            config.clone(),
            env_keys.clone(),
            tray_cmd.as_mut(),
            &mut listener,
            first,
            &tasks,
        )
        .await?;
        if !reason.should_restart() {
            return Ok(());
        }
        tracing::info!("Restarting the server in place");
        first = false;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_server(
    db: DatabaseConnection,
    config: Config,
    env_keys: EnvKeys,
    mut tray_cmd: Option<&mut TrayCmdReceiver>,
    listener: &mut Option<server::restart::BoundListener>,
    first: bool,
    tasks: &Arc<server::tasks::TaskSystem>,
) -> anyhow::Result<server::restart::StopReason> {
    // Read the saved settings before anything is built: a value an administrator saved must
    // decide the bind address, the data directories, the caches and other startup captures.
    let settings_repo =
        server::repository::settings::DbSettingsRepository::new(Arc::new(db.clone()));
    let layers = server::settings::SettingsLayers::load(&settings_repo, &config, env_keys).await?;
    let cipher = infra::crypto::token_encryption::TokenCipher::from_master_key(
        config.server.secret_key.as_bytes(),
    );
    let (startup, broken_secrets) = server::settings::service::resolve_startup(&layers, &cipher);
    for key in &broken_secrets {
        tracing::warn!(
            key = %key,
            "a saved secret cannot be decrypted any more (the server secret_key changed); \
             re-enter it at /sysadmin/settings/"
        );
    }
    if let Err(e) = startup.validate() {
        tracing::warn!(
            "the effective configuration does not validate: {e}; the server will still start"
        );
    }
    let config = startup;

    // These read the layered config, not the file: a saved value is what the server runs
    // with, and an advisory about the other one is noise.

    // While `site_url` is unconfigured the request Host is echoed into
    // download/block URLs so LAN clients get a reachable address. That value is
    // client-supplied, so an attacker who can make a client use their hostname
    // (DNS rebinding, wildcard vhost) receives that client's capability token.
    if config.server.site_url_is_default() && config.server.allowed_hosts.is_empty() {
        if config.server.trust_request_host {
            tracing::warn!(
                "site_url is still the built-in default and server.allowed_hosts is empty: \
                 download/block URLs will echo a syntactically valid request Host header, \
                 which is attacker-influenced behind a wildcard vhost or a proxy that \
                 forwards arbitrary Host values. Set server.site_url to the address \
                 clients use (at /sysadmin/settings/ or in the file), restrict \
                 server.allowed_hosts, or disable the echo with \
                 server.trust_request_host = false."
            );
        } else {
            tracing::info!(
                "site_url is still the built-in default; download/block URLs will use it \
                 verbatim (server.trust_request_host = false)."
            );
        }
    }

    // `0` is not "use the default" for these two: on the API login path it means a token
    // that never expires, a permanent bearer credential; the web-UI path reads it opposite.
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

    // With GC off nothing reclaims blocks no commit references (an upload abandoned
    // before its final chunk, a sync whose branch update failed). Block writes are
    // still charged as uncommitted writes (`service::fs::quota::reserve_block_bytes`),
    // so quota bounds one user but total disk usage only grows.
    if !config.gc.enabled {
        tracing::warn!(
            "gc.enabled = false: blocks and FS objects that no commit references are \
             never reclaimed. Per-user quota still bounds how much one user may write, \
             but total disk usage only grows. Enable [gc] to reclaim them."
        );
    }

    tracing::info!(
        "starting nanofile server on {}:{}",
        config.server.addr,
        config.server.port
    );

    // Private data directories are created with owner-only permissions (the log
    // directory too, via `logging::init`), so the operator's umask does not decide.
    // Failures name the setting and the directory: under a service the only report is
    // a log line, and "Permission denied" without the path is not actionable.
    for (field, dir) in [
        ("storage.block_dir", &config.storage.block_dir),
        ("storage.temp_dir", &config.storage.temp_dir),
        ("storage.thumbnail_dir", &config.storage.thumbnail_dir),
        ("storage.avatar_dir", &config.storage.avatar_dir),
        ("index.index_dir", &config.index.index_dir),
    ] {
        infra::common::util::ensure_private_dir(dir).map_err(|e| {
            anyhow::anyhow!(
                "cannot create or write the {field} directory {} ({e}); the account this \
                 process runs as needs write access to it",
                dir.display()
            )
        })?;
    }

    // The legacy flat layout (`blocks/<2hex>/<id>`) has no read path in the running
    // server, so this must complete *before* anything serves: with the legacy tree
    // present every download would 404. A resumable copy that then removes it.
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

    // A writer killed mid-block leaves a `{block_id}.{uuid}.tmp` beside the block:
    // writes publish atomically, so it is never the block itself, but it would linger
    // forever. Swept once, before requests are served.
    {
        let store =
            infra::storage::block_store::BlockStorage::new(config.storage.block_dir.clone());
        match store.purge_layout_temp_files().await {
            Ok(0) => {}
            Ok(removed) => tracing::info!("removed {removed} stale block temp file(s)"),
            Err(e) => tracing::warn!("could not sweep stale block temp files: {e}"),
        }
    }

    // The DB half runs in the `purge_legacy_thumbnail_sizes` migration; the bytes on
    // disk can only go here, where the cache directory is known, and only once (a
    // marker file records it). Spawned rather than awaited: a cache file is regenerated.
    {
        let thumbnail_dir = config.storage.thumbnail_dir.clone();
        tokio::spawn(async move {
            match server::service::fs::thumbnail::purge_legacy_cache_files(&thumbnail_dir).await {
                // A no-op pass stays off the INFO log so a normal boot stays quiet.
                Ok(report) if report.ran => {
                    tracing::info!("thumbnail cache purge: {}", report.summary())
                }
                Ok(_) => tracing::debug!("thumbnail cache purge: already done, skipped"),
                Err(e) => tracing::warn!(
                    dir = %thumbnail_dir.display(),
                    "thumbnail cache purge failed; it will retry on the next start: {e}"
                ),
            }
        });
    }

    // Media thumbnail scratch files an earlier run could not remove: a process killed
    // mid-request does not delete its own. Swept before serving, so no request races one.
    {
        let removed =
            server::service::fs::thumbnail::purge_media_scratch(&config.storage.temp_dir).await;
        if removed > 0 {
            tracing::info!("removed {removed} leftover media thumbnail scratch files");
        }
    }

    let temp_file_manager = server::handler::web::temp_file::TempFileManager::new(
        config.storage.temp_dir.clone(),
        config.storage.max_temp_uploads,
        config.storage.max_temp_upload_bytes,
    )
    .await;

    let state = Arc::new(AppState::new_with_layers_and_tasks(
        db,
        layers,
        temp_file_manager,
        tasks.clone(),
    ));
    // Say what the layering decided: superseded values, pending restarts, unknown
    // rows, unreadable secrets.
    state.settings.log_startup_diagnostics();
    // Follow the settings table, so a change made on another instance reaches this
    // one without a restart.
    state.spawn_settings_refresh();

    // Rewrite legacy plaintext sync tokens as AEAD ciphertext: non-disruptive (clients keep
    // presenting the same raw token) and it must happen before the HTTP server serves.
    server::repository::sync_token::encrypt_legacy_sync_tokens(
        state.db.as_ref(),
        &state.token_cipher,
    )
    .await?;

    if let (Some(admin_email), Some(admin_password)) = (
        &state.config().admin_init.email,
        &state.config().admin_init.password,
    ) {
        let count = infra::entity::user::Entity::find()
            .count(state.db.as_ref())
            .await?;
        if count == 0 {
            tracing::info!("No users found; creating initial admin user");
            server::service::auth::bootstrap::create_first_admin(
                state.db.as_ref(),
                admin_email,
                admin_password,
                server::service::auth::bootstrap::AdminPolicy::from_config(&state.config()),
            )
            .await?;
            tracing::info!("Admin user '{}' created", admin_email);
        } else {
            tracing::debug!(
                "Users already exist (count={}), skipping admin auto-creation",
                count
            );
        }
    }

    let app = server::app::build_app(state.clone());

    // A restart keeps the original socket and hands this generation a duplicate, so
    // the port is never rebound (Windows refuses a bind while a drained connection
    // sits in TIME_WAIT). Only a *changed* address rebinds, and that path retries.
    let addr = format!("{}:{}", config.server.addr, config.server.port);
    match server::restart::BoundListener::acquire(listener, &addr, !first).await {
        Ok(()) => server::restart::clear_failure(),
        Err(e) => {
            let Some(current) = listener.as_ref() else {
                return Err(anyhow::anyhow!("failed to bind {addr}: {e}"));
            };
            // The restart cannot move to the saved address. Serving nothing would be worse
            // than serving the old one, so keep the socket that works and record why:
            // `/sysadmin/settings/` shows it, long after the RESTART response was sent.
            server::restart::note_failure(format!(
                "the restart could not bind {addr} ({e}); still listening on {}",
                current.addr()
            ));
        }
    }

    let bound = listener
        .as_ref()
        .expect("a listener is bound once `acquire` has succeeded");
    let bound_addr = bound.addr().to_string();
    let conn_listener = bound.to_tokio()?;
    tracing::info!("listening on {}", bound_addr);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // ConnectInfo exposes the TCP peer address to handlers so rate limiting can use
    // the real client IP instead of the spoofable X-Forwarded-For header.
    let header_read_timeout = match config.server.header_read_timeout_secs {
        0 => None,
        secs => Some(std::time::Duration::from_secs(secs)),
    };
    let server_handle = tokio::spawn(async move {
        server::serve::serve_with_timeouts(conn_listener, app, shutdown_rx, header_read_timeout)
            .await
    });

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
    // Tray and service control send on a tokio channel that is *reused* across restarts
    // (`recv()` borrows), so a restart cannot disconnect the menu's Quit item.
    let tray_quit = async {
        if let Some(rx) = tray_cmd.as_mut() {
            while let Some(cmd) = rx.recv().await {
                if matches!(cmd, TrayCommand::Quit) {
                    return;
                }
            }
        }
        std::future::pending::<()>().await;
    };
    let admin_restart = state.restart.wait();

    let reason = tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received SIGINT (Ctrl+C)");
            server::restart::StopReason::Shutdown
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM");
            server::restart::StopReason::Shutdown
        }
        _ = tray_quit => {
            tracing::info!("Quit requested from the tray");
            server::restart::StopReason::Shutdown
        }
        _ = admin_restart => {
            tracing::info!("Restart requested from the admin UI");
            server::restart::StopReason::Restart
        }
    };

    tracing::info!("Shutdown signal received, starting graceful shutdown...");

    let _ = shutdown_tx.send(());
    match tokio::time::timeout(std::time::Duration::from_secs(25), server_handle).await {
        Ok(Ok(Ok(()))) => tracing::info!("Server finished normally"),
        Ok(Ok(Err(e))) => tracing::error!("Server error: {e}"),
        Ok(Err(e)) => tracing::error!("Server task panicked: {e}"),
        Err(_) => tracing::warn!("Drain timed out after 25s, proceeding with cleanup"),
    }

    tracing::info!("Stopping background tasks...");
    state.shutdown_token.cancel();

    // Record anything this generation started that has not finished: otherwise a
    // client polling a task id submitted just before the restart gets a 404 from the
    // next generation. Bounded so a wedged job cannot hold the restart open.
    state.tasks.drain(std::time::Duration::from_secs(5)).await;

    if let Some(ref mgr) = state.notification_manager {
        mgr.shutdown().await;
    }

    if let Some(ref indexer) = state.indexer
        && let Err(e) = indexer.commit()
    {
        tracing::error!("Failed to commit indexer during shutdown: {e}");
    }

    // DB connection is dropped when `state` goes out of scope;
    // the OS handles final file descriptor cleanup.

    tracing::info!("Server shutdown complete");

    Ok(reason)
}

/// Read a password from the first line of `reader`, trimmed like
/// `NANOFILE_ADMIN_INIT_PASSWORD_FILE`, so a trailing newline or CRLF never gets in.
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
        // A command-line password is readable by every local user through `ps` and
        // lands in the shell history; keep it working for scripts that already rely
        // on it, but steer the operator elsewhere.
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
        email_verified: Set(true),
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
        // Derived (empty) and legacy placeholder values are replaced at startup by
        // a 256-bit derived key, so they are not reportable here.
        assert!(!notification_key_is_weak(""));
        assert!(!notification_key_is_weak("nanofile-notification-secret"));
        assert!(!notification_key_is_weak(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!notification_key_is_weak(
            "a-32-byte-plus-raw-secret-value!!!"
        ));
        assert!(notification_key_is_weak("abc"));
        assert!(notification_key_is_weak("nanofile-notify"));
        assert!(notification_key_is_weak(&"z".repeat(64)));
    }
}
