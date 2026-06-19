use axum::Router;
use axum::routing::{delete, get, post, put};
use std::sync::Arc;

use crate::AppState;

pub mod activities;
pub mod async_batch;
pub mod batch;
pub mod dir;
pub mod file;
pub mod links;
pub mod metadata;
pub mod repo_set_password;
pub mod repos;
pub mod smart_link;
pub mod starred;
pub mod task_manager;
pub mod user_avatar;
pub mod wiki;

/// Routes for the `/api/v2.1/` prefix.
pub fn api_v21_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Repos
        .route("/api/v2.1/repos/", get(repos::list_repos_v21))
        .route(
            "/api/v2.1/repos/{repo_id}/",
            get(repos::get_repo_v21).delete(repos::delete_repo_v21),
        )
        // Dir
        .route(
            "/api/v2.1/repos/{repo_id}/{obj}/",
            delete(dir::delete_dirent_v21),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/dir/detail/",
            get(dir::dir_detail_v21),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/dir/",
            get(dir::list_dir_v21)
                .post(dir::create_dir_v21)
                .delete(dir::delete_dir_v21),
        )
        // Starred
        .route(
            "/api/v2.1/starred-items/",
            get(starred::get_starred_items)
                .post(starred::star_item)
                .delete(starred::unstar_item),
        )
        // Share Links
        .route(
            "/api/v2.1/share-links/",
            get(links::list_share_links_v21).post(links::create_share_link_v21),
        )
        .route(
            "/api/v2.1/share-links/{token}/",
            delete(links::delete_share_link_v21),
        )
        // Upload Links
        .route(
            "/api/v2.1/upload-links/",
            get(links::list_upload_links_v21).post(links::create_upload_link_v21),
        )
        .route(
            "/api/v2.1/upload-links/{id}/",
            delete(links::delete_upload_link_v21),
        )
        // Activities
        .route("/api/v2.1/activities/", get(activities::get_activities))
        // Wikis
        .route("/api/v2.1/wikis/", get(wiki::list_wikis))
        .route("/api/v2.1/wikis2/", get(wiki::list_wikis))
        .route(
            "/api/v2.1/wiki2/{wiki_id}/",
            put(wiki::rename_wiki).delete(wiki::delete_wiki),
        )
        .route(
            "/api/v2.1/wiki2/{wiki_id}/publish/",
            post(wiki::publish_wiki).delete(wiki::unpublish_wiki),
        )
        // Batch
        .route(
            "/api/v2.1/repos/sync-batch-move-item/",
            post(batch::batch_move_items),
        )
        .route(
            "/api/v2.1/repos/sync-batch-copy-item/",
            post(batch::sync_batch_copy_item),
        )
        .route(
            "/api/v2.1/repos/batch-delete-item/",
            post(batch::batch_delete_item),
        )
        // Async batch copy/move
        .route(
            "/api/v2.1/repos/async-batch-copy-item/",
            post(async_batch::async_batch_copy_item),
        )
        .route(
            "/api/v2.1/repos/async-batch-move-item/",
            post(async_batch::async_batch_move_item),
        )
        .route(
            "/api/v2.1/copy-move-task/",
            post(async_batch::copy_move_task),
        )
        .route(
            "/api/v2.1/query-copy-move-progress/",
            get(async_batch::query_copy_move_progress),
        )
        // File uploaded bytes (for resumable upload)
        .route(
            "/api/v2.1/repos/{repo_id}/file-uploaded-bytes/",
            get(file::file_uploaded_bytes),
        )
        // File (create=POST, delete via file wrapper since axum prefers static segments over captures)
        .route(
            "/api/v2.1/repos/{repo_id}/file/",
            post(file::create_file_v21).delete(file::delete_file_v21),
        )
        // Metadata
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/",
            get(metadata::get_metadata_config).put(metadata::update_metadata_config),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/tags/",
            get(metadata::get_file_tags),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/file-tags/",
            put(metadata::update_file_tags),
        )
        .route(
            "/api/v2.1/repos/{repo_id}/related-users/",
            get(metadata::related_users),
        )
        // Search file (v2.1)
        .route("/api/v2.1/search-file", get(crate::api::search::search))
        // User avatar upload
        .route("/api/v2.1/user-avatar/", post(user_avatar::upload_avatar))
        // Set/change repo password for encrypted repos
        .route(
            "/api/v2.1/repos/{repo_id}/set-password/",
            post(repo_set_password::set_password_v21).put(repo_set_password::change_password_v21),
        )
        // Smart link
        .route("/api/v2.1/smart-link/", get(smart_link::get_smart_link))
        // Custom share permissions
        .route(
            "/api/v2.1/repos/{repo_id}/custom-share-permissions/",
            get(metadata::custom_share_permissions),
        )
        // SDoc upload image
        .route(
            "/api/v2.1/seadoc/upload-image/{sdoc_uuid}/",
            post(metadata::seadoc_upload_image),
        )
        // Metadata record
        .route(
            "/api/v2.1/repos/{repo_id}/metadata/record/",
            get(metadata::get_metadata_record).put(metadata::update_metadata_record),
        )
}
