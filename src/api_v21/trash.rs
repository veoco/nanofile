use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::Request,
};
use sea_orm::EntityTrait;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::user;
use crate::error::AppError;
use crate::storage::trash::TrashService;

// ─── Query / Request types ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct Trash2Query {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Deserialize)]
pub struct TrashQuery {
    pub cursor: Option<i64>,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct SearchTrashQuery {
    pub q: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub op_users: Option<String>,
    pub time_from: Option<i64>,
    pub time_to: Option<i64>,
    pub suffixes: Option<String>,
}

/// Request body for POST /api/v2.1/repos/{repo_id}/trash2/revert/
///
/// Keys are commit_ids, values are arrays of full paths.
/// e.g. `{"abc123": ["/dir/file.txt"], "def456": ["/other/doc.pdf"]}`
type RevertTrashBody = HashMap<String, Vec<String>>;

/// Request body for POST /api/v2.1/repos/{repo_id}/trash/revert-dirents/
/// Parsed from form-encoded body (old API).
#[derive(Deserialize)]
pub struct RevertDirentsForm {
    pub commit_id: String,
    pub file_names: Option<String>,
}

/// Request body for DELETE /api/v2.1/repos/{repo_id}/trash/
#[derive(Deserialize)]
pub struct CleanTrashBody {
    pub keep_days: Option<i64>,
}

/// Request body for POST /api/v2.1/deleted-repos/
#[derive(Deserialize)]
pub struct RestoreDeletedRepoBody {
    pub repo_id: String,
}

// ─── Handlers ─────────────────────────────────────────────────────────

/// GET /api/v2.1/repos/{repo_id}/trash2/
///
/// Page-based trash listing. Requires read permission.
///
/// Response:
/// ```json
/// {
///   "items": [{ "parent_dir": "...", "obj_name": "...", ... }],
///   "total_count": 42,
///   "can_search": true
/// }
/// ```
pub async fn list_trash2(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<Trash2Query>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(50);

    let result = TrashService::list_trash2(state.db.as_ref(), &repo_id, page, per_page).await?;

    Ok(Json(serde_json::to_value(result)?))
}

/// GET /api/v2.1/repos/{repo_id}/trash2/search/
///
/// Search trash by keyword, user, time range, and file extensions.
/// Requires read permission.
pub async fn search_trash(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<SearchTrashQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(50);

    let result = TrashService::search_trash(
        state.db.as_ref(),
        &repo_id,
        query.q.as_deref().unwrap_or(""),
        page,
        per_page,
        query.op_users.as_deref(),
        query.time_from,
        query.time_to,
        query.suffixes.as_deref(),
    )
    .await?;

    Ok(Json(serde_json::to_value(result)?))
}

/// POST /api/v2.1/repos/{repo_id}/trash2/revert/
///
/// Restore deleted items from trash. Requires write permission.
///
/// Request body:
/// ```json
/// {
///   "commit_id_1": ["/path/file1.txt", "/path/file2.txt"],
///   "commit_id_2": ["/path/dir/"]
/// }
/// ```
///
/// Response:
/// ```json
/// {
///   "success": [{"path": "/path/file1.txt", "is_dir": false}],
///   "failed": [{"commit_id": "...", "path": "/path/notfound", "error_msg": "Dirent ... not found."}]
/// }
/// ```
pub async fn revert_trash(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(body): Json<RevertTrashBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let result = TrashService::restore_trash_items(
        state.db.as_ref(),
        &repo_id,
        &auth.email,
        auth.user_id,
        body,
    )
    .await?;

    Ok(Json(serde_json::to_value(result)?))
}

/// POST /api/v2.1/repos/{repo_id}/trash/revert-dirents/
///
/// Old API for restoring items from trash.
/// Accepts form-encoded body with `commit_id` and `file_names` (colon-separated paths).
/// Requires write permission.
pub async fn revert_dirents(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let (_, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))?;

    let commit_id = form
        .get("commit_id")
        .ok_or_else(|| AppError::BadRequest("commit_id required".into()))?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");

    let paths: Vec<String> = if file_names_str.is_empty() {
        Vec::new()
    } else {
        file_names_str
            .split(':')
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .collect()
    };

    let result = TrashService::restore_dirents(
        state.db.as_ref(),
        &repo_id,
        &auth.email,
        auth.user_id,
        commit_id,
        paths,
    )
    .await?;

    Ok(Json(serde_json::to_value(result)?))
}

/// DELETE /api/v2.1/repos/{repo_id}/trash/
///
/// Clean trash, optionally keeping items newer than `keep_days`.
/// Requires write permission.
///
/// Request body (JSON, optional):
/// ```json
/// { "keep_days": 30 }
/// ```
pub async fn clean_trash(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let keep_days = parse_clean_trash_body(req).await;

    TrashService::clean_trash(state.db.as_ref(), &repo_id, keep_days).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// Parse optional keep_days from the request body (may be empty JSON or
/// no body at all).
async fn parse_clean_trash_body(req: Request<Body>) -> Option<i64> {
    let (_, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.ok()?;
    serde_json::from_slice::<CleanTrashBody>(&bytes)
        .ok()
        .and_then(|b| b.keep_days)
}

/// GET /api/v2.1/repos/{repo_id}/trash/
///
/// Cursor-based trash listing. Requires read permission.
///
/// Query params:
/// - `cursor`: optional delete_time cursor (returns items older than cursor)
/// - `limit`: max items to return (default 50, max 100)
pub async fn list_trash(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<TrashQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let limit = query.limit.unwrap_or(50);

    let result =
        TrashService::list_trash_cursor(state.db.as_ref(), &repo_id, query.cursor, limit).await?;

    Ok(Json(serde_json::to_value(result)?))
}

/// GET /api/v2.1/deleted-repos/
///
/// List repos that the current user has deleted.
pub async fn list_deleted_repos(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repos = TrashService::list_deleted_repos(state.db.as_ref(), auth.user_id).await?;

    // Look up user for nickname
    let owner_name = user::Entity::find_by_id(auth.user_id)
        .one(state.db.as_ref())
        .await?
        .map(|u| u.nickname())
        .unwrap_or_else(|| auth.email.split('@').next().unwrap_or("").to_string());

    let items: Vec<serde_json::Value> = repos
        .iter()
        .map(|r| {
            serde_json::json!({
                "repo_id": r.repo_id,
                "repo_name": r.repo_name,
                "owner_email": auth.email,
                "owner_name": &owner_name,
                "owner_contact_email": auth.email,
                "head_commit_id": r.head_id,
                "size": r.size,
                "del_time": chrono::DateTime::from_timestamp(r.del_time, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                "org_id": -1,
                "encrypted": false,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"repos": items})))
}

/// POST /api/v2.1/deleted-repos/
///
/// Restore a deleted repo from trash.
///
/// Request body:
/// ```json
/// { "repo_id": "..." }
/// ```
pub async fn restore_deleted_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RestoreDeletedRepoBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    TrashService::restore_deleted_repo(state.db.as_ref(), &body.repo_id, auth.user_id).await?;

    Ok(Json(serde_json::json!({"success": true})))
}
