/// SSO local-browser flow UI handlers.
///
/// `GET /client-sso/{token}/` opens in the browser, records the first visit
/// and bounces the user through the web login (with `next` set back here).
/// `GET/POST /client-sso/{token}/complete/` confirm the login against the
/// logged-in web session and mint the API token the client polls for.
use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::service::auth::csrf;
use crate::ui::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "page/client_login_confirm.html")]
pub struct ClientLoginConfirmTemplate {
    /// HMAC CSRF token for the hidden form field.
    pub csrf_token: String,
    /// Absolute path this form POSTs to.
    pub action: String,
}

#[derive(Template)]
#[template(path = "page/client_login_complete.html")]
pub struct ClientLoginCompleteTemplate {}

#[derive(Template)]
#[template(path = "page/client_sso_error.html")]
pub struct ClientSsoErrorTemplate {
    pub message: String,
}

#[derive(Deserialize)]
pub struct ClientSsoCompleteForm {
    pub csrf_token: Option<String>,
}

// ─── Handlers ─────────────────────────────────────────────────────────────

/// GET /client-sso/{token}/ — the browser entry point.
///
/// First visit records `accessed_at` (starting the 300s window) and redirects
/// to the web login page with `next` pointing back to the confirm page.
/// Subsequent visits show the seahub-compatible "already visited" error.
pub async fn client_sso(State(state): State<Arc<AppState>>, Path(token): Path<String>) -> Response {
    if !state.config.server.sso_enabled {
        return sso_error("Feature is not enabled.").await;
    }

    let svc = state.sso_service();
    match svc.open_sso_link(&token).await {
        Ok(true) => {}
        Ok(false) => {
            return sso_error(
                "This link has already been visited, please click the login button on the client again",
            )
            .await;
        }
        Err(_) => {
            return sso_error("Invalid link, please click the login button on the client again")
                .await;
        }
    }

    let next = format!("/client-sso/{token}/complete/");
    let encoded: String =
        percent_encoding::utf8_percent_encode(&next, percent_encoding::NON_ALPHANUMERIC).collect();
    // 302, matching seahub's HttpResponseRedirect (axum's Redirect::to is 303).
    (
        StatusCode::FOUND,
        [("Location", format!("/accounts/login/?next={encoded}"))],
    )
        .into_response()
}

/// GET /client-sso/{token}/complete/ — confirm page (requires web session).
pub async fn client_sso_complete_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Response {
    let svc = state.sso_service();
    if svc.validate_sso_link_for_completion(&token).await.is_err() {
        return sso_error(
            "Invalid or expired link, please click the login button on the client again",
        )
        .await;
    }

    let csrf_token = csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    let tpl = ClientLoginConfirmTemplate {
        csrf_token,
        action: format!("/client-sso/{token}/complete/"),
    };
    render(tpl)
}

/// POST /client-sso/{token}/complete/ — confirm login, mint the API token.
pub async fn client_sso_complete(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Form(form): Form<ClientSsoCompleteForm>,
) -> Response {
    if csrf::check_form_csrf(&state, &user.session_token, form.csrf_token.as_deref()).is_err() {
        return sso_error("Invalid CSRF token.").await;
    }

    let svc = state.sso_service();
    if svc.complete_sso_link(&token, &user.email).await.is_err() {
        return sso_error(
            "Invalid or expired link, please click the login button on the client again",
        )
        .await;
    }

    render(ClientLoginCompleteTemplate {})
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn render<T: Template>(tpl: T) -> Response {
    match tpl.render() {
        Ok(html) => (StatusCode::OK, Html(html)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn sso_error(message: &str) -> Response {
    let tpl = ClientSsoErrorTemplate {
        message: message.to_string(),
    };
    match tpl.render() {
        Ok(html) => (StatusCode::BAD_REQUEST, Html(html)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
