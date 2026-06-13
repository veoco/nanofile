/// Web UI module — a Seahub-compatible management interface.
pub mod auth;
pub mod auth_extractor;
pub mod client_login;
pub mod files;
pub mod invitations;
pub mod repos;
pub mod search;
pub mod settings;
pub mod shares;
pub mod starred;
pub mod two_factor;

use axum::Router;
use axum::routing::get;
use std::sync::Arc;

use crate::AppState;

/// Build the web UI route tree with Seahub-compatible paths.
pub fn ui_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Auth — Seahub path /accounts/login/
        .route("/accounts/login/", get(auth::login_page).post(auth::login))
        .route(
            "/accounts/two-factor-auth/",
            get(auth::two_factor_auth_page).post(auth::two_factor_auth),
        )
        .route("/accounts/logout/", get(auth::logout))
        // Registration — invitation-only
        .route(
            "/accounts/register/",
            get(auth::register_page).post(auth::register),
        )
        // Password reset
        .route(
            "/accounts/password/reset/",
            get(auth::password_reset_page).post(auth::password_reset),
        )
        .route(
            "/accounts/password/reset/done/",
            get(auth::password_reset_done),
        )
        .route(
            "/accounts/password/reset/{token}/",
            get(auth::password_reset_confirm_page).post(auth::password_reset_confirm),
        )
        .route(
            "/accounts/password/reset/complete/",
            get(auth::password_reset_complete),
        )
        // Client-login — auto-login from desktop client
        .route("/client-login/", get(client_login::client_token_login))
        // Libraries — root and listing
        .route(
            "/",
            get(|| async { axum::response::Redirect::to("/libraries/") }),
        )
        .route("/libraries/", get(repos::list_repos))
        // Library CRUD (web UI)
        .route(
            "/libraries/create/",
            axum::routing::post(repos::create_repo),
        )
        .route(
            "/libraries/{id}/rename",
            axum::routing::post(repos::rename_repo),
        )
        .route(
            "/libraries/{id}/delete",
            axum::routing::post(repos::delete_repo),
        )
        // Library file browser — /library/{repo_id}/{repo_name}/{*path}
        .route("/library/{id}/{*path}", get(files::file_browser_seahub))
        // Seahub-compatible file view — /lib/{repo_id}/file{*path}
        .route("/lib/{id}/file{*path}", get(files::view_lib_file))
        // Library actions
        .route(
            "/library/{id}/upload",
            axum::routing::post(files::upload_file),
        )
        .route("/library/{id}/download/{*path}", get(files::download_file))
        .route(
            "/library/{id}/delete",
            axum::routing::post(files::delete_entry),
        )
        .route(
            "/library/{id}/new-dir",
            axum::routing::post(files::create_directory),
        )
        .route(
            "/library/{id}/rename",
            axum::routing::post(files::rename_entry),
        )
        .route("/library/{id}/preview/{*path}", get(files::preview_file))
        // Shares
        .route("/share/", get(shares::list_shares))
        .route("/share/create", axum::routing::post(shares::create_share))
        .route(
            "/share/{token}/delete",
            axum::routing::post(shares::delete_share),
        )
        // Profile / Settings
        .route("/profile/", get(settings::settings_page))
        .route(
            "/profile/devices/",
            get(settings::devices_page).post(settings::unlink_device),
        )
        .route(
            "/profile/password",
            axum::routing::post(settings::change_password),
        )
        // Invitation codes
        .route(
            "/profile/invitations/",
            get(invitations::list_invitations).post(invitations::generate_invitation),
        )
        .route(
            "/profile/invitations/{id}/delete",
            axum::routing::post(invitations::delete_invitation),
        )
        // Two-Factor Authentication
        .route("/profile/two-factor/", get(two_factor::setup_page))
        .route(
            "/profile/two-factor/setup",
            axum::routing::post(two_factor::setup_2fa),
        )
        .route(
            "/profile/two-factor/verify",
            axum::routing::post(two_factor::verify_2fa),
        )
        .route(
            "/profile/two-factor/disable",
            axum::routing::post(two_factor::disable_2fa),
        )
        // Starred items
        .route(
            "/starred/",
            get(starred::starred_page).post(starred::unstar_item_ui),
        )
        // Search
        .route("/search", get(search::search_page))
}
