use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::sharing::link;
use base::error::AppError;

#[derive(Deserialize)]
pub struct SmartLinkQuery {
    pub repo_id: String,
    pub path: String,
}

/// GET /api/v2.1/smart-link/
///
/// Returns a smart link URL that redirects to the file's download URL.
/// The smart link is designed to always point to the latest version of a file.
pub async fn get_smart_link(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SmartLinkQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let base_url = state.config.server.site_url_origin();
    let url = link::get_smart_link(&base_url, &query.repo_id, &query.path);

    Ok(Json(serde_json::json!({
        "url": url,
    })))
}
