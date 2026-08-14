use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub per_page: Option<i32>,
    pub page: Option<i32>,
    pub search_repo: Option<String>,
    pub search_filename_only: Option<bool>,
}

#[derive(serde::Serialize)]
pub struct SearchResponse {
    pub results: Vec<serde_json::Value>,
    pub total: i32,
    pub has_more: bool,
}

#[derive(Deserialize)]
pub struct SearchFileQuery {
    pub repo_id: String,
    pub q: String,
}

/// `GET /api/v2.1/search-file/` — search files/folders within a single repo.
///
/// Returns the seahub-compatible shape `{"data": [{"path","size","mtime","type"}]}`
/// where `type` is `"file"`/`"folder"` and `mtime` is an ISO-8601 string.
/// This is distinct from the legacy `/api2/search/` (repo-spanning) endpoint.
pub async fn search_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchFileQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let q = query.q.trim().to_string();
    if q.is_empty() {
        return Err(AppError::BadRequest("q invalid.".into()));
    }

    let svc = state.search_service();
    // Filename-only search scoped to the single repo; fetch a large page and
    // paginate the reshaped result below.
    let (results, _total, _has_more) = svc
        .search(&q, auth.user_id, 1000, 1, Some(&query.repo_id), true)
        .await?;

    let mut data: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let is_dir = r["is_dir"].as_bool().unwrap_or(false);
            let mtime = r["mtime"]
                .as_i64()
                .or_else(|| r["last_modified"].as_i64())
                .unwrap_or(0);
            serde_json::json!({
                "path": r["fullpath"].as_str().unwrap_or(""),
                "size": r["size"].as_i64().unwrap_or(0),
                "mtime": infra::common::util::timestamp_rfc3339(mtime),
                "type": if is_dir { "folder" } else { "file" },
            })
        })
        .collect();

    // Folders first, then files, each sorted by mtime descending (seahub order).
    data.sort_by(|a, b| {
        let (ad, bd) = (a["type"] == "folder", b["type"] == "folder");
        bd.cmp(&ad).then_with(|| {
            b["mtime"]
                .as_str()
                .unwrap_or("")
                .cmp(a["mtime"].as_str().unwrap_or(""))
        })
    });

    Ok(Json(serde_json::json!({ "data": data })))
}

pub async fn search(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, AppError> {
    let q = query.q.unwrap_or_default().trim().to_string();
    let per_page = query.per_page.unwrap_or(10).max(1);
    let page = query.page.unwrap_or(1).max(1);
    let search_filename_only = query.search_filename_only.unwrap_or(false);

    let svc = state.search_service();
    let (results, total, has_more) = svc
        .search(
            &q,
            auth.user_id,
            per_page,
            page,
            query.search_repo.as_deref(),
            search_filename_only,
        )
        .await?;

    Ok(Json(SearchResponse {
        results,
        total,
        has_more,
    }))
}
