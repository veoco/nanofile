/// Web UI module — a Seahub-compatible management interface.
pub mod activities;
pub mod adminshares;
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
pub mod sysadmin;
pub mod trash;
pub mod two_factor;

use axum::Router;
use axum::routing::get;
use std::sync::Arc;

use crate::AppState;

/// Build the web UI route tree.
///
/// Web UI routes serve only GET requests (page browsing, file preview, download).
/// All mutation operations (rename, delete, upload, etc.) are handled by the
/// frontend JavaScript calling the Seafile-compatible API (`/api/v2.1/`, `/api2/`).
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
        // Libraries — root redirect and listing (GET only)
        .route(
            "/",
            get(|| async { axum::response::Redirect::to("/libraries/") }),
        )
        .route("/libraries/", get(repos::list_repos))
        // Trash — global trash page (GET only)
        .route("/trash/", get(trash::trash_list_page))
        // Library file browser — root and sub-paths (GET only)
        // Both trailing-slash variants are registered explicitly because
        // NormalizePathLayer::trim_trailing_slash() in tower-http 0.7 doesn't
        // reliably rewrite the URI before axum 0.8's router sees it.
        .route("/libraries/{id}/files", get(files::file_browser_root))
        .route("/libraries/{id}/files/", get(files::file_browser_root))
        .route("/libraries/{id}/files/{*path}", get(files::file_browser))
        // Shares — page listing (GET only)
        .route("/shares/", get(shares::list_shares))
        .route("/shares/create/", axum::routing::post(shares::create_share))
        .route(
            "/shares/{token}/delete/",
            axum::routing::post(shares::delete_share),
        )
        .route(
            "/shares/upload/{token}/delete/",
            axum::routing::post(shares::delete_upload),
        )
        // Profile / Settings
        .route("/settings/", get(settings::settings_page))
        .route(
            "/settings/devices/",
            get(settings::devices_page).post(settings::unlink_device),
        )
        .route(
            "/settings/password/",
            axum::routing::post(settings::change_password),
        )
        .route(
            "/settings/display-name/",
            axum::routing::post(settings::update_display_name),
        )
        // Invitation codes
        .route(
            "/settings/invitations/",
            get(invitations::list_invitations).post(invitations::generate_invitation),
        )
        .route(
            "/settings/invitations/{id}/delete/",
            axum::routing::post(invitations::delete_invitation),
        )
        // Avatar upload
        .route(
            "/settings/avatar/",
            axum::routing::post(settings::upload_avatar),
        )
        // Two-Factor Authentication
        .route("/settings/two-factor/", get(two_factor::setup_page))
        .route(
            "/settings/two-factor/setup/",
            axum::routing::post(two_factor::setup_2fa),
        )
        .route(
            "/settings/two-factor/verify/",
            axum::routing::post(two_factor::verify_2fa),
        )
        .route(
            "/settings/two-factor/disable/",
            axum::routing::post(two_factor::disable_2fa),
        )
        // Starred items — page listing only (GET).
        // Star/unstar is handled by JS via /api/v2.1/starred-items/.
        .route("/starred/", get(starred::starred_page))
        // File activities
        .route("/activities/", get(activities::activities_page))
        // Search
        .route("/search/", get(search::search_page))
        // Admin — user management
        .route("/sysadmin/users/", get(sysadmin::sysadmin_page))
        .route(
            "/sysadmin/users/create/",
            axum::routing::post(sysadmin::create_user),
        )
        .route(
            "/sysadmin/users/{user_id}/update/",
            axum::routing::post(sysadmin::update_user),
        )
        .route(
            "/sysadmin/users/{user_id}/delete/",
            axum::routing::post(sysadmin::delete_user),
        )
        // Admin — share management
        .route("/sysadmin/shares/", get(adminshares::list_all_shares))
        .route(
            "/sysadmin/shares/share/{token}/delete/",
            axum::routing::post(adminshares::delete_share),
        )
        .route(
            "/sysadmin/shares/upload/{token}/delete/",
            axum::routing::post(adminshares::delete_upload),
        )
}
