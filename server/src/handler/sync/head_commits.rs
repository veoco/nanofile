use axum::{Json, extract::State};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

/// Maximum number of repo ids accepted in one batch.
const MAX_REPO_IDS: usize = 4096;

/// POST `/seafhttp/repo/head-commits-multi/`
///
/// Accepts a JSON array of repo IDs and returns `{repo_id: head_commit_id}` map.
/// Uses raw body (not `Json` extractor) because the C sync client sends JSON
/// body without a `Content-Type` header.
///
/// This endpoint is intentionally unauthenticated (seaf-daemon calls it with
/// no credentials, matching official seafile), so it validates its input
/// strictly instead: every id must be a UUID and the array size is bounded.
pub async fn head_commits_multi(
    State(state): State<Arc<AppState>>,
    body: axum::body::Body,
) -> Result<Json<HashMap<String, String>>, AppError> {
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let repo_id_list: Vec<String> = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("expected JSON array of repo IDs".into()))?;

    if repo_id_list.is_empty() || repo_id_list.len() > MAX_REPO_IDS {
        return Err(AppError::BadRequest("invalid repo id array size".into()));
    }

    for id in &repo_id_list {
        if !is_uuid_str(id) {
            return Err(AppError::BadRequest("invalid repo id format".into()));
        }
    }

    let commits = state
        .sync_service()
        .head_commits_multi(&repo_id_list)
        .await?;
    Ok(Json(commits))
}

/// True when `s` is a UUID-formatted string (8-4-4-4-12 hex).
fn is_uuid_str(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, &c)| i == 8 || i == 13 || i == 18 || i == 23 || c.is_ascii_hexdigit())
}
