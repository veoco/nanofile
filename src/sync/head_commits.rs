use axum::{Json, extract::State};
use sea_orm::EntityTrait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::entity::repo;
use crate::error::AppError;

/// POST `/seafhttp/repo/head-commits-multi/`
///
/// Accepts a JSON array of repo IDs and returns `{repo_id: head_commit_id}` map.
///
/// Uses raw body (not `Json` extractor) because the C sync client sends JSON
/// body without a `Content-Type` header, causing axum's `Json` extractor to
/// reject with 415.
pub async fn head_commits_multi(
    State(state): State<Arc<AppState>>,
    body: axum::body::Body,
) -> Result<Json<HashMap<String, String>>, AppError> {
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let repo_id_list: Vec<String> = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("expected JSON array of repo IDs".into()))?;

    let mut commits = HashMap::new();

    for repo_id in &repo_id_list {
        let repo_model = repo::Entity::find_by_id(repo_id)
            .one(state.db.as_ref())
            .await?;

        if let Some(r) = repo_model
            && let Some(head_id) = &r.head_commit_id
        {
            commits.insert(repo_id.clone(), head_id.clone());
        }
    }

    Ok(Json(commits))
}
