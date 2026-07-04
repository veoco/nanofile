use axum::Router;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::AppState;

pub mod download;
pub mod progress;
pub mod share_view;
pub mod temp_file;
pub mod upload;
pub mod zip_download;

/// Routes for web file access.
pub fn web_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/f/{token}",
            get(share_view::shared_file_view).post(share_view::shared_file_view_post),
        )
        .route(
            "/f/{token}/",
            get(share_view::shared_file_view).post(share_view::shared_file_view_post),
        )
        .route(
            "/d/{token}",
            get(share_view::shared_dir_view).post(share_view::shared_dir_view_post),
        )
        .route(
            "/d/{token}/",
            get(share_view::shared_dir_view).post(share_view::shared_dir_view_post),
        )
        .route(
            "/d/{token}/files/{*path}",
            get(share_view::shared_dir_file_view),
        )
        .route(
            "/repos/{repo_id}/files/{*path}",
            get(download::repo_file_download),
        )
        .route("/upload-aj/", post(upload::upload_aj))
        .route("/upload-aj/{token}", post(upload::upload_aj_token))
        .route("/upload-api/{token}", post(upload::upload_api))
        .route("/download-api/{token}", get(download::download_api))
        .route(
            "/blks/{token}/{file_id}/{block_id}",
            get(download::block_download),
        )
        .route("/upload-blks-api/{token}", post(upload::upload_blks_api))
        .route("/update-aj/", post(upload::update_aj))
        .route("/update-aj/{token}", post(upload::update_aj_token))
        .route("/update-api/", post(upload::update_api))
        .route("/update-api/{token}", post(upload::update_api_handler))
        .route("/upload_progress", get(progress::upload_progress))
        .route("/idx_progress", get(progress::idx_progress))
        .route(
            "/api/v2.1/repos/{repo_id}/zip-task/",
            post(zip_download::zip_task_handler),
        )
        .route("/zip/{token}", get(zip_download::zip_download_handler))
}
