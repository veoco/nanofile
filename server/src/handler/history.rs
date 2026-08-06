use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::repo_extractor::{RepoPathRead, RepoPathWrite};
use crate::service::repo::history::HistoryService;
use base::error::AppError;

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub commit_id: String,
}

/// `GET /api2/repo_history_changes/{repo_id}/?commit_id=`
///
/// Returns the file changes introduced by a specific commit.
pub async fn repo_history_changes(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<crate::service::repo::history::HistoryChangesResponse>, AppError> {
    let repo_id = path.repo_id;
    let response =
        HistoryService::get_history_changes(&state.repos, &repo_id, &query.commit_id).await?;

    Ok(Json(response))
}

pub fn repo_history_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/repo_history_changes/{repo_id}/",
        axum::routing::get(repo_history_changes),
    )
}

#[derive(Deserialize)]
pub struct FileHistoryQuery {
    pub p: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v2.1/repos/{repo_id}/file/history/?p=&limit=`
///
/// Returns the version history of a single file (newest first), in the seahub
/// `FileHistoryView` envelope `{data, page, page_next}`.
pub async fn get_file_history_v21(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileHistoryQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let p = query
        .p
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let limit = query.limit.unwrap_or(25).clamp(1, 100) as u64;

    let items = HistoryService::get_file_history(&state.repos, &repo_id, &p, limit).await?;
    Ok(Json(serde_json::json!({
        "data": items,
        "page": 1,
        "page_next": false,
    })))
}

#[derive(Deserialize)]
pub struct RevisionQuery {
    pub p: Option<String>,
    pub commit_id: Option<String>,
}

/// `GET /api/v2.1/repos/{repo_id}/file/revision/?p=&commit_id=`
///
/// Streams the file content as it existed at the given commit (for download or
/// preview). Mirrors seahub's `FileRevision` endpoint.
pub async fn get_file_revision_v21(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<RevisionQuery>,
) -> Result<Response, AppError> {
    let repo_id = path.repo_id;
    let p = query
        .p
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let commit_id = query
        .commit_id
        .ok_or_else(|| AppError::BadRequest("commit_id required".into()))?;

    let (_fs_id, file_data) =
        HistoryService::get_file_revision(&state.repos, &repo_id, &commit_id, &p).await?;

    let dec_key = crate::handler::web::download::get_decryption_key_for_repo(
        &state,
        &repo_id,
        path.user.user_id,
    )
    .await?;
    let stream = crate::fs::core::download::stream_blocks(
        file_data.block_ids,
        state.block_store.clone(),
        dec_key,
    );

    let filename = p.rsplit_once('/').map(|(_, n)| n).unwrap_or("download");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

#[derive(Deserialize)]
pub struct RestoreQuery {
    pub p: Option<String>,
    pub commit_id: Option<String>,
}

/// `POST /api/v2.1/repos/{repo_id}/file/revision/restore/?p=&commit_id=`
///
/// Restores a file to a historical version by pointing the file's dirent at
/// the target commit's fs_id and committing the change.
pub async fn restore_file_revision_v21(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<RestoreQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let p = query
        .p
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let commit_id = query
        .commit_id
        .ok_or_else(|| AppError::BadRequest("commit_id required".into()))?;

    HistoryService::restore_file_revision(
        &state.db,
        &state.repos,
        &repo_id,
        &commit_id,
        &p,
        &path.user.email,
        path.user.user_id,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// Routes nested under `/api/v2.1` by `routes.rs`.
pub fn file_history_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/repos/{repo_id}/file/history/",
            axum::routing::get(get_file_history_v21),
        )
        .route(
            "/repos/{repo_id}/file/revision/",
            axum::routing::get(get_file_revision_v21),
        )
        .route(
            "/repos/{repo_id}/file/revision/restore/",
            axum::routing::post(restore_file_revision_v21),
        )
}
