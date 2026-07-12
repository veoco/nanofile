use axum::{Json, extract::State};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

/// POST `/seafhttp/repo/head-commits-multi/`
///
/// Accepts a JSON array of repo IDs and returns `{repo_id: head_commit_id}` map.
/// Uses raw body (not `Json` extractor) because the C sync client sends JSON
/// body without a `Content-Type` header.
pub async fn head_commits_multi(
    State(state): State<Arc<AppState>>,
    body: axum::body::Body,
) -> Result<Json<HashMap<String, String>>, AppError> {
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let repo_id_list: Vec<String> = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("expected JSON array of repo IDs".into()))?;

    let commits = state
        .sync_service()
        .head_commits_multi(&repo_id_list)
        .await?;
    Ok(Json(commits))
}
