use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, header},
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::repo_extractor::{RepoPathRead, RepoPathWrite};
use crate::service::repo::history::HistoryService;
use base::error::AppError;

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub commit_id: String,
}

/// `GET /api2/repo_history_changes/{repo_id}/?commit_id=` — the file changes a
/// specific commit introduced.
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

/// `GET /api/v2.1/repos/{repo_id}/file/history/?p=&limit=` — one file's version
/// history (newest first) in seahub's `FileHistoryView` `{data, page, page_next}`
/// envelope, always one page.
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

/// `GET /api/v2.1/repos/{repo_id}/file/revision/?p=&commit_id=` — the file content
/// at the given commit, for download or preview (seahub's `FileRevision`).
pub async fn get_file_revision_v21(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
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
    let filename = p.rsplit_once('/').map(|(_, n)| n).unwrap_or("download");
    let block_ids = file_data.block_ids;
    let total_size = file_data.size.max(0) as u64;
    let etag = crate::http::download::blocks_etag(&block_ids);
    Ok(crate::http::download::file_download_response(
        crate::http::download::FileDownloadParams {
            repo_id,
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: dec_key,
            total_size,
            content_type: base::mime::mime_guess(filename),
            content_disposition: Some(crate::http::download::content_disposition(filename, true)),
            range_header: headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            etag: Some(etag),
            head_only: false,
            // The URL names a commit, so its bytes never change.
            cache: crate::http::download::CachePolicy::MaxAge(3600),
        },
        &headers,
    ))
}

#[derive(Deserialize)]
pub struct RestoreQuery {
    pub p: Option<String>,
    pub commit_id: Option<String>,
}

/// `POST .../file/revision/restore/?p=&commit_id=` — point the file's dirent at the
/// target commit's fs_id and commit that change.
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
        state.config().storage.default_user_quota_bytes,
    )
    .await?;

    Ok(ok_json())
}

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
