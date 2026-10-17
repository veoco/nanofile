use axum::Router;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::AppState;

pub mod download;
pub mod progress;
pub mod share_view;
pub mod temp_file;
pub mod upload;
pub mod upload_link_view;
pub mod zip_download;

/// The page routes (share link, upload link, a file download opened in the app):
/// visited by a browser, so a failure renders the error page (see `ui::error_page`).
pub fn web_page_routes() -> Router<Arc<AppState>> {
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
            "/u/{token}",
            get(upload_link_view::upload_link_view).post(upload_link_view::upload_link_view_post),
        )
        .route(
            "/u/{token}/",
            get(upload_link_view::upload_link_view).post(upload_link_view::upload_link_view_post),
        )
}

/// The endpoints the frontend calls itself (chunked uploads, block uploads, progress,
/// the zip task); they keep their wire bodies because the browser parses them.
pub fn web_api_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/upload-aj/", post(upload::upload_aj))
        .route("/upload-aj/{token}", post(upload::upload_aj_token))
        .route("/upload-api/{token}", post(upload::upload_api))
        .route("/download-api/{token}", get(download::download_api))
        // Raw file content, opened by a file-list link or the media preview element; it
        // belongs to the wire group because the page group's error middleware rewrites
        // 4xx bodies to HTML, which dropped `Content-Range` on a 416.
        .route(
            "/repos/{repo_id}/files/{*path}",
            get(download::repo_file_download),
        )
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
            // Pure metadata: a parent dir plus a bounded list of names, so cap the
            // body far below the app-wide JSON limit.
            post(zip_download::zip_task_handler).layer(axum::extract::DefaultBodyLimit::max(
                crate::handler::MAX_SMALL_BODY_BYTES,
            )),
        )
        .route("/zip/{token}", get(zip_download::zip_download_handler))
}
