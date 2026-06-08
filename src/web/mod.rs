use axum::Router;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::AppState;

pub mod download;
pub mod progress;
pub mod upload;

/// Routes for web file access.
pub fn web_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/f/{token}", get(download::shared_file_download))
        .route("/f/{token}/", get(download::shared_file_download))
        .route(
            "/repos/{repo_id}/files/{*path}",
            get(download::repo_file_download),
        )
        .route("/upload-aj/", post(upload::upload_aj))
        .route("/upload-aj/{token}", post(upload::upload_aj_token))
        .route("/upload-api/{token}", post(upload::upload_api))
        .route("/update-aj/", post(upload::update_aj))
        .route("/update-aj/{token}", post(upload::update_aj_token))
        .route("/update-api/", post(upload::update_api))
        .route("/update-api/{token}", post(upload::update_api_handler))
        .route("/upload_progress", get(progress::upload_progress))
        .route("/idx_progress", get(progress::idx_progress))
}
