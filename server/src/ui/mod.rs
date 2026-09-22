/// Web UI module — a Seahub-compatible management interface.
pub mod activities;
pub mod adminshares;
pub mod admintasks;
pub mod api_keys;
pub mod auth;
pub mod auth_extractor;
pub mod banner;
pub mod client_login;
pub mod ctx;
pub mod email;
pub mod error_page;
pub mod files;
pub mod invitations;
pub mod left_panel_cache;
pub mod repos;
pub mod search;
pub mod settings;
pub mod shares;
pub mod sso;
pub mod starred;
pub mod sysadmin;
pub mod system_settings;
pub mod trash;
pub mod two_factor;
pub mod user_agent;

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
        .route(
            "/client-login/",
            get(client_login::client_token_login).post(client_login::client_token_login_confirm),
        )
        // SSO local-browser — browser entry + confirm pages
        .route("/client-sso/{token}/", get(sso::client_sso))
        .route(
            "/client-sso/{token}/complete/",
            get(sso::client_sso_complete_page).post(sso::client_sso_complete),
        )
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
        // Seahub-compatible library URL: the desktop client's "view on website"
        // opens `/library/<repo-id>/<name>/…` (`repo-tree-view.cpp:578`), so the
        // singular spelling has to resolve for that flow to work. The id is
        // authoritative (the name segment is decorative) and both are redirected
        // to this server's own `/libraries/{id}/files/…` path.
        .route("/library/{id}/{name}", get(files::library_redirect_root))
        .route("/library/{id}/{name}/", get(files::library_redirect_root))
        .route(
            "/library/{id}/{name}/{*path}",
            get(files::library_redirect_path),
        )
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
        // Profile / Settings — one shell, entered from any of its pages.
        .route("/settings/", get(settings::settings_page))
        .route("/settings/profile/", get(settings::profile_page))
        .route("/settings/security/", get(settings::security_page))
        // The credential inventory. `/settings/devices/` is the path this page
        // used to live at; it redirects so old links and bookmarks still land.
        .route("/settings/credentials/", get(settings::credentials_page))
        .route(
            "/settings/credentials/revoke/",
            axum::routing::post(settings::revoke_credential),
        )
        .route(
            "/settings/credentials/bulk/",
            axum::routing::post(settings::revoke_bulk),
        )
        .route(
            "/settings/credentials/unlink/",
            axum::routing::post(settings::unlink_device),
        )
        .route(
            "/settings/devices/",
            get(settings::redirect_credentials).post(settings::unlink_device),
        )
        .route(
            "/settings/devices/revoke/",
            axum::routing::post(settings::revoke_credential),
        )
        .route(
            "/settings/password/",
            axum::routing::post(settings::change_password),
        )
        .route(
            "/settings/profile/display-name/",
            axum::routing::post(settings::update_display_name),
        )
        .route(
            "/settings/profile/language/",
            axum::routing::post(settings::update_language),
        )
        // Legacy profile-form paths, kept as aliases so existing tests and
        // bookmarked pages keep working.
        .route(
            "/settings/display-name/",
            axum::routing::post(settings::update_display_name),
        )
        .route(
            "/settings/language/",
            axum::routing::post(settings::update_language),
        )
        // Unified API keys
        .route("/settings/api-keys/", get(api_keys::list_page))
        .route(
            "/settings/api-keys/create/",
            axum::routing::post(api_keys::create),
        )
        .route(
            "/settings/api-keys/{id}/update/",
            axum::routing::post(api_keys::update),
        )
        .route(
            "/settings/api-keys/{id}/revoke/",
            axum::routing::post(api_keys::revoke),
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
        // Avatar upload (the legacy path stays an alias of the profile one)
        .route(
            "/settings/profile/avatar/",
            axum::routing::post(settings::upload_avatar),
        )
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
        .route(
            "/settings/two-factor/qr-code/",
            get(two_factor::qr_code_image),
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
        // Admin — task management
        .route("/sysadmin/tasks/", get(admintasks::task_list_page))
        .route(
            "/sysadmin/tasks/{name}/trigger/",
            axum::routing::post(admintasks::trigger_task),
        )
        // Admin — system management: every setting that can be managed at
        // runtime, one page per area. `{section}` is validated against the
        // catalog, so an unknown one lands on the first page rather than 404ing.
        .route("/sysadmin/settings/", get(system_settings::settings_page))
        .route(
            "/sysadmin/settings/{section}/",
            get(system_settings::settings_page),
        )
        .route(
            "/sysadmin/settings/{section}/save/",
            axum::routing::post(system_settings::save),
        )
        .route(
            "/sysadmin/settings/reset/",
            axum::routing::post(system_settings::reset),
        )
        .route(
            "/sysadmin/settings/refresh/",
            axum::routing::post(system_settings::refresh),
        )
        // Admin — email management (outbox, delivery state, test message).
        // The SMTP configuration itself lives under /sysadmin/settings/email/.
        .route("/sysadmin/email/", get(email::email_page))
        .route(
            "/sysadmin/email/test/",
            axum::routing::post(email::send_test),
        )
        .route(
            "/sysadmin/email/drain/",
            axum::routing::post(email::drain_now),
        )
        .route(
            "/sysadmin/email/clear/",
            axum::routing::post(email::clear_finished),
        )
        .route(
            "/sysadmin/email/{id}/retry/",
            axum::routing::post(email::retry_message),
        )
        .route(
            "/sysadmin/email/{id}/delete/",
            axum::routing::post(email::delete_message),
        )
}

// ── Shared display helpers ─────────────────────────────────────────────

/// Format a byte count as a human-readable size (`B`/`KB`/`MB`/`GB`/`TB`).
pub use infra::common::util::format_size;
