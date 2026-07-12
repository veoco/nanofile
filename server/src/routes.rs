//! Consolidated route definitions for all API versions.
//!
//! Replaces the old `api/` and `api_v21/` shim modules that only re-exported
//! handler functions from the domain-oriented handler modules.

use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::{delete, get, post, put};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

// ── V2 (legacy) API routes ────────────────────────────────────────────────

fn v1_routes() -> Router<Arc<AppState>> {
    let repo_all = crate::handler::repos::repo_routes()
        .merge(crate::handler::file::file_routes())
        .merge(crate::handler::dir::dir_routes());

    Router::new()
        .nest("/api2", crate::handler::login::auth_routes())
        .route(
            "/api2/repos/",
            get(crate::handler::repos::list_repos).post(crate::handler::repos::create_repo),
        )
        .nest("/api2/repos", repo_all)
        .nest("/api2/repos", crate::handler::fileops::fileops_routes())
        .nest(
            "/api2/repos",
            crate::handler::chunked_upload::chunked_upload_routes(),
        )
        .nest("/api2/shared-links", crate::handler::share::share_routes())
        .nest(
            "/api2/upload-links",
            crate::handler::upload_link::upload_link_routes(),
        )
        .nest("/api2/account", crate::handler::account::account_routes())
        .route(
            "/api2/accounts/",
            post(crate::handler::account::register_user),
        )
        .nest("/api2/2fa", crate::handler::two_factor::two_factor_routes())
        .route(
            "/api2/server-info/",
            get(crate::handler::server_info::server_info),
        )
        .route("/api2/ping/", get(crate::handler::login::public_ping))
        .nest("/api2", crate::handler::starred::starred_routes())
        .nest(
            "/api2",
            crate::handler::notifications::notifications_routes(),
        )
        .route(
            "/api2/repo-tokens/",
            get(crate::handler::repos::repo_tokens),
        )
        .route(
            "/api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/",
            get(crate::handler::file::get_block_download_link),
        )
        .nest("/api2/repos", crate::handler::thumbnail::thumbnail_routes())
        .nest("/api2", crate::handler::avatar::api_routes())
        .route("/api2/groups/", get(crate::handler::groups::list_groups))
        .route(
            "/api2/groupandcontacts/",
            get(crate::handler::groups::groups_and_contacts),
        )
        .route(
            "/api2/search-user/",
            get(crate::handler::groups::search_user),
        )
        .route(
            "/api2/client-login/",
            post(crate::handler::client_login::client_login),
        )
        .route(
            "/api2/client-sso-link/",
            post(crate::handler::sso::client_sso_link),
        )
        .route(
            "/api2/client-sso-link/{token}/",
            get(crate::handler::sso::poll_sso_link),
        )
        .route(
            "/api2/device-wiped/",
            post(crate::handler::device_wipe::device_wiped),
        )
        .route("/api2/search/", get(crate::handler::search::search))
        .route("/api2/reindex/", post(crate::handler::reindex::reindex))
        .nest("/api2/admin", crate::handler::users::admin_user_routes())
        .route(
            "/api2/index-file-text/",
            post(crate::handler::reindex::index_file_text),
        )
        .route(
            "/api2/beshared-repos/{repo_id}/",
            get(crate::handler::share::list_share_members)
                .post(crate::handler::share::beshare_repo)
                .put(crate::handler::share::modify_share_permission)
                .delete(crate::handler::share::delete_share),
        )
        .nest("/api2", crate::handler::history::repo_history_routes())
        .nest("/api2", crate::handler::devices::devices_routes())
}

// ── V2.1 API routes ──────────────────────────────────────────────────────

fn v2_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Repos
        .route(
            "/api/v2.1/repos/",
            get(crate::handler::repos::list_repos_v21),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/",
            get(crate::handler::repos::get_repo_v21).delete(crate::handler::repos::delete_repo_v21),
        )
        // Dir
        .route(
            "/api/v2.1/repos/{repo_id}/dir/detail/",
            get(crate::handler::dir::dir_detail_v21),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/dir/",
            get(crate::handler::dir::list_dir_v21)
                .post(crate::handler::dir::create_dir_v21)
                .delete(v2_delete_dir),
        )
        // Starred
        .route(
            "/api/v2.1/starred-items/",
            get(crate::handler::starred::get_starred_items)
                .post(crate::handler::starred::star_item)
                .delete(crate::handler::starred::unstar_item),
        )
        // Share Links
        .route(
            "/api/v2.1/share-links/",
            get(crate::handler::links::list_share_links_v21)
                .post(crate::handler::links::create_share_link_v21),
        )
        .route(
            "/api/v2.1/share-links/{token}/",
            put(crate::handler::links::update_share_link_v21)
                .delete(crate::handler::links::delete_share_link_v21),
        )
        // Upload Links
        .route(
            "/api/v2.1/upload-links/",
            get(crate::handler::links::list_upload_links_v21)
                .post(crate::handler::links::create_upload_link_v21),
        )
        .route(
            "/api/v2.1/upload-links/clean-invalid/",
            delete(crate::handler::links::clean_invalid_upload_links_v21),
        )
        .route(
            "/api/v2.1/upload-links/{token}/",
            get(crate::handler::links::get_upload_link_v21)
                .put(crate::handler::links::update_upload_link_v21)
                .delete(crate::handler::links::delete_upload_link_v21),
        )
        .route(
            "/api/v2.1/upload-links/{token}/upload/",
            get(crate::handler::links::get_upload_link_upload_url_v21),
        )
        // Repo-level upload links
        .route(
            "/api/v2.1/repos/{repo_id}/upload-links/",
            get(crate::handler::links::list_repo_upload_links_v21),
        )
        // Activities
        .route(
            "/api/v2.1/activities/",
            get(crate::handler::activities::get_activities),
        )
        // Wikis
        .route("/api/v2.1/wikis/", get(crate::handler::wiki::list_wikis))
        .route("/api/v2.1/wikis2/", get(crate::handler::wiki::list_wikis))
        .route(
            "/api/v2.1/wiki2/{wiki_id}/",
            put(crate::handler::wiki::rename_wiki).delete(crate::handler::wiki::delete_wiki),
        )
        .route(
            "/api/v2.1/wiki2/{wiki_id}/publish/",
            post(crate::handler::wiki::publish_wiki).delete(crate::handler::wiki::unpublish_wiki),
        )
        // Batch
        .route(
            "/api/v2.1/repos/sync-batch-move-item/",
            post(crate::handler::batch::batch_move_items),
        )
        .route(
            "/api/v2.1/repos/sync-batch-copy-item/",
            post(crate::handler::batch::sync_batch_copy_item),
        )
        .route(
            "/api/v2.1/repos/batch-delete-item/",
            post(crate::handler::batch::batch_delete_item),
        )
        // Async batch copy/move
        .route(
            "/api/v2.1/repos/async-batch-copy-item/",
            post(crate::handler::async_batch::async_batch_copy_item),
        )
        .route(
            "/api/v2.1/repos/async-batch-move-item/",
            post(crate::handler::async_batch::async_batch_move_item),
        )
        .route(
            "/api/v2.1/copy-move-task/",
            post(crate::handler::async_batch::copy_move_task),
        )
        .route(
            "/api/v2.1/query-copy-move-progress/",
            get(crate::handler::async_batch::query_copy_move_progress),
        )
        // File uploaded bytes (for resumable upload)
        .route(
            "/api/v2.1/repos/{repo_id}/file-uploaded-bytes/",
            get(crate::handler::file::file_uploaded_bytes),
        )
        // File (create=POST, delete delegated to v2 handler)
        .route(
            "/api/v2.1/repos/{repo_id}/file/",
            post(crate::handler::file::create_file_v21).delete(v2_delete_file),
        )
        // Metadata
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/",
            get(crate::handler::metadata::get_metadata_config)
                .put(crate::handler::metadata::update_metadata_config),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/tags/",
            get(crate::handler::metadata::get_file_tags),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/file-tags/",
            put(crate::handler::metadata::update_file_tags),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/related-users/",
            get(crate::handler::metadata::related_users),
        )
        // Search file (v2.1)
        .route("/api/v2.1/search-file", get(crate::handler::search::search))
        // User avatar upload
        .route(
            "/api/v2.1/user-avatar/",
            post(crate::handler::user_avatar::upload_avatar),
        )
        // Set/change repo password for encrypted repos
        .route(
            "/api/v2.1/repos/{repo_id}/set-password/",
            post(crate::handler::password::set_password_v21)
                .put(crate::handler::password::change_password_v21),
        )
        // Smart link
        .route(
            "/api/v2.1/smart-link/",
            get(crate::handler::smart_link::get_smart_link),
        )
        // Custom share permissions
        .route(
            "/api/v2.1/repos/{repo_id}/custom-share-permissions/",
            get(crate::handler::metadata::custom_share_permissions),
        )
        // SDoc upload image
        .route(
            "/api/v2.1/seadoc/upload-image/{sdoc_uuid}/",
            post(crate::handler::metadata::seadoc_upload_image),
        )
        // Metadata record
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/record/",
            get(crate::handler::metadata::get_metadata_record)
                .put(crate::handler::metadata::update_metadata_record),
        )
        // Trash
        .route(
            "/api/v2.1/repos/{repo_id}/trash/",
            get(crate::handler::trash::list_trash).delete(crate::handler::trash::clean_trash),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash/revert-dirents/",
            post(crate::handler::trash::revert_dirents),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash2/",
            get(crate::handler::trash::list_trash2),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash2/search/",
            get(crate::handler::trash::search_trash),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash2/revert/",
            post(crate::handler::trash::revert_trash),
        )
        .route(
            "/api/v2.1/deleted-repos/",
            get(crate::handler::trash::list_deleted_repos)
                .post(crate::handler::trash::restore_deleted_repo),
        )
}

// ── Inline handlers for v2.1 routes that need extra logic ─────────────────

async fn v2_delete_dir(
    access: crate::middleware::repo_extractor::RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    let p = query
        .get("p")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let v2_query = crate::handler::dir::DirQuery {
        p: Some(p),
        t: None,
        recursive: None,
    };
    crate::handler::dir::delete_dir(
        access,
        axum::extract::State(state),
        axum::extract::Query(v2_query),
    )
    .await?;
    Ok(Json(serde_json::json!({"success": true})))
}

async fn v2_delete_file(
    access: crate::middleware::repo_extractor::RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    let p = query
        .get("p")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let v2_query = crate::handler::file::FileQuery {
        p: Some(p),
        reuse: None,
    };
    crate::handler::file::delete_file(
        access,
        axum::extract::State(state),
        axum::extract::Query(v2_query),
    )
    .await?;
    Ok(Json(serde_json::json!({"success": true})))
}

// ── Combined API routes (V1 + V2.1) ───────────────────────────────────────

/// All HTTP API routes: legacy v1 (`/api2/`) and v2.1 (`/api/v2.1/`).
pub fn api_routes() -> Router<Arc<AppState>> {
    v1_routes().merge(v2_routes())
}
