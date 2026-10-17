//! First-run setup: what a start with no configuration at all serves.
//!
//! A zero-config start has nothing to report — there is no configuration file to
//! complain about and no environment variable to name — so it serves a page
//! instead: one that creates the first administrator, generates the master
//! secret, decides where state lives, and writes `~/.nanofile/config.toml` once.
//! Every later start reads that file and never comes back here.
//!
//! Three properties shape the implementation:
//!
//! * **It runs before the server exists.** The setup page is its own small
//!   router on its own listener; no `AppState`, no database-backed route and no
//!   middleware that assumes a signed-in session is involved. The configuration
//!   the page writes is therefore the configuration the *next* line of `main`
//!   loads, and nothing has to be reloaded in place.
//! * **The socket is handed over, not released.** When the page is served on the
//!   address the server will use, the completed setup gives that exact
//!   [`BoundListener`] to the server flow. Releasing the port and rebinding it
//!   moments later is what Windows refuses while a drained connection sits in
//!   `TIME_WAIT` — the browser that just finished the form is exactly such a
//!   connection.
//! * **The token is the credential.** The page URL carries a 128-bit token that
//!   is printed to stdout and to the log, and auto-opened in a browser when
//!   there is a desktop. A bare `/` never reveals it, so a port that happens to
//!   be reachable from elsewhere cannot be used to claim the installation.

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

/// How long a setup page may sit unanswered before the process gives up.
///
/// Bounded rather than indefinite: a double-clicked GUI build has no window a
/// person can close, so a page nobody ever finishes would leave a process
/// running with no visible trace of why. Running the program again starts a
/// fresh setup page.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a completed setup waits for the page it just served to be flushed
/// before the listener is torn down.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// What a completed setup created, as the page reports it.
#[derive(Debug, Clone)]
pub struct SetupResult {
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub site_url: String,
    /// The address the submitted settings name. The runner compares it with the
    /// socket the page was served on to decide whether that socket can be
    /// handed over.
    pub address: String,
    /// Whether the socket this page was served on is the one the server will
    /// use. The done page's automatic refresh is relative only when it is: a
    /// fallback socket is closed before the server starts.
    pub handover: bool,
    /// The administrator this setup created, when the database had no account
    /// yet.
    pub created_email: Option<String>,
    /// Accounts the database already had — a configuration file that was
    /// deleted while its data stayed behind.
    pub accounts_kept: u64,
}

/// Why the setup phase ended.
pub enum SetupOutcome {
    /// The page was completed and the configuration is on disk.
    Completed {
        result: SetupResult,
        /// The socket the page was served on, when it is the one the server
        /// will use (`None` when the submitted address differs, or when the
        /// configured one was busy and setup fell back to a random port).
        listener: Option<BoundListener>,
    },
    /// Ctrl+C, SIGTERM or a service stop: nothing was written.
    Aborted,
    /// Nobody completed the page within [`SETUP_TIMEOUT`].
    Expired,
}

/// The page's state: what it was built with, and what it produced.
pub struct SetupState {
    token: String,
    config_path: PathBuf,
    defaults: SetupDefaults,
    config: Config,
    outcome: tokio::sync::Mutex<Option<SetupResult>>,
    /// Held for the duration of a submit, so two tabs (or a reload racing the
    /// first response) cannot both create an administrator.
    submitting: tokio::sync::Mutex<()>,
    done: tokio::sync::Notify,
}

impl SetupState {
    /// Build the state a [`setup_router`] serves.
    ///
    /// Public so a test (or an embedding host) can drive the page without going
    /// through [`run`], which owns a real listener and a real timeout.
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

    /// What a completed submit produced, if one has completed.
    pub async fn result(&self) -> Option<SetupResult> {
        self.outcome.lock().await.clone()
    }

    /// Resolve once a submit has finished the setup.
    ///
    /// `notify_one` rather than `notify_waiters`: it stores a permit when nobody
    /// is waiting yet, so a completion that lands between the check below and
    /// the await cannot be missed.
    pub async fn wait_for_result(&self) -> SetupResult {
        loop {
            if let Some(result) = self.result().await {
                return result;
            }
            self.done.notified().await;
        }
    }
}

/// Run the setup phase: serve the page on the best socket available, wait for it
/// to be completed (or for the process to be asked to stop), and report what
/// happened.
pub async fn run(
    config_path: PathBuf,
    config: Config,
    mut defaults: SetupDefaults,
) -> anyhow::Result<SetupOutcome> {
    let token = random_token();
    let address = format!("{}:{}", defaults.addr.trim(), defaults.port);

    // Preferred: the very socket the server will use. When it works, the
    // completed setup hands it over instead of releasing the port and racing
    // `TIME_WAIT`.
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
            // Somebody else holds the configured address. The page is then served
            // somewhere reachable *now*, and the form says the prefilled port is
            // taken instead of accepting it and failing to start.
            held = None;
            (tokio::net::TcpListener::bind("127.0.0.1:0").await?, true)
        }
    };
    // Whatever socket is actually serving the page is the one the submit is
    // allowed to name: its address counts as available even though a second
    // bind of it would fail. Set for the fallback too, so a form that names the
    // address the page is served on is not rejected by its own listener.
    let bound_addr = listener.local_addr()?;
    defaults.held_address = Some(bound_addr.to_string());
    defaults.port_in_use = fallback;
    defaults.existing_accounts = probe_existing_accounts(&defaults.data_dir).await;

    let port = bound_addr.port();
    let host = match bound_addr.ip() {
        // A wildcard bind is reachable on the loopback of its own family, and
        // that is the address the person at the machine types.
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let state = SetupState::new(token.clone(), config_path.clone(), config, defaults);
    let url = format!("http://{host}:{port}/setup/{token}");

    // The URL is a one-time credential, so it belongs where the operator can see
    // it and nowhere else: the console (or the container log, which is the only
    // console a container has) and the log file.
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
        // The serving loop only returns when the listener is closed, which is
        // the shutdown this same future was told to wait for.
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

    // Stop accepting, let the response the handler already produced reach the
    // browser, and only then give the socket up (or keep it, below).
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, serve).await;

    match stop {
        Stop::Completed(result) => {
            // Only a socket on the address the operator chose may be handed to
            // the server: a different one would make it listen somewhere else,
            // and the fallback socket is not even on the configured interface.
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

/// The setup page as a router.
///
/// Public so the integration tests can drive the page and its side effects
/// without a browser.
pub fn setup_router(state: Arc<SetupState>) -> Router {
    Router::new()
        // A container healthcheck (or any supervisor) must not declare the
        // process dead while it waits for a person.
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route("/", get(notice_page))
        .route("/setup/{token}", get(form_page).post(submit))
        .route("/setup/{token}/", get(form_page).post(submit))
        .route("/static/{*path}", get(crate::static_assets::serve_static))
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(state)
}

/// The same security headers every page carries, minus the ones that would be a
/// lie here: the setup listener speaks no TLS of its own, so no HSTS is claimed.
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
    // One submit at a time: a reload that races the first response must find
    // the result rather than start a second first run.
    let _submitting = state.submitting.lock().await;

    // Idempotent: a reload of the done page (or a second tab) must not create a
    // second administrator or write a second file.
    if let Some(result) = state.result().await {
        return done_page(&state, &result, &headers);
    }
    if state.config_path.exists() {
        // Another instance — or the operator — got there first. The file is the
        // record that setup is done, so it is never overwritten.
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
            // Stored before the notification: a waiter that wakes up here must
            // find the result already in place.
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

/// Why a completed form could not be turned into an installation.
enum SetupFailure {
    /// The configuration file appeared while the form was open.
    AlreadyConfigured,
    Failed(anyhow::Error),
}

/// Create what the plan describes, then write it down.
///
/// Order matters: the directories, the database and the account exist *before*
/// the configuration file does. The other order would leave a file that makes
/// every later start skip the setup while the installation it describes has no
/// account to sign in with — and the only way back would be `nanofile adduser`.
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
        // The socket can only be handed over when it is the one the operator
        // asked the server to listen on; `run` applies the same condition.
        handover: state.defaults.held_address.as_deref() == Some(plan.listener_address().as_str()),
        created_email,
        accounts_kept: accounts,
    })
}

/// Accounts a database that already exists in `data_dir` holds.
///
/// A hint on the page, and deliberately read-only: nothing is created here, so a
/// setup that is abandoned leaves no empty database behind.
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

/// The password rule the form will enforce, said out loud.
///
/// A form that rejected a password without ever saying what it wanted would make
/// the operator guess at the one thing this page exists to collect.
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

/// A failure of the page itself. There is no error middleware on this router
/// (there is no `AppState` for one to read), so this is the whole of it.
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

/// Constant-time comparison, so the token cannot be recovered byte by byte from
/// response timings.
fn token_matches(expected: &str, presented: &str) -> bool {
    expected.as_bytes().ct_eq(presented.as_bytes()).into()
}

/// Open the setup page in a browser, when this build has one to open it in.
///
/// Best effort: on a machine with no browser (a server, a container) the URL in
/// the log is the whole interface, and the page stays reachable until the setup
/// completes.
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

/// Resolves on SIGTERM where the platform has one.
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

        // Stored, then notified -- and the wait that starts *after* both still
        // returns, because the permit is kept.
        *state.outcome.lock().await = Some(result.clone());
        state.done.notify_one();
        let seen = state.wait_for_result().await;
        assert_eq!(seen.address, result.address);
        assert_eq!(seen.created_email.as_deref(), Some("admin@example.com"));
    }
}
