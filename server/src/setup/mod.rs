//! First-run setup: what a start with no configuration at all serves.
//!
//! A zero-config start serves a page that creates the first administrator, generates
//! the master secret, decides where state lives, and writes `~/.nanofile/config.toml`
//! once; every later start reads that file and never comes back here.
//!
//! Invariants: it runs before the server exists, so the file it writes is the one the
//! *next* line of `main` loads; the completed setup hands the exact [`BoundListener`]
//! over rather than releasing and rebinding it, which Windows refuses while a drained
//! connection sits in `TIME_WAIT`; and the URL's 128-bit token is printed and logged
//! but never revealed at a bare `/`.

pub mod page;
pub mod plan;

pub use plan::{SetupDefaults, SetupError, SetupForm, SetupPlan};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use askama::Template;
use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use sea_orm::{EntityTrait, PaginatorTrait};
use sea_orm_migration::MigratorTrait;
use subtle::ConstantTimeEq;

use base::error::AppError;
use infra::config::{Config, WriteConfigError, write_new_config};

use crate::i18n::I18n;
use crate::restart::BoundListener;
use plan::CONFIG_HEADER;

/// How long a setup page may sit unanswered. Bounded: a GUI build has no window to close.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a completed setup waits for the page to flush before the listener closes.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct SetupResult {
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub site_url: String,
    /// The address the submitted settings name, compared with the socket serving the page.
    pub address: String,
    /// Whether this page's socket is the server's; refresh is relative only when it is.
    pub handover: bool,
    /// The administrator this setup created, when the database had no account yet.
    pub created_email: Option<String>,
    /// Accounts the database already had: a configuration file deleted, its data kept.
    pub accounts_kept: u64,
}

pub enum SetupOutcome {
    /// The page was completed and the configuration is on disk.
    Completed {
        result: SetupResult,
        /// The socket the page was served on when it is the server's: `None` otherwise.
        listener: Option<BoundListener>,
    },
    /// Ctrl+C, SIGTERM or a service stop: nothing was written.
    Aborted,
    /// Nobody completed the page within `SETUP_TIMEOUT`.
    Expired,
}

pub struct SetupState {
    token: String,
    config_path: PathBuf,
    defaults: SetupDefaults,
    config: Config,
    outcome: tokio::sync::Mutex<Option<SetupResult>>,
    /// Held for a submit, so two tabs cannot both create an administrator.
    submitting: tokio::sync::Mutex<()>,
    done: tokio::sync::Notify,
}

impl SetupState {
    /// Build the state a [`setup_router`] serves; public so a test can drive the page alone.
    pub fn new(
        token: String,
        config_path: PathBuf,
        config: Config,
        defaults: SetupDefaults,
    ) -> Arc<Self> {
        Arc::new(Self {
            token,
            config_path,
            defaults,
            config,
            outcome: tokio::sync::Mutex::new(None),
            submitting: tokio::sync::Mutex::new(()),
            done: tokio::sync::Notify::new(),
        })
    }

    pub async fn result(&self) -> Option<SetupResult> {
        self.outcome.lock().await.clone()
    }

    /// Resolve once a submit has finished the setup. `notify_one` rather than
    /// `notify_waiters` keeps a permit when nobody waits yet, so nothing is missed.
    pub async fn wait_for_result(&self) -> SetupResult {
        loop {
            if let Some(result) = self.result().await {
                return result;
            }
            self.done.notified().await;
        }
    }
}

/// Run the setup phase: serve the page, wait for it to be completed, and report it.
pub async fn run(
    config_path: PathBuf,
    config: Config,
    mut defaults: SetupDefaults,
) -> anyhow::Result<SetupOutcome> {
    let token = random_token();
    let address = format!("{}:{}", defaults.addr.trim(), defaults.port);

    // Preferred: the socket the server will use, so the setup hands it over, never a rebind.
    let mut held: Option<BoundListener> = None;
    let mut preferred: Option<tokio::net::TcpListener> = None;
    match BoundListener::acquire(&mut held, &address, false).await {
        Ok(()) => {
            let bound = held
                .as_ref()
                .expect("a successful acquire stores a listener");
            match bound.to_tokio() {
                Ok(listener) => preferred = Some(listener),
                Err(e) => tracing::warn!(
                    address = %address,
                    "cannot serve the setup page on the configured socket ({e}); \
                     falling back to a random loopback port"
                ),
            }
        }
        Err(e) => tracing::warn!(
            address = %address,
            "the configured address cannot be bound ({e}); the setup page will be served \
             on a random loopback port"
        ),
    }

    let (listener, fallback) = match preferred {
        Some(listener) => (listener, false),
        None => {
            // Somebody else holds the configured address: the page is served elsewhere.
            held = None;
            (tokio::net::TcpListener::bind("127.0.0.1:0").await?, true)
        }
    };
    // The socket serving the page counts as available to the submit though a bind fails.
    let bound_addr = listener.local_addr()?;
    defaults.held_address = Some(bound_addr.to_string());
    defaults.port_in_use = fallback;
    defaults.existing_accounts = probe_existing_accounts(&defaults.data_dir).await;

    let port = bound_addr.port();
    let host = match bound_addr.ip() {
        // A wildcard bind is reachable on the loopback of its own family.
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let state = SetupState::new(token.clone(), config_path.clone(), config, defaults);
    let url = format!("http://{host}:{port}/setup/{token}");

    // The URL is a one-time credential, so only the console and the log file see it.
    tracing::info!(%url, config = %config_path.display(), "first-run setup: open this page to create the administrator");
    println!("[nanofile] first-run setup: {url}");
    open_browser(&url);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let app = setup_router(state.clone());
    let serve = crate::serve::serve_with_timeouts(listener, app, shutdown_rx, None);
    let mut serve = std::pin::pin!(serve);

    enum Stop {
        Completed(SetupResult),
        Aborted,
        Expired,
    }

    let stop = tokio::select! {
        // The serving loop returns only when the listener closes, the shutdown it awaits.
        result = &mut serve => {
            result?;
            tracing::warn!("the setup listener stopped before the setup was completed");
            Stop::Aborted
        }
        result = state.wait_for_result() => Stop::Completed(result),
        _ = tokio::signal::ctrl_c() => Stop::Aborted,
        _ = terminate_signal() => Stop::Aborted,
        _ = tokio::time::sleep(SETUP_TIMEOUT) => Stop::Expired,
    };

    // Stop accepting, let the served response reach the browser, then release the socket.
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, serve).await;

    match stop {
        Stop::Completed(result) => {
            // Only a socket on the operator's address is handed over, never the fallback.
            let listener = held.filter(|_| result.handover);
            tracing::info!(
                config = %result.config_path.display(),
                address = %result.address,
                socket_handed_over = listener.is_some(),
                "first-run setup completed"
            );
            Ok(SetupOutcome::Completed { result, listener })
        }
        Stop::Aborted => {
            tracing::info!("first-run setup was stopped before it completed; nothing was written");
            Ok(SetupOutcome::Aborted)
        }
        Stop::Expired => {
            let minutes = SETUP_TIMEOUT.as_secs() / 60;
            tracing::error!(
                "first-run setup was not completed within {minutes} minutes; exiting. \
                 Run the program again, or open the page at {url} while it is running"
            );
            Ok(SetupOutcome::Expired)
        }
    }
}

/// The setup page as a router; public so the integration tests can drive it without a browser.
pub fn setup_router(state: Arc<SetupState>) -> Router {
    Router::new()
        // A healthcheck must not declare the process dead while it waits for a person.
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/", get(notice_page))
        .route("/setup/{token}", get(form_page).post(submit))
        .route("/setup/{token}/", get(form_page).post(submit))
        .route("/static/{*path}", get(crate::static_assets::serve_static))
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(state)
}

/// The headers every page carries, minus HSTS: this listener speaks no TLS of its own.
async fn security_headers(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(req).await;
    crate::middleware::apply_security_headers(response.headers_mut(), false, false);
    response
}

/// The page for anybody who reached the port without the token.
async fn notice_page(State(state): State<Arc<SetupState>>, headers: HeaderMap) -> Response {
    let t = I18n::from_headers(&headers, &state.defaults.language);
    notice(
        t,
        "setup.in_progress_title",
        "setup.in_progress_body",
        &state.config_path.display().to_string(),
    )
}

async fn form_page(
    State(state): State<Arc<SetupState>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if !token_matches(&state.token, &token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(result) = state.result().await {
        return done_page(&state, &result, &headers);
    }
    render_form(
        &state,
        &headers,
        &plan::SetupForm {
            addr: state.defaults.addr.clone(),
            port: state.defaults.port.to_string(),
            site_url: state.defaults.site_url.clone(),
            data_dir: state.defaults.data_dir.clone(),
            encryption: state.defaults.encryption.clone(),
            ..Default::default()
        },
        None,
    )
}

async fn submit(
    State(state): State<Arc<SetupState>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<plan::SetupForm>,
) -> Response {
    if !token_matches(&state.token, &token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // One submit at a time: a reload racing the first response must find the result.
    let _submitting = state.submitting.lock().await;

    // Idempotent: a reload or second tab must not create a second administrator.
    if let Some(result) = state.result().await {
        return done_page(&state, &result, &headers);
    }
    if state.config_path.exists() {
        // The file is the record that setup is done, so it is never overwritten.
        let t = I18n::from_headers(&headers, &state.defaults.language);
        return (
            StatusCode::CONFLICT,
            notice(
                t,
                "setup.already_title",
                "setup.already_body",
                &state.config_path.display().to_string(),
            ),
        )
            .into_response();
    }

    let plan = match form.into_plan(&state.defaults) {
        Ok(plan) => plan,
        Err(error) => {
            let t = I18n::from_headers(&headers, &state.defaults.language);
            return render_form(&state, &headers, &form, Some(t.tr(error.key).to_string()));
        }
    };

    match execute(&state, &plan).await {
        Ok(result) => {
            *state.outcome.lock().await = Some(result.clone());
            // Stored before the notification, so a waiter waking here finds it.
            state.done.notify_one();
            done_page(&state, &result, &headers)
        }
        Err(SetupFailure::AlreadyConfigured) => {
            let t = I18n::from_headers(&headers, &state.defaults.language);
            (
                StatusCode::CONFLICT,
                notice(
                    t,
                    "setup.already_title",
                    "setup.already_body",
                    &state.config_path.display().to_string(),
                ),
            )
                .into_response()
        }
        Err(SetupFailure::Failed(error)) => {
            tracing::error!("first-run setup failed: {error:#}");
            let t = I18n::from_headers(&headers, &state.defaults.language);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                notice(
                    t,
                    "setup.failed_title",
                    "setup.failed_body",
                    &format!("{error:#}"),
                ),
            )
                .into_response()
        }
    }
}

enum SetupFailure {
    /// The configuration file appeared while the form was open.
    AlreadyConfigured,
    Failed(anyhow::Error),
}

/// Create what the plan describes, then write it down. Directories, database and account
/// exist *before* the configuration file: the reverse order leaves no account behind.
async fn execute(state: &SetupState, plan: &SetupPlan) -> Result<SetupResult, SetupFailure> {
    let mut config = state.config.clone();
    plan.apply_to(&mut config);

    for (field, dir) in plan.directories() {
        infra::common::util::ensure_private_dir(&dir).map_err(|e| {
            SetupFailure::Failed(anyhow::anyhow!(
                "cannot create the {field} directory {} ({e})",
                dir.display()
            ))
        })?;
    }

    let db = infra::db::establish_connection(&config.database)
        .await
        .map_err(SetupFailure::Failed)?;
    migration::Migrator::up(&db, None)
        .await
        .map_err(|e| SetupFailure::Failed(anyhow::anyhow!("database migration failed: {e}")))?;

    let accounts = infra::entity::user::Entity::find()
        .count(&db)
        .await
        .map_err(|e| SetupFailure::Failed(anyhow::anyhow!("cannot count accounts: {e}")))?;
    let created_email = if accounts == 0 {
        let created = crate::service::auth::bootstrap::create_first_admin(
            &db,
            &plan.email,
            &plan.password,
            crate::service::auth::bootstrap::AdminPolicy {
                password_min_length: state.defaults.password_min_length,
                require_strong_password: state.defaults.require_strong_password,
                password_hash_iterations: config.auth.password_hash_iterations,
            },
        )
        .await
        .map_err(|e: AppError| {
            SetupFailure::Failed(anyhow::anyhow!("cannot create the administrator: {e}"))
        })?;
        tracing::info!(id = created, email = %plan.email, "first administrator created");
        Some(plan.email.clone())
    } else {
        tracing::info!(
            accounts,
            "the database already holds accounts; the setup keeps them and writes the \
             configuration only"
        );
        None
    };

    let sections = plan.sections();
    write_new_config(&state.config_path, CONFIG_HEADER, &sections).map_err(|e| match e {
        WriteConfigError::AlreadyExists(_) => SetupFailure::AlreadyConfigured,
        WriteConfigError::Other(e) => SetupFailure::Failed(e),
    })?;

    Ok(SetupResult {
        config_path: state.config_path.clone(),
        data_dir: plan.data_dir.clone(),
        site_url: plan.site_url.clone(),
        address: plan.listener_address(),
        // Handed over only when it is the socket the operator asked the server to use.
        handover: state.defaults.held_address.as_deref() == Some(plan.listener_address().as_str()),
        created_email,
        accounts_kept: accounts,
    })
}

/// Accounts a database already in `data_dir` holds; read-only, leaving no empty database behind.
async fn probe_existing_accounts(data_dir: &str) -> Option<u64> {
    let file = Path::new(data_dir).join("nanofile.db");
    if !file.is_file() {
        return None;
    }
    let database = infra::config::DatabaseConfig {
        url: format!("sqlite:{}?mode=rwc", file.display()),
        ..Default::default()
    };
    let db = infra::db::establish_connection(&database).await.ok()?;
    infra::entity::user::Entity::find().count(&db).await.ok()
}

fn render_form(
    state: &SetupState,
    headers: &HeaderMap,
    form: &plan::SetupForm,
    error: Option<String>,
) -> Response {
    let t = I18n::from_headers(headers, &state.defaults.language);
    let template = page::SetupTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        token: state.token.clone(),
        email: form.email.clone(),
        addr: form.addr.clone(),
        port: form.port.clone(),
        site_url: form.site_url.clone(),
        data_dir: form.data_dir.clone(),
        encryption: form.encryption.clone(),
        config_path: state.config_path.display().to_string(),
        port_in_use: state.defaults.port_in_use,
        existing_accounts: state.defaults.existing_accounts,
        password_hint: password_hint(t, &state.defaults),
        error: error.unwrap_or_default(),
    };
    let status = if template.error.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    match template.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(e) => page_failure("cannot render the setup page", &e.to_string()),
    }
}

/// The password rule the form enforces, said out loud: rejecting silently makes them guess.
fn password_hint(t: &'static I18n, defaults: &SetupDefaults) -> String {
    if defaults.require_strong_password {
        t.tr("setup.password_strong_hint").to_string()
    } else {
        t.tr("setup.password_min_hint")
            .replace("{min}", &defaults.password_min_length.to_string())
    }
}

fn done_page(state: &SetupState, result: &SetupResult, headers: &HeaderMap) -> Response {
    let t = I18n::from_headers(headers, &state.defaults.language);
    let template = page::SetupDoneTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        site_url: result.site_url.clone(),
        config_path: result.config_path.display().to_string(),
        data_dir: result.data_dir.display().to_string(),
        handover: result.handover,
        created_email: result.created_email.clone(),
        accounts_kept: result.accounts_kept,
    };
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => page_failure("cannot render the setup page", &e.to_string()),
    }
}

fn notice(
    t: &'static I18n,
    heading_key: &'static str,
    body_key: &'static str,
    detail: &str,
) -> Response {
    let template = page::SetupNoticeTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        heading_key,
        body_key,
        detail: detail.to_string(),
    };
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => page_failure("cannot render the setup page", &e.to_string()),
    }
}

/// A failure of the page itself; this router has no error middleware to catch it.
fn page_failure(context: &str, detail: &str) -> Response {
    tracing::error!("{context}: {detail}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "The setup page could not be rendered. See the log.",
    )
        .into_response()
}

/// 128 bits of randomness, hex-encoded.
fn random_token() -> String {
    use rand::Rng;

    let mut token = [0u8; 16];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

/// Constant-time comparison, so the token cannot be recovered from response timings.
fn token_matches(expected: &str, presented: &str) -> bool {
    expected.as_bytes().ct_eq(presented.as_bytes()).into()
}

/// Open the setup page in a browser, when this build has one; best effort otherwise.
#[cfg(feature = "tray")]
fn open_browser(url: &str) {
    if !crate::desktop::has_desktop_session() {
        return;
    }
    if let Err(e) = open::that(url) {
        tracing::warn!("could not open a browser at {url}: {e}");
    }
}

#[cfg(not(feature = "tray"))]
fn open_browser(url: &str) {
    let _ = url;
}

async fn terminate_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::warn!("cannot install the SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_the_only_key_and_compared_exactly() {
        let token = random_token();
        assert_eq!(token.len(), 32, "16 bytes, hex-encoded");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(random_token(), token, "a fresh token every start");

        assert!(token_matches(&token, &token));
        assert!(!token_matches(&token, ""));
        assert!(!token_matches(&token, &token[..token.len() - 1]));
        assert!(!token_matches(&token, &format!("{token}0")));
        assert!(!token_matches(&token, &token.to_uppercase()));
    }

    #[tokio::test]
    async fn a_notified_waiter_never_misses_a_result_stored_first() {
        let state = SetupState::new(
            random_token(),
            PathBuf::from("/tmp/nanofile-test/config.toml"),
            Config::default(),
            SetupDefaults::detect(&Config::default()),
        );
        let result = SetupResult {
            config_path: PathBuf::from("/tmp/nanofile-test/config.toml"),
            data_dir: PathBuf::from("/tmp/nanofile-test/data"),
            site_url: "http://127.0.0.1:8082".into(),
            address: "0.0.0.0:8082".into(),
            handover: true,
            created_email: Some("admin@example.com".into()),
            accounts_kept: 0,
        };

        // Stored, then notified: the wait that starts *after* both still returns.
        *state.outcome.lock().await = Some(result.clone());
        state.done.notify_one();
        let seen = state.wait_for_result().await;
        assert_eq!(seen.address, result.address);
        assert_eq!(seen.created_email.as_deref(), Some("admin@example.com"));
    }
}
