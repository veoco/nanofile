//! The HTML error pages.
//!
//! `AppError` answers with the Seafile wire protocol — `{"error_msg": ...}`, or
//! plain text for a locked file — and that is what the desktop client, the CLI
//! and the frontend's fetch calls parse. A *browser* that lands on a page route
//! should not be shown that, so the page routers are wrapped in
//! [`anonymous_pages`] and [`session_pages`]: an error response is rendered as
//! a page. Every wire endpoint (the API namespaces, `/seafhttp/`, sync, WebDAV,
//! and the upload/block endpoints the browser itself calls with `fetch`) keeps
//! the body it has always had.
//!
//! Which shell a page renders in is decided by the route group, not by the
//! reader: the public share pages live in the anonymous shell when they
//! succeed, so their errors do too.

use askama::Template;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::service::sharing::share::LinkFailure;
use crate::ui::auth_extractor::{WebUser, session_cookie};

// ── What the page is ────────────────────────────────────────────────────────

/// A button on an error page. `label_key` is a locale key, so the page is
/// translated exactly like every other page; `primary` picks the filled
/// variant over the outlined one.
pub struct ErrorAction {
    pub label_key: &'static str,
    pub href: String,
    pub primary: bool,
}

/// One error page: what it says and what it offers.
pub struct ErrorPage {
    pub status: StatusCode,
    pub title_key: &'static str,
    pub desc_key: &'static str,
    /// True for a link that no longer resolves. Those pages show a glyph where
    /// the status code would be: "404" is the server's story, not the reader's.
    pub glyph: bool,
    pub actions: Vec<ErrorAction>,
}

impl ErrorPage {
    /// The page for a failed response.
    pub fn for_status(status: StatusCode, signed_in: bool, retry: Option<String>) -> Self {
        let (title_key, desc_key) = copy(status);
        Self {
            status,
            title_key,
            desc_key,
            glyph: false,
            actions: actions(status, signed_in, retry),
        }
    }

    /// The page for a share or upload link that stopped resolving.
    pub fn for_link(failure: LinkFailure) -> Self {
        let (title_key, desc_key) = match failure {
            LinkFailure::Expired => ("err.link_expired.title", "err.link_expired.desc"),
            LinkFailure::Unknown => ("err.link_gone.title", "err.link_gone.desc"),
        };
        Self {
            status: StatusCode::NOT_FOUND,
            title_key,
            desc_key,
            glyph: true,
            actions: vec![ErrorAction {
                label_key: "err.action.home",
                href: "/".into(),
                // Outlined, not filled: there is nothing here to do. The page
                // exists to explain, and the link is just a way off it.
                primary: false,
            }],
        }
    }
}

/// Copy per status. An unmapped 4xx falls back to the 400 wording and an
/// unmapped 5xx to the 500 one, so a status nobody thought about still renders
/// a sentence rather than an empty page.
fn copy(status: StatusCode) -> (&'static str, &'static str) {
    match status.as_u16() {
        400 => ("err.400.title", "err.400.desc"),
        401 => ("err.401.title", "err.401.desc"),
        403 => ("err.403.title", "err.403.desc"),
        404 => ("err.404.title", "err.404.desc"),
        408 => ("err.408.title", "err.408.desc"),
        413 => ("err.413.title", "err.413.desc"),
        429 => ("err.429.title", "err.429.desc"),
        _ if status.is_server_error() => ("err.500.title", "err.500.desc"),
        _ => ("err.400.title", "err.400.desc"),
    }
}

/// What the page offers. A guest cannot go "back to libraries", so the primary
/// action is the one the reader can actually take.
fn actions(status: StatusCode, signed_in: bool, retry: Option<String>) -> Vec<ErrorAction> {
    let repeatable = status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS;

    let mut actions = Vec::new();
    if repeatable && let Some(href) = retry {
        actions.push(ErrorAction {
            label_key: "err.action.retry",
            href,
            primary: true,
        });
    }
    if signed_in {
        actions.push(ErrorAction {
            label_key: "err.action.back",
            href: "/libraries/".into(),
            primary: actions.is_empty(),
        });
    } else if !repeatable {
        actions.push(ErrorAction {
            label_key: "err.action.signin",
            href: "/accounts/login/".into(),
            primary: actions.is_empty(),
        });
    }
    actions
}

// ── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "web/error.html")]
struct AnonymousErrorTemplate {
    t: &'static I18n,
    urls: &'static crate::static_assets::TemplateUrls,
    status: u16,
    glyph: bool,
    title_key: &'static str,
    desc_key: &'static str,
    actions: Vec<ErrorAction>,
}

#[derive(Template)]
#[template(path = "error.html")]
struct AppErrorTemplate {
    t: &'static I18n,
    urls: &'static crate::static_assets::TemplateUrls,
    user_email: String,
    is_admin: bool,
    active_page: &'static str,
    left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    current_repo_id: Option<String>,
    status: u16,
    title_key: &'static str,
    desc_key: &'static str,
    actions: Vec<ErrorAction>,
}

// ── Rendering ───────────────────────────────────────────────────────────────

/// Render in the app chrome when there is a user, in the anonymous shell
/// otherwise.
///
/// The app shell reads the left panel, so building it can itself fail; that
/// falls back to the anonymous page rather than turning one error into two. The
/// anonymous page needs nothing but the locale and the stylesheet URL, which is
/// also why a database outage can still be reported.
async fn render(
    state: &AppState,
    headers: &HeaderMap,
    page: ErrorPage,
    user: Option<&WebUser>,
) -> Response {
    let status = page.status;
    let t = I18n::from_headers(headers, &state.config().ui.default_language);
    let urls = crate::static_assets::template_urls();

    if let Some(user) = user
        && let Ok(ctx) = crate::ui::ctx::build_page_ctx(state, user).await
    {
        return html(
            AppErrorTemplate {
                t: ctx.t,
                urls: ctx.urls,
                user_email: ctx.user_email,
                is_admin: ctx.is_admin,
                // Nothing in the navigation is the current page.
                active_page: "",
                left_panel_repos: ctx.left_panel_repos,
                current_repo_id: None,
                status: status.as_u16(),
                title_key: page.title_key,
                desc_key: page.desc_key,
                actions: page.actions,
            },
            status,
        );
    }

    html(
        AnonymousErrorTemplate {
            t,
            urls,
            status: status.as_u16(),
            glyph: page.glyph,
            title_key: page.title_key,
            desc_key: page.desc_key,
            actions: page.actions,
        },
        status,
    )
}

/// The page for a share or upload link that no longer resolves.
///
/// Always the anonymous shell, and never asynchronous: this is the one page a
/// reader reaches with no account, on a route where there is no session to
/// look up, and it must render even when the database is the reason the link
/// did not resolve.
pub fn link_page(failure: LinkFailure, state: &AppState, headers: &HeaderMap) -> Response {
    let page = ErrorPage::for_link(failure);
    let status = page.status;
    html(
        AnonymousErrorTemplate {
            t: I18n::from_headers(headers, &state.config().ui.default_language),
            urls: crate::static_assets::template_urls(),
            status: status.as_u16(),
            glyph: page.glyph,
            title_key: page.title_key,
            desc_key: page.desc_key,
            actions: page.actions,
        },
        status,
    )
}

/// Render a page with the status it reports.
fn html<T: Template>(template: T, status: StatusCode) -> Response {
    match template.render() {
        Ok(body) => {
            let mut response = Html(body).into_response();
            *response.status_mut() = status;
            // A transient 500 must not be cached into a permanent one, and a
            // stale 404 must not outlive the fix.
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(err) => {
            // There is no page to fall back to, so answer the status with a
            // body that needs no machinery at all.
            tracing::error!(?err, "error page failed to render");
            (status, "error").into_response()
        }
    }
}

// ── Middleware ──────────────────────────────────────────────────────────────

/// Wrap a router whose pages render in the anonymous shell. Used for the public
/// share and upload pages, which look the same signed in or out.
pub async fn anonymous_pages(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    let retry = retry_target(&req);
    let response = next.run(req).await;
    // No session lookup on this path: the shell does not depend on one.
    replace_error(&state, &headers, response, retry, false).await
}

/// Wrap the signed-in page routes: the app chrome when the request carries a
/// session, the anonymous shell otherwise.
pub async fn session_pages(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    let retry = retry_target(&req);
    let response = next.run(req).await;
    replace_error(&state, &headers, response, retry, true).await
}

/// Swap an error response for a page, unless the handler already rendered one.
async fn replace_error(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    response: Response,
    retry: Option<String>,
    look_up_user: bool,
) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) || is_html(&response) {
        return response;
    }

    // The session is read only here, on the error path: resolving it costs three
    // lookups, and a page that rendered fine must not pay for them.
    let user = match look_up_user {
        true => match session_cookie(headers) {
            Some(token) => WebUser::from_session_token(state, &token).await,
            None => None,
        },
        false => None,
    };
    let page = ErrorPage::for_status(status, user.is_some(), retry);
    render(state, headers, page, user.as_ref()).await
}

/// Whether a handler already produced a page — the link pages and the password
/// gate do, and wrapping one in another would lose it.
fn is_html(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"))
}

/// Where "Try again" points, or `None` when repeating the request is not what
/// the reader wants.
///
/// Offered for a GET only: replaying a failed POST through a link would turn a
/// form submission into a different request. The value is always a path this
/// server just served, so it cannot become an off-site href.
fn retry_target(req: &Request) -> Option<String> {
    if req.method() != Method::GET {
        return None;
    }
    req.uri()
        .path_and_query()
        .map(|path_and_query| path_and_query.as_str().to_string())
}

// ── Router fallback ─────────────────────────────────────────────────────────

/// The path matched no route.
///
/// A browser gets the page; anything else keeps the empty 404 body axum
/// produced before this existed, so a client probing an unknown API path sees
/// no change.
pub async fn unknown_path(State(state): State<Arc<AppState>>, req: Request) -> Response {
    if !prefers_html(req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let headers = req.headers().clone();
    let retry = retry_target(&req);
    let user = match session_cookie(&headers) {
        Some(token) => WebUser::from_session_token(&state, &token).await,
        None => None,
    };
    let page = ErrorPage::for_status(StatusCode::NOT_FOUND, user.is_some(), retry);
    render(&state, &headers, page, user.as_ref()).await
}

/// Whether the client asked for HTML. A browser sends
/// `text/html,application/xhtml+xml,…`; a client that wants the wire protocol
/// asks for JSON, or for nothing in particular.
fn prefers_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(page: &ErrorPage) -> Vec<&'static str> {
        page.actions.iter().map(|a| a.label_key).collect()
    }

    #[test]
    fn a_guest_is_offered_sign_in_and_a_user_is_offered_their_libraries() {
        let guest = ErrorPage::for_status(StatusCode::NOT_FOUND, false, Some("/nope/".into()));
        assert_eq!(labels(&guest), vec!["err.action.signin"]);
        assert!(guest.actions[0].primary);

        let user = ErrorPage::for_status(StatusCode::NOT_FOUND, true, Some("/nope/".into()));
        assert_eq!(labels(&user), vec!["err.action.back"]);
        assert!(user.actions[0].primary);
        assert_eq!(user.actions[0].href, "/libraries/");
    }

    #[test]
    fn a_repeatable_failure_leads_with_retry_and_keeps_the_libraries_link() {
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let page = ErrorPage::for_status(status, true, Some("/libraries/x/files/".into()));
            assert_eq!(
                labels(&page),
                vec!["err.action.retry", "err.action.back"],
                "{status}"
            );
            assert!(page.actions[0].primary, "{status}");
            assert!(!page.actions[1].primary, "{status}");
        }
        // A guest gets the retry with no second button to a page they cannot
        // open.
        let guest =
            ErrorPage::for_status(StatusCode::INTERNAL_SERVER_ERROR, false, Some("/x/".into()));
        assert_eq!(labels(&guest), vec!["err.action.retry"]);
    }

    #[test]
    fn a_failure_that_cannot_be_repeated_offers_no_retry() {
        // The POST path passes no retry target, and neither does any status
        // whose request the reader cannot simply send again.
        let page = ErrorPage::for_status(StatusCode::FORBIDDEN, true, None);
        assert_eq!(labels(&page), vec!["err.action.back"]);
        let page = ErrorPage::for_status(StatusCode::BAD_REQUEST, true, Some("/x/".into()));
        assert_eq!(labels(&page), vec!["err.action.back"]);
    }

    #[test]
    fn every_status_renders_some_copy() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::PAYLOAD_TOO_LARGE,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            // A status nobody enumerated still gets a sentence, not an empty
            // page or a raw key.
            StatusCode::IM_A_TEAPOT,
        ] {
            let t = I18n::get(Some("en"));
            let (title, desc) = copy(status);
            assert_ne!(t.tr(title), title, "no title for {status}");
            assert_ne!(t.tr(desc), desc, "no description for {status}");
        }
        let t = I18n::get(Some("en"));
        for key in ["err.link_expired.title", "err.link_gone.title"] {
            assert_ne!(t.tr(key), key);
        }
    }

    #[test]
    fn a_dead_link_shows_a_glyph_instead_of_a_status_code() {
        let expired = ErrorPage::for_link(LinkFailure::Expired);
        assert!(expired.glyph);
        assert_eq!(expired.status, StatusCode::NOT_FOUND);
        assert_eq!(expired.title_key, "err.link_expired.title");

        let gone = ErrorPage::for_link(LinkFailure::Unknown);
        assert_eq!(gone.title_key, "err.link_gone.title");
        // "Go to Nanofile" is a way off the page, not the point of it.
        assert_eq!(labels(&gone), vec!["err.action.home"]);
        assert!(!gone.actions[0].primary);
    }

    #[test]
    fn only_a_get_can_be_retried() {
        let get = Request::builder()
            .uri("/libraries/x/files/?p=/sub")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            retry_target(&get).as_deref(),
            Some("/libraries/x/files/?p=/sub")
        );

        let post = Request::builder()
            .method(Method::POST)
            .uri("/settings/profile/")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(retry_target(&post), None);
    }

    #[test]
    fn browsers_are_told_apart_from_clients() {
        let browser = HeaderMap::from_iter([(
            header::ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9"),
        )]);
        assert!(prefers_html(&browser));

        let client =
            HeaderMap::from_iter([(header::ACCEPT, HeaderValue::from_static("application/json"))]);
        assert!(!prefers_html(&client));

        // curl's default, and what a link preview bot sends.
        let anything = HeaderMap::from_iter([(header::ACCEPT, HeaderValue::from_static("*/*"))]);
        assert!(!prefers_html(&anything));
        assert!(!prefers_html(&HeaderMap::new()));
    }
}
