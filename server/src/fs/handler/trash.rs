use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::Request,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::{RepoPathRead, RepoPathWrite};
use crate::error::AppError;

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

type RevertTrashBody = HashMap<String, Vec<String>>;

#[derive(Deserialize)]
pub struct RevertDirentsForm {
    pub commit_id: String,
    pub file_names: Option<String>,
}

#[derive(Deserialize)]
pub struct CleanTrashBody {
    pub keep_days: Option<i64>,
}

#[derive(Deserialize)]
pub struct RestoreDeletedRepoBody {
    pub repo_id: String,
}

pub async fn list_trash2(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<Trash2Query>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(50);

    let svc = state.trash_service();
    let result = svc.list_trash2(repo_id, page, per_page).await?;

    Ok(Json(result))
}

pub async fn search_trash(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchTrashQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(50);

    let svc = state.trash_service();
    let result = svc
        .search_trash(
            repo_id,
            query.q.as_deref().unwrap_or(""),
            page,
            per_page,
            query.op_users.as_deref(),
            query.time_from,
            query.time_to,
            query.suffixes.as_deref(),
        )
        .await?;

    Ok(Json(result))
}

pub async fn revert_trash(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RevertTrashBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let svc = state.trash_service();
    let result = svc
        .revert_trash(repo_id, &access.user.email, access.user.user_id, body)
        .await?;

    Ok(Json(result))
}

pub async fn revert_dirents(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

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

    let svc = state.trash_service();
    let result = svc
        .revert_dirents(
            repo_id,
            &access.user.email,
            access.user.user_id,
            commit_id,
            paths,
        )
        .await?;

    Ok(Json(result))
}

pub async fn clean_trash(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let keep_days = parse_clean_trash_body(req).await;

    let svc = state.trash_service();
    svc.clean_trash(repo_id, access.user.user_id, keep_days)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

async fn parse_clean_trash_body(req: Request<Body>) -> Option<i64> {
    let (_, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.ok()?;
    serde_json::from_slice::<CleanTrashBody>(&bytes)
        .ok()
        .and_then(|b| b.keep_days)
}

pub async fn list_trash(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<TrashQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let limit = query.limit.unwrap_or(50);

    let svc = state.trash_service();
    let result = svc.list_trash_cursor(repo_id, query.cursor, limit).await?;

    Ok(Json(result))
}

pub async fn list_deleted_repos(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.trash_service();
    let items = svc.list_deleted_repos(auth.user_id, &auth.email).await?;

    Ok(Json(serde_json::json!({"repos": items})))
}

pub async fn restore_deleted_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RestoreDeletedRepoBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.trash_service();
    svc.restore_deleted_repo(&body.repo_id, auth.user_id)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
