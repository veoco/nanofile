/// Web UI module — a Seahub-compatible management interface.
pub mod activities;
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
pub mod trash;
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
            "/libraries/{id}/rename/",
            axum::routing::post(repos::rename_repo),
        )
        .route(
            "/libraries/{id}/delete/",
            axum::routing::post(repos::delete_repo),
        )
        // Trash — global trash page (sidebar entry)
        .route("/trash/", get(trash::trash_list_page))
        .route(
            "/trash/restore/",
            axum::routing::post(trash::restore_trash_item),
        )
        .route("/trash/clean/", axum::routing::post(trash::clean_trash))
        // Library file browser — root and sub-paths
        .route("/libraries/{id}/file", get(files::file_browser_root))
        .route("/libraries/{id}/file/{*path}", get(files::file_browser))
        // File actions — all under /libraries/{id}/file/
        .route(
            "/libraries/{id}/file/upload/",
            axum::routing::post(files::upload_file),
        )
        .route(
            "/libraries/{id}/file/download/{*path}",
            get(files::download_file),
        )
        .route(
            "/libraries/{id}/file/delete/",
            axum::routing::post(files::delete_entry),
        )
        .route(
            "/libraries/{id}/file/new-dir/",
            axum::routing::post(files::create_directory),
        )
        .route(
            "/libraries/{id}/file/rename/",
            axum::routing::post(files::rename_entry),
        )
        .route(
            "/libraries/{id}/file/preview/{*path}",
            get(files::preview_file),
        )
        // Shares
        .route("/shares/", get(shares::list_shares))
        .route("/shares/create/", axum::routing::post(shares::create_share))
        .route(
            "/shares/{token}/delete/",
            axum::routing::post(shares::delete_share),
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
        // Starred items
        .route(
            "/starred/",
            get(starred::starred_page).post(starred::unstar_item_ui),
        )
        // File activities
        .route("/activities/", get(activities::activities_page))
        // Search
        .route("/search/", get(search::search_page))
}
