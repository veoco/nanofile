use askama::Template;
use axum::{
    extract::{ConnectInfo, Path, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use super::link_gate::{Gate, GateMode, LinkKind, LinkModel};
use crate::AppState;
use crate::i18n::I18n;
use base::error::AppError;

#[derive(Template)]
#[template(path = "web/upload_link_view.html")]
struct UploadLinkViewTemplate {
    pub t: &'static I18n,
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub token: String,
    pub dir_name: String,
    pub has_password: bool,
    pub max_upload_size_mb: i64,
    pub description: Option<String>,
}

/// GET /u/{token}/ — show the public upload page.
///
/// Query parameters are deliberately not extracted: the password arrives via
/// the `X-Seafile-Sharelink-Password` header, the password form POST, or the
/// signed unlock cookie — never the URL.
pub async fn upload_link_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let (link, unlock_cookie) = match super::link_gate::gate_get(
        &state,
        &headers,
        &token,
        LinkKind::Upload,
        GateMode::Page,
    )
    .await?
    {
        Gate::Open {
            link: LinkModel::Upload(link),
            unlock_cookie,
        } => (link, unlock_cookie),
        Gate::Open { .. } => {
            return Err(AppError::internal("wrong link kind behind an upload gate"));
        }
        Gate::Password(response) => return Ok(response),
    };

    let dir_name = link
        .path
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| link.path.clone());

    let tpl = UploadLinkViewTemplate {
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
        urls: crate::static_assets::template_urls(),
        token: link.token.clone(),
        dir_name,
        has_password: link.password.is_some(),
        max_upload_size_mb: state.config().server.max_upload_size_mb as i64,
        description: link.description.clone(),
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut response = Html(html).into_response();

    // A password supplied through the header unlocks the one-shot upload URL too:
    // that endpoint authorizes via the signed `visited_ufs_` cookie.
    if let Some(cookie) = unlock_cookie {
        response.headers_mut().append(
            axum::http::header::SET_COOKIE,
            axum::http::HeaderValue::from_str(&cookie)
                .map_err(|e| AppError::internal(format!("invalid unlock cookie: {e}")))?,
        );
    }
    Ok(response)
}

/// POST /u/{token}/ — validate the password, set the unlock cookie, then redirect
/// to the upload page (the password is NOT carried in the URL).
pub async fn upload_link_view_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    super::link_gate::gate_post(&state, &headers, &addr, &token, LinkKind::Upload, &form).await
}
