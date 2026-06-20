//! Consolidated route definitions for all API versions.
//!
//! Replaces the old `api/` and `api_v21/` shim modules that only re-exported
//! handler functions from the domain-oriented handler modules.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::error::AppError;

// ── V2 (legacy) API routes ────────────────────────────────────────────────

fn v1_routes() -> Router<Arc<AppState>> {
    let repo_all = crate::repo::handler::repos::repo_routes()
        .merge(crate::fs::handler::file::file_routes())
        .merge(crate::fs::handler::dir::dir_routes());

    Router::new()
        .nest("/api2", crate::auth::handler::login::auth_routes())
        .route(
            "/api2/repos/",
            get(crate::repo::handler::repos::list_repos)
                .post(crate::repo::handler::repos::create_repo),
        )
        .nest("/api2/repos", repo_all)
        .nest("/api2/repos", crate::fs::handler::fileops::fileops_routes())
        .nest(
            "/api2/repos",
            crate::fs::handler::chunked_upload::chunked_upload_routes(),
        )
        .nest(
            "/api2/shared-links",
            crate::sharing::handler::share::share_routes(),
        )
        .nest(
            "/api2/upload-links",
            crate::sharing::handler::upload_link::upload_link_routes(),
        )
        .nest(
            "/api2/account",
            crate::user::handler::account::account_routes(),
        )
        .route(
            "/api2/accounts/",
            post(crate::user::handler::account::register_user),
        )
        .nest(
            "/api2/2fa",
            crate::auth::handler::two_factor::two_factor_routes(),
        )
        .route(
            "/api2/server-info/",
            get(crate::admin::handler::server_info::server_info),
        )
        .route("/api2/ping/", get(crate::auth::handler::login::public_ping))
        .nest("/api2", crate::fs::handler::starred::starred_routes())
        .nest(
            "/api2",
            crate::activity::handler::notifications::notifications_routes(),
        )
        .route(
            "/api2/repo-tokens/",
            get(crate::repo::handler::repos::repo_tokens),
        )
        .route(
            "/api2/default-repo/",
            get(crate::repo::handler::repos::get_default_repo)
                .post(crate::repo::handler::repos::create_default_repo),
        )
        .route(
            "/api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/",
            get(crate::fs::handler::file::get_block_download_link),
        )
        .nest(
            "/api2/repos",
            crate::fs::handler::thumbnail::thumbnail_routes(),
        )
        .nest("/api2", crate::user::handler::avatar::api_routes())
        .route(
            "/api2/groups/",
            get(crate::sharing::handler::groups::list_groups),
        )
        .route(
            "/api2/groupandcontacts/",
            get(crate::sharing::handler::groups::groups_and_contacts),
        )
        .route(
            "/api2/search-user/",
            get(crate::sharing::handler::groups::search_user),
        )
        .route(
            "/api2/client-login/",
            post(crate::auth::handler::client_login::client_login),
        )
        .route(
            "/api2/client-sso-link/",
            post(crate::auth::handler::sso::client_sso_link),
        )
        .route(
            "/api2/client-sso-link/{token}/",
            get(crate::auth::handler::sso::poll_sso_link),
        )
        .route(
            "/api2/device-wiped/",
            post(crate::auth::handler::device_wipe::device_wiped),
        )
        .route("/api2/search/", get(crate::fs::handler::search::search))
        .route(
            "/api2/reindex/",
            post(crate::admin::handler::reindex::reindex),
        )
        .route(
            "/api2/index-file-text/",
            post(crate::admin::handler::reindex::index_file_text),
        )
        .route(
            "/api2/beshared-repos/{repo_id}/",
            get(crate::sharing::handler::share::list_share_members)
                .post(crate::sharing::handler::share::beshare_repo)
                .put(crate::sharing::handler::share::modify_share_permission)
                .delete(crate::sharing::handler::share::delete_share),
        )
        .nest(
            "/api2",
            crate::repo::handler::history::repo_history_routes(),
        )
        .nest("/api2", crate::user::handler::devices::devices_routes())
}

// ── V2.1 API routes ──────────────────────────────────────────────────────

fn v2_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Repos
        .route(
            "/api/v2.1/repos/",
            get(crate::repo::handler::repos::list_repos_v21),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/",
            get(crate::repo::handler::repos::get_repo_v21)
                .delete(crate::repo::handler::repos::delete_repo_v21),
        )
        // Dir
        .route(
            "/api/v2.1/repos/{repo_id}/dir/detail/",
            get(crate::fs::handler::dir::dir_detail_v21),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/dir/",
            get(crate::fs::handler::dir::list_dir_v21)
                .post(crate::fs::handler::dir::create_dir_v21)
                .delete(v2_delete_dir),
        )
        // Starred
        .route(
            "/api/v2.1/starred-items/",
            get(crate::fs::handler::starred::get_starred_items)
                .post(crate::fs::handler::starred::star_item)
                .delete(crate::fs::handler::starred::unstar_item),
        )
        // Share Links
        .route(
            "/api/v2.1/share-links/",
            get(crate::sharing::handler::links::list_share_links_v21)
                .post(crate::sharing::handler::links::create_share_link_v21),
        )
        .route(
            "/api/v2.1/share-links/{token}/",
            delete(crate::sharing::handler::links::delete_share_link_v21),
        )
        // Upload Links
        .route(
            "/api/v2.1/upload-links/",
            get(crate::sharing::handler::links::list_upload_links_v21)
                .post(crate::sharing::handler::links::create_upload_link_v21),
        )
        .route(
            "/api/v2.1/upload-links/{id}/",
            delete(crate::sharing::handler::links::delete_upload_link_v21),
        )
        // Activities
        .route(
            "/api/v2.1/activities/",
            get(crate::activity::handler::activities::get_activities),
        )
        // Wikis
        .route(
            "/api/v2.1/wikis/",
            get(crate::sharing::handler::wiki::list_wikis),
        )
        .route(
            "/api/v2.1/wikis2/",
            get(crate::sharing::handler::wiki::list_wikis),
        )
        .route(
            "/api/v2.1/wiki2/{wiki_id}/",
            put(crate::sharing::handler::wiki::rename_wiki)
                .delete(crate::sharing::handler::wiki::delete_wiki),
        )
        .route(
            "/api/v2.1/wiki2/{wiki_id}/publish/",
            post(crate::sharing::handler::wiki::publish_wiki)
                .delete(crate::sharing::handler::wiki::unpublish_wiki),
        )
        // Batch
        .route(
            "/api/v2.1/repos/sync-batch-move-item/",
            post(crate::fs::handler::batch::batch_move_items),
        )
        .route(
            "/api/v2.1/repos/sync-batch-copy-item/",
            post(crate::fs::handler::batch::sync_batch_copy_item),
        )
        .route(
            "/api/v2.1/repos/batch-delete-item/",
            post(crate::fs::handler::batch::batch_delete_item),
        )
        // Async batch copy/move
        .route(
            "/api/v2.1/repos/async-batch-copy-item/",
            post(crate::fs::handler::async_batch::async_batch_copy_item),
        )
        .route(
            "/api/v2.1/repos/async-batch-move-item/",
            post(crate::fs::handler::async_batch::async_batch_move_item),
        )
        .route(
            "/api/v2.1/copy-move-task/",
            post(crate::fs::handler::async_batch::copy_move_task),
        )
        .route(
            "/api/v2.1/query-copy-move-progress/",
            get(crate::fs::handler::async_batch::query_copy_move_progress),
        )
        // File uploaded bytes (for resumable upload)
        .route(
            "/api/v2.1/repos/{repo_id}/file-uploaded-bytes/",
            get(crate::fs::handler::file::file_uploaded_bytes),
        )
        // File (create=POST, delete delegated to v2 handler)
        .route(
            "/api/v2.1/repos/{repo_id}/file/",
            post(crate::fs::handler::file::create_file_v21).delete(v2_delete_file),
        )
        // Metadata
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/",
            get(crate::fs::handler::metadata::get_metadata_config)
                .put(crate::fs::handler::metadata::update_metadata_config),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/tags/",
            get(crate::fs::handler::metadata::get_file_tags),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/file-tags/",
            put(crate::fs::handler::metadata::update_file_tags),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/related-users/",
            get(crate::fs::handler::metadata::related_users),
        )
        // Search file (v2.1)
        .route(
            "/api/v2.1/search-file",
            get(crate::fs::handler::search::search),
        )
        // User avatar upload
        .route(
            "/api/v2.1/user-avatar/",
            post(crate::user::handler::user_avatar::upload_avatar),
        )
        // Set/change repo password for encrypted repos
        .route(
            "/api/v2.1/repos/{repo_id}/set-password/",
            post(crate::repo::handler::password::set_password_v21)
                .put(crate::repo::handler::password::change_password_v21),
        )
        // Smart link
        .route(
            "/api/v2.1/smart-link/",
            get(crate::sharing::handler::smart_link::get_smart_link),
        )
        // Custom share permissions
        .route(
            "/api/v2.1/repos/{repo_id}/custom-share-permissions/",
            get(crate::fs::handler::metadata::custom_share_permissions),
        )
        // SDoc upload image
        .route(
            "/api/v2.1/seadoc/upload-image/{sdoc_uuid}/",
            post(crate::fs::handler::metadata::seadoc_upload_image),
        )
        // Metadata record
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/record/",
            get(crate::fs::handler::metadata::get_metadata_record)
                .put(crate::fs::handler::metadata::update_metadata_record),
        )
        // Trash
        .route(
            "/api/v2.1/repos/{repo_id}/trash/",
            get(crate::fs::handler::trash::list_trash)
                .delete(crate::fs::handler::trash::clean_trash),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash/revert-dirents/",
            post(crate::fs::handler::trash::revert_dirents),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash2/",
            get(crate::fs::handler::trash::list_trash2),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash2/search/",
            get(crate::fs::handler::trash::search_trash),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/trash2/revert/",
            post(crate::fs::handler::trash::revert_trash),
        )
        .route(
            "/api/v2.1/deleted-repos/",
            get(crate::fs::handler::trash::list_deleted_repos)
                .post(crate::fs::handler::trash::restore_deleted_repo),
        )
}

// ── Inline handlers for v2.1 routes that need extra logic ─────────────────

async fn v2_delete_dir(
    auth: crate::auth::middleware::AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    let p = query
        .get("p")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let v2_query = crate::fs::handler::dir::DirQuery {
        p: Some(p),
        t: None,
        recursive: None,
    };
    crate::fs::handler::dir::delete_dir(
        auth,
        axum::extract::State(state),
        axum::extract::Path(repo_id),
        axum::extract::Query(v2_query),
    )
    .await?;
    Ok(Json(serde_json::json!({"success": true})))
}

async fn v2_delete_file(
    auth: crate::auth::middleware::AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    let p = query
        .get("p")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let v2_query = crate::fs::handler::file::FileQuery {
        p: Some(p),
        reuse: None,
    };
    crate::fs::handler::file::delete_file(
        auth,
        axum::extract::State(state),
        axum::extract::Path(repo_id),
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
