//! File serving and transfer handlers.
//!
//! Three route groups live here. [`web_page_routes`] serves the HTML pages a
//! browser opens for a share or an upload link. [`web_api_routes`] serves the
//! fileserver/seahub wire paths: the URLs the API hands out and the official
//! clients POST to, so they stay exactly where Seafile registers them.
//! [`nanofile_transfer_routes`] is the Nanofile-only remainder, mounted under
//! `/api/transfers/` (see `crate::routes::nanofile_routes`) so the root surface
//! keeps reading as Seafile's own route table.

use axum::Router;
use axum::routing::{get, post};
use std::sync::Arc;

use crate::AppState;

pub mod download;
pub mod link_gate;
pub mod progress;
pub mod share_view;
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

/// The endpoints the frontend calls itself (chunked uploads, block uploads, the
/// zip task); they keep their wire bodies because the browser parses them.
///
/// Every path here is also registered by Seafile's fileserver or seahub; the
/// Nanofile-only transfer endpoints live in [`nanofile_transfer_routes`].
pub fn web_api_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/upload-aj/{token}", post(upload::upload_aj_token))
        .route("/upload-api/{token}", post(upload::upload_api))
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
        // The raw-block half of the upload-blocks link (mobile/iOS): the same
        // block-write phase without a commit, one `file` part per block.
        .route(
            "/upload-raw-blks-api/{token}",
            post(upload::upload_blks_api),
        )
        // Upstream seahub's `update-blks-link/` mints this path; the token op
        // (`update-blks`) makes the shared handler replace an existing file.
        .route("/update-blks-api/{token}", post(upload::upload_blks_api))
        .route("/update-aj/{token}", post(upload::update_aj_token))
        .route("/update-api/{token}", post(upload::update_api_handler))
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

/// The Nanofile-only transfer endpoints, mounted under `/api/transfers/`.
///
/// None of these paths exists in Seafile's fileserver or seahub route tables:
/// the session upload/update forms, the token download (the fileserver serves
/// `/files/{token}/{filename}` instead) and the two progress shims are this
/// server's own additions. The token-upload family (`/upload-api/{token}`,
/// `/upload-aj/{token}`, `/update-api/{token}`, the block endpoints) stays on the
/// root because Seafile's own link endpoints mint those URLs and the official
/// clients POST to them verbatim.
pub fn nanofile_transfer_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/download/{token}", get(download::download_api))
        // The no-token browser form: the session is the credential, so it has no
        // Seafile counterpart.
        .route("/upload/", post(upload::upload_aj))
        // The session counterpart of `/update-api/{token}` and
        // `/update-aj/{token}`; it accepts either target spelling.
        .route("/update/", post(upload::update_session))
        // Compatibility shims for the web interface's progress polling. Uploads
        // are synchronous per request, so both always answer 100%.
        .route("/upload-progress/", get(progress::upload_progress))
        .route("/index-progress/", get(progress::idx_progress))
}
