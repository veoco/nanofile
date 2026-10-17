/// SSO local-browser flow UI handlers.
///
/// `GET /client-sso/{token}/` records the first visit and bounces through the
/// web login; `.../complete/` mints the API token the client polls for.
use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::service::auth::csrf;
use crate::ui::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "page/client_login_confirm.html")]
pub struct ClientLoginConfirmTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    /// HMAC CSRF token for the hidden form field.
    pub csrf_token: String,
    pub action: String,
    /// Who is asking for the account, when the caller supplied it.
    ///
    /// Showing the reported platform/device (`shib_platform` / `shib_device_name`
    /// / `shib_client_version`, sent by the desktop client) is what lets a user
    /// notice a link they did not start; without it this page is a phishing primitive.
    pub requester: Option<String>,
}

#[derive(Template)]
#[template(path = "page/client_login_complete.html")]
pub struct ClientLoginCompleteTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
}

#[derive(Template)]
#[template(path = "page/client_sso_error.html")]
pub struct ClientSsoErrorTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    /// Locale key for the failure, not the text: these pages share the auth shell.
    pub message_key: &'static str,
}

#[derive(Deserialize)]
pub struct ClientSsoCompleteForm {
    pub csrf_token: Option<String>,
}

/// GET /client-sso/{token}/ — the browser entry point.
///
/// First visit records `accessed_at` (starting the 300s window) and redirects
/// to the login page; later visits show seahub's "already visited" error.
pub async fn client_sso(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    if !state.config().server.sso_enabled {
        return sso_error(&state, &headers, "auth.sso_disabled").await;
    }

    let svc = state.sso_service();
    match svc.open_sso_link(&token).await {
        Ok(true) => {}
        Ok(false) => {
            return sso_error(&state, &headers, "auth.sso_link_visited").await;
        }
        Err(_) => {
            return sso_error(&state, &headers, "auth.sso_link_invalid").await;
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
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let svc = state.sso_service();
    if svc.validate_sso_link_for_completion(&token).await.is_err() {
        return sso_error(&state, &headers, "auth.sso_link_expired").await;
    }

    let csrf_token = csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    let tpl = ClientLoginConfirmTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
        csrf_token,
        action: format!("/client-sso/{token}/complete/"),
        requester: svc.sso_link_requester(&token).await,
    };
    render(tpl)
}

/// POST /client-sso/{token}/complete/ — confirm login, mint the API token.
pub async fn client_sso_complete(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Form(form): Form<ClientSsoCompleteForm>,
) -> Response {
    if csrf::check_form_csrf(&state, &user.session_token, form.csrf_token.as_deref()).is_err() {
        return sso_error(&state, &headers, "common.invalid_csrf").await;
    }

    let svc = state.sso_service();
    if svc.complete_sso_link(&token, &user.email).await.is_err() {
        return sso_error(&state, &headers, "auth.sso_link_expired").await;
    }

    render(ClientLoginCompleteTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
    })
}

fn render<T: Template>(tpl: T) -> Response {
    match tpl.render() {
        Ok(html) => (StatusCode::OK, Html(html)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn sso_error(state: &AppState, headers: &HeaderMap, message_key: &'static str) -> Response {
    let tpl = ClientSsoErrorTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(headers, &state.config().ui.default_language),
        message_key,
    };
    match tpl.render() {
        Ok(html) => (StatusCode::BAD_REQUEST, Html(html)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
