use axum::{Json, Router, extract::State, http::StatusCode};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::entity::{repo_member, sync_token};
use crate::error::AppError;

/// Individual permission entry (path + permission level).
#[derive(Serialize, Deserialize)]
pub struct PermEntry {
    pub path: String,
    pub permission: String,
}

/// A single request from the daemon for a specific repo.
#[derive(Deserialize)]
pub struct FolderPermReq {
    pub repo_id: String,
    pub token: String,
    pub ts: i64,
}

/// A single response entry — mirrors original seafile-server format.
#[derive(Serialize)]
pub struct FolderPermRes {
    pub repo_id: String,
    pub ts: i64,
    #[serde(rename = "user_perms")]
    pub user_perms: Vec<PermEntry>,
    #[serde(rename = "group_perms")]
    pub group_perms: Vec<PermEntry>,
}

/// `POST /seafhttp/repo/folder-perm`
///
/// The seaf-daemon sends a POST with a JSON body containing a list of
/// `{repo_id, token, ts}` objects. Uses curl defaults (no Content-Type
/// header), so we parse the raw body manually rather than relying on
/// axum's Json extractor.
pub async fn folder_perm_post(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<(StatusCode, Json<Vec<FolderPermRes>>), AppError> {
    let requests: Vec<FolderPermReq> = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {}", e)))?;

    let mut results = Vec::new();
    for req in &requests {
        let token_valid = sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(&req.token))
            .filter(sync_token::Column::RepoId.eq(&req.repo_id))
            .one(state.db.as_ref())
            .await?
            .is_some();

        let user_perms = if token_valid {
            let memberships = repo_member::Entity::find()
                .filter(repo_member::Column::RepoId.eq(&req.repo_id))
                .all(state.db.as_ref())
                .await?;

            let permission = memberships
                .first()
                .map(|m| m.permission.clone())
                .unwrap_or_else(|| "rw".to_string());

            vec![PermEntry {
                path: "/".to_string(),
                permission,
            }]
        } else {
            vec![]
        };

        results.push(FolderPermRes {
            repo_id: req.repo_id.clone(),
            ts: req.ts,
            user_perms,
            group_perms: vec![],
        });
    }

    Ok((StatusCode::OK, Json(results)))
}

pub fn folder_perm_routes() -> Router<Arc<AppState>> {
    Router::new().route("/folder-perm", axum::routing::post(folder_perm_post))
}
