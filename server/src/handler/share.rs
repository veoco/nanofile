use axum::{
    Json, Router,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use crate::service::sharing::share;
use base::error::AppError;

#[derive(Deserialize)]
pub struct CreateShareLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expires_at: Option<i64>,
}

pub fn share_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/",
            axum::routing::get(list_share_links).post(create_share_link),
        )
        .route("/{token}", axum::routing::delete(delete_share_link))
}

pub async fn list_share_links(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<share::ShareLinkInfo>>, AppError> {
    let mut infos =
        share::list_share_links(&state.repos, &state.config().server.site_url, auth.user_id)
            .await?;
    // A share token is a capability URL: never hand one out for a library
    // outside the key's scope.
    let scope = auth.repo_scope();
    infos.retain(|link| scope.allows(&link.repo_id));
    Ok(Json(infos))
}

pub async fn create_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateShareLinkRequest>,
) -> Result<Json<share::ShareLinkInfo>, AppError> {
    crate::middleware::ensure_share_links_enabled(&state)?;
    let info = share::create_share_link(
        &state.repos,
        &state.config(),
        &req.repo_id,
        &req.path,
        req.password.as_deref(),
        req.expires_at,
        auth.user_id,
    )
    .await?;
    Ok(Json(info))
}

pub async fn delete_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    share::delete_share_link(&state.repos, &token, auth.user_id).await?;
    Ok(ok_json())
}
