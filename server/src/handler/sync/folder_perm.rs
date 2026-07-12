use axum::{Json, Router, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

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
pub async fn folder_perm_post(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<(StatusCode, Json<Vec<FolderPermRes>>), AppError> {
    let requests: Vec<FolderPermReq> = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {}", e)))?;

    let svc = state.sync_service();
    let mut results = Vec::new();
    for req in &requests {
        let result = svc.folder_perm_for_repo(&req.repo_id, &req.token).await?;
        let user_perms = if result.valid {
            vec![PermEntry {
                path: "/".to_string(),
                permission: result.permission,
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
