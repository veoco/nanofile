use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

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

pub async fn search(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, AppError> {
    let q = query.q.unwrap_or_default().trim().to_string();
    let per_page = query.per_page.unwrap_or(10).max(1);
    let page = query.page.unwrap_or(1).max(1);
    let search_filename_only = query.search_filename_only.unwrap_or(true);

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
