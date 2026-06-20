use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::fs::service::dir_service::DirService;

/// Local query type for the v2.1 dir delete endpoint.
/// Only needs `p` (path) — avoids sharing `V21DirQuery` which has more fields.
#[derive(Deserialize)]
pub struct DeleteQuery {
    p: Option<String>,
}

/// DELETE /api/v2.1/repos/{repo_id}/dir/
pub async fn v21_delete_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    let svc = DirService::new(state.repos.clone(), state.db.clone(), state.indexer.clone());
    let email = auth.email.clone();
    // Use a closure to isolate the async call from the handler signature
    let result = svc
        .delete_dirent(&repo_id, "dir", &normalized, &email, auth.user_id)
        .await;
    let _ = result;
    Ok(Json(serde_json::json!({"success": true})))
}
