//! Shared password gate for the anonymous link pages (`/f/`, `/d/`, `/u/`).
//!
//! The three pages used to carry their own copies of "direction switch → resolve
//! the link → check the password header/cookie → count failures → render the
//! validation page", and the copies had drifted: the POST paths verified a
//! passwordless link against an empty hash while the GET paths opened it. The
//! gate is the one place that decides whether a link is open and what a locked
//! visitor sees.

use askama::Template;
use axum::{http::HeaderMap, response::Html, response::IntoResponse, response::Response};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use base::error::AppError;
use infra::entity::{share_link, upload_link};

/// Which page owns the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkKind {
    ShareFile,
    ShareDir,
    Upload,
}

impl LinkKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::ShareFile => "/f",
            Self::ShareDir => "/d",
            Self::Upload => "/u",
        }
    }

    fn form_action(self, token: &str) -> String {
        format!("{}/{}/", self.prefix(), token)
    }

    fn redirect_target(self, token: &str) -> String {
        format!("{}/{}/", self.prefix(), token)
    }

    fn cookie_name(self, token: &str) -> String {
        match self {
            Self::ShareFile | Self::ShareDir => {
                crate::service::auth::csrf::share_link_cookie_name(token)
            }
            Self::Upload => crate::service::auth::csrf::upload_link_cookie_name(token),
        }
    }

    fn enabled(self, state: &AppState) -> bool {
        match self {
            Self::ShareFile | Self::ShareDir => state.config().server.share_download_enabled,
            Self::Upload => state.config().server.share_upload_enabled,
        }
    }

    fn share_model(self) -> bool {
        matches!(self, Self::ShareFile | Self::ShareDir)
    }
}

/// The link behind a gate. The model kinds stay separate because the pages read
/// different fields; the gate only needs token/password/repo.
pub(crate) enum LinkModel {
    Share(share_link::Model),
    Upload(upload_link::Model),
}

impl LinkModel {
    fn password(&self) -> Option<&str> {
        match self {
            Self::Share(link) => link.password.as_deref(),
            Self::Upload(link) => link.password.as_deref(),
        }
    }

    fn token(&self) -> &str {
        match self {
            Self::Share(link) => &link.token,
            Self::Upload(link) => &link.token,
        }
    }
}

/// The outcome of a GET gate check.
pub(crate) enum Gate {
    /// The link is open. `unlock_cookie` is set when the visitor supplied the
    /// password through the header: the upload page hands it back as a cookie so
    /// the follow-up upload-URL call is granted.
    Open {
        link: LinkModel,
        unlock_cookie: Option<String>,
    },
    /// The link is password protected and the visitor must submit the form.
    Password(Response),
}

/// Whether a locked link should answer with the HTML form or with the raw HTTP
/// error the file endpoint has always used.
#[derive(Clone, Copy)]
pub(crate) enum GateMode {
    Page,
    Raw,
}

/// The validation form, shared by all three link kinds.
#[derive(Template)]
#[template(path = "web/share_access_validation.html")]
struct ShareAccessValidationTemplate {
    t: &'static I18n,
    urls: &'static crate::static_assets::TemplateUrls,
    error: Option<String>,
    form_action: String,
}

/// Turn a link lookup failure into the page the visitor sees.
///
/// "Turned off" and "expired" read differently to the link holder, while any
/// other lookup failure stays a 500 rather than being dressed up as a dead link.
fn link_error_page(
    state: &AppState,
    err: AppError,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    match crate::service::sharing::share::classify_link_failure(&err) {
        Some(failure) => Ok(crate::ui::error_page::link_page(failure, state, headers)),
        None => Err(err),
    }
}

/// The page a visitor gets when the link's direction has been switched off.
///
/// "Gone", not "forbidden": a retired feature must not advertise that the link
/// ever existed, and the existing dead-link copy already says the owner may have
/// turned it off.
pub(crate) fn switched_off_link_page(state: &AppState, headers: &HeaderMap) -> Response {
    crate::ui::error_page::link_page(
        crate::service::sharing::share::LinkFailure::Unknown,
        state,
        headers,
    )
}

/// Rate-limit failed link-password attempts, keyed by the link token rather than
/// client IP: an IP-based limiter is bypassed by spreading attempts across many
/// addresses, while a per-token cap bounds brute force per link.
pub(crate) fn record_link_password_failure(
    state: &Arc<AppState>,
    token: &str,
) -> Result<(), AppError> {
    let key = format!("link_password_token:{token}");
    if !state.auth_limiters.link_password.check_and_record(&key) {
        return Err(AppError::TooManyRequests);
    }
    Ok(())
}

async fn resolve_share_link(
    state: &Arc<AppState>,
    token: &str,
) -> Result<share_link::Model, AppError> {
    crate::service::sharing::share::resolve_share_link(&state.repos, token).await
}

/// Resolve an upload link: exists, not expired, and its library still exists.
///
/// The creator's access is checked by the caller: the page view needs read
/// access to list the folder, while minting an upload URL needs write access.
pub(crate) async fn resolve_upload_link(
    state: &Arc<AppState>,
    token: &str,
) -> Result<upload_link::Model, AppError> {
    use crate::service::sharing::share::{UPLOAD_LINK_EXPIRED, UPLOAD_LINK_NOT_FOUND};

    let link = state
        .repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound(UPLOAD_LINK_NOT_FOUND.into()))?;

    if let Some(exp) = link.expires_at
        && chrono::Utc::now().timestamp() > exp
    {
        return Err(AppError::NotFound(UPLOAD_LINK_EXPIRED.into()));
    }

    if !state.sync_service().repo_exists(&link.repo_id).await? {
        return Err(AppError::NotFound(UPLOAD_LINK_NOT_FOUND.into()));
    }

    Ok(link)
}

async fn resolve_link(
    state: &Arc<AppState>,
    token: &str,
    kind: LinkKind,
) -> Result<LinkModel, AppError> {
    if kind.share_model() {
        Ok(LinkModel::Share(resolve_share_link(state, token).await?))
    } else {
        let link = resolve_upload_link(state, token).await?;
        // The link acts as its creator, so it must stop resolving once the
        // creator loses access to the library (member removed, account
        // deactivated); the view only needs read access.
        if !crate::service::sharing::share::link_creator_may_access(
            &state.repos,
            link.creator_id,
            &link.repo_id,
            false,
        )
        .await
        {
            return Err(AppError::NotFound(
                crate::service::sharing::share::UPLOAD_LINK_NOT_FOUND.into(),
            ));
        }
        Ok(LinkModel::Upload(link))
    }
}

fn prompt_page(
    state: &AppState,
    headers: &HeaderMap,
    kind: LinkKind,
    token: &str,
    error: Option<String>,
) -> Result<Response, AppError> {
    let t = I18n::from_headers(headers, &state.config().ui.default_language);
    let tpl = ShareAccessValidationTemplate {
        t,
        urls: crate::static_assets::template_urls(),
        error,
        form_action: kind.form_action(token),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// The URL to send a visitor to when the POST path needs nothing more from them.
fn redirect(token: &str, kind: LinkKind, cookie: Option<String>) -> Response {
    match cookie {
        Some(cookie) => (
            axum::http::StatusCode::FOUND,
            [
                (axum::http::header::LOCATION, kind.redirect_target(token)),
                (axum::http::header::SET_COOKIE, cookie),
            ],
        )
            .into_response(),
        None => (
            axum::http::StatusCode::FOUND,
            [(axum::http::header::LOCATION, kind.redirect_target(token))],
        )
            .into_response(),
    }
}

/// Check the GET password gate for one link.
///
/// The password arrives in the `X-Seafile-Sharelink-Password` header (the
/// pre-cookie client shape) or as the signed unlock cookie set by the form;
/// never in the query string, which would put it into browser history, referrers
/// and request logs.
pub(crate) async fn gate_get(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    token: &str,
    kind: LinkKind,
    mode: GateMode,
) -> Result<Gate, AppError> {
    if !kind.enabled(state) {
        return Ok(Gate::Password(switched_off_link_page(state, headers)));
    }
    let link = match resolve_link(state, token, kind).await {
        Ok(link) => link,
        Err(err) => return Ok(Gate::Password(link_error_page(state, err, headers)?)),
    };

    let provided_pwd = headers
        .get("X-Seafile-Sharelink-Password")
        .and_then(|v| v.to_str().ok());
    let password_version = link.password().unwrap_or("");
    let pw_ok = crate::service::sharing::share::verify_link_password(
        link.password(),
        provided_pwd,
        state.config().auth.password_hash_iterations,
    )
    .await?;
    let cookie_ok = crate::service::auth::csrf::has_link_unlock_cookie(
        headers,
        &kind.cookie_name(link.token()),
        link.token(),
        password_version,
        &state.csrf_secret,
    );
    // A link without a stored password is open; the password check returns true
    // above, so this also covers the POST path's "no password = open" case.
    if link.password().is_none() || pw_ok || cookie_ok {
        let unlock_cookie = if link.password().is_some() && pw_ok {
            Some(crate::service::auth::csrf::link_unlock_cookie(
                &kind.cookie_name(link.token()),
                link.token(),
                password_version,
                &state.csrf_secret,
                state.config().server.secure_cookies(),
            ))
        } else {
            None
        };
        return Ok(Gate::Open {
            link,
            unlock_cookie,
        });
    }

    // Only count genuine failures: a request without a password is the first render.
    if provided_pwd.is_some() {
        record_link_password_failure(state, token)?;
    }
    match mode {
        GateMode::Page => {
            let t = I18n::from_headers(headers, &state.config().ui.default_language);
            let error = provided_pwd.map(|_| t.tr("pub.incorrect_password").to_string());
            Ok(Gate::Password(prompt_page(
                state, headers, kind, token, error,
            )?))
        }
        // The file endpoint answers a raw 403/400 rather than an HTML form.
        GateMode::Raw => {
            if provided_pwd.is_some() {
                Err(AppError::Forbidden)
            } else {
                Err(AppError::BadRequest("password required".into()))
            }
        }
    }
}

/// Check the POST password form for one link.
///
/// A link without a password is already open (upstream's
/// `check_share_link_common` returns before reading the POST), so the visitor is
/// redirected without touching the password or the limiter. A protected link
/// consumes an IP-keyed attempt and, on a wrong password, the per-token budget,
/// then re-renders the form.
pub(crate) async fn gate_post(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    addr: &SocketAddr,
    token: &str,
    kind: LinkKind,
    form: &HashMap<String, String>,
) -> Result<Response, AppError> {
    if !kind.enabled(state) {
        return Ok(switched_off_link_page(state, headers));
    }
    let link = match resolve_link(state, token, kind).await {
        Ok(link) => link,
        Err(err) => return link_error_page(state, err, headers),
    };

    // No stored password: the link is open. Upstream never verifies a password
    // here, so do not charge the limiter for the redirect either.
    if link.password().is_none() {
        return Ok(redirect(token, kind, None));
    }

    // An empty submission is the form's own validation error, not a guess.
    let Some(password) = form
        .get("password")
        .map(String::as_str)
        .filter(|p| !p.is_empty())
    else {
        let t = I18n::from_headers(headers, &state.config().ui.default_language);
        let error = Some(t.tr("pub.incorrect_password").to_string());
        return prompt_page(state, headers, kind, token, error);
    };

    let client_ip = crate::middleware::effective_client_ip(
        addr,
        headers,
        &state.config().server.trusted_proxies,
    );
    let rl_key = format!("link_password:{client_ip}");
    if !state.auth_limiters.link_password.check_and_record(&rl_key) {
        return Err(AppError::TooManyRequests);
    }

    let valid = crate::service::sharing::share::verify_link_password(
        link.password(),
        Some(password),
        state.config().auth.password_hash_iterations,
    )
    .await?;
    if !valid {
        // Token-dimension cap on top of the IP cap, so distributed brute force is bounded.
        record_link_password_failure(state, token)?;
        let t = I18n::from_headers(headers, &state.config().ui.default_language);
        let error = Some(t.tr("pub.incorrect_password").to_string());
        return prompt_page(state, headers, kind, token, error);
    }

    // Hand the browser a signed unlock cookie instead of echoing the password in the URL.
    let cookie = crate::service::auth::csrf::link_unlock_cookie(
        &kind.cookie_name(token),
        token,
        link.password().unwrap_or(""),
        &state.csrf_secret,
        state.config().server.secure_cookies(),
    );
    Ok(redirect(token, kind, Some(cookie)))
}
