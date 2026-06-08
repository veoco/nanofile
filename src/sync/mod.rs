pub mod accessible_repos;
pub mod block;
pub mod commit;
pub mod folder_perm;
pub mod fs;
pub mod head_commits;
pub mod jwt_token;
pub mod lock;
pub mod permission;
pub mod protocol;
pub mod quota;

use axum::Router;

use crate::AppState;
use std::sync::Arc;

pub fn sync_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/seafhttp/protocol-version",
            axum::routing::get(protocol::protocol_version),
        )
        .nest("/seafhttp/repo", commit::commit_routes())
        .nest("/seafhttp/repo", fs::fs_routes())
        .nest("/seafhttp/repo", block::block_routes())
        .nest("/seafhttp/repo", permission::permission_routes())
        .nest("/seafhttp/repo", jwt_token::jwt_token_routes())
        .nest("/seafhttp/repo", quota::quota_routes())
        .nest("/seafhttp/repo", lock::lock_routes())
        .nest("/seafhttp/repo", folder_perm::folder_perm_routes())
        .nest("/seafhttp", accessible_repos::accessible_repos_routes())
        .route(
            "/seafhttp/repo/head-commits-multi/",
            axum::routing::post(head_commits::head_commits_multi),
        )
}
