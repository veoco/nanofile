pub mod account;
pub mod auth;
pub mod avatar;
pub mod client_login;
pub mod device_wipe;
pub mod devices;
pub mod dir;
pub mod file;
pub mod groups;
pub mod notifications;
pub mod reindex;
pub mod repo_history;
pub mod repos;
pub mod search;
pub mod server_info;
pub mod share;
pub mod sso;
pub mod starred;
pub mod thumbnail;
pub mod two_factor;
pub mod upload_link;

use axum::Router;

use crate::AppState;
use std::sync::Arc;

pub fn api_routes() -> Router<Arc<AppState>> {
    let repo_all = repos::repo_routes()
        .merge(file::file_routes())
        .merge(dir::dir_routes());

    Router::new()
        .nest("/api2", auth::auth_routes())
        .route(
            "/api2/repos/",
            axum::routing::get(repos::list_repos).post(repos::create_repo),
        )
        .nest("/api2/repos", repo_all)
        .nest("/api2/shared-links", share::share_routes())
        .nest("/api2/upload-links", upload_link::upload_link_routes())
        .nest("/api2/account", account::account_routes())
        .route(
            "/api2/accounts/",
            axum::routing::post(account::register_user),
        )
        .nest("/api2/2fa", two_factor::two_factor_routes())
        .route(
            "/api2/server-info/",
            axum::routing::get(server_info::server_info),
        )
        .route("/api2/ping/", axum::routing::get(auth::ping))
        .nest("/api2", starred::starred_routes())
        .nest("/api2", notifications::notifications_routes())
        .route("/api2/repo-tokens/", axum::routing::get(repos::repo_tokens))
        .route(
            "/api2/default-repo/",
            axum::routing::get(repos::get_default_repo).post(repos::create_default_repo),
        )
        .nest("/api2/repos", thumbnail::thumbnail_routes())
        .nest("/api2", avatar::avatar_routes())
        .route("/api2/groups/", axum::routing::get(groups::list_groups))
        .route(
            "/api2/groupandcontacts/",
            axum::routing::get(groups::groups_and_contacts),
        )
        .route(
            "/api2/search-user/",
            axum::routing::get(groups::search_user),
        )
        .route(
            "/api2/client-login/",
            axum::routing::post(client_login::client_login),
        )
        .route(
            "/api2/client-sso-link/",
            axum::routing::post(sso::client_sso_link),
        )
        .route(
            "/api2/client-sso-link/{token}/",
            axum::routing::get(sso::poll_sso_link),
        )
        .route(
            "/api2/device-wiped/",
            axum::routing::post(device_wipe::device_wiped),
        )
        .route("/api2/search/", axum::routing::get(search::search))
        .route("/api2/reindex/", axum::routing::post(reindex::reindex))
        .route(
            "/api2/index-file-text/",
            axum::routing::post(reindex::index_file_text),
        )
        .route(
            "/api2/beshared-repos/{repo_id}/",
            axum::routing::post(share::beshare_repo),
        )
        .nest("/api2", repo_history::repo_history_routes())
        .nest("/api2", devices::devices_routes())
}
