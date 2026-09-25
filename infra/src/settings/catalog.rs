//! The settings catalog: one entry per `Config` field.
//!
//! Every entry names its `NANOFILE_*` variable, how the value is parsed, and
//! when a change takes effect ([`Apply`]). The environment layer is applied
//! *from* this table, so a setting cannot be added without wiring its variable,
//! and the admin UI cannot advertise a variable the server does not read.
//!
//! Entries are grouped by the admin page they appear on ([`Section`]) and, on a
//! page, by the heading they are read under ([`GROUPS`]). This file therefore
//! reads top to bottom like the pages do; where a group's order disagrees with
//! the config file's, the page wins, because the page is what an operator reads.
//! [`GROUPS`] names the same keys a second time, and the
//! `the_groups_partition_the_catalog` test is what keeps the two lists honest.

use std::path::PathBuf;

use super::{
    Apply, Hook, Kind, Section, SettingDef, fmt_bool, fmt_num_list, fmt_opt_bool, fmt_text_list,
    opt_text, parse_bool, parse_enum, parse_i32, parse_num_list, parse_opt_bool, parse_text_list,
    parse_u16, parse_u32, parse_u64, parse_usize,
};

/// Declare one catalog entry.
///
/// The two-arm form carries a `*_FILE` variable for a secret, which is preferred
/// over the plain variable because it keeps the value out of a process listing.
macro_rules! setting {
    ($key:literal, $section:ident, $env:literal, $kind:expr, $apply:expr, $get:expr, $set:expr) => {
        setting!($key, $section, $env, None, $kind, $apply, $get, $set)
    };
    ($key:literal, $section:ident, $env:literal, $env_file:expr, $kind:expr, $apply:expr, $get:expr, $set:expr) => {
        SettingDef {
            key: $key,
            section: Section::$section,
            env: Some($env),
            env_file: $env_file,
            kind: $kind,
            apply: $apply,
            get: $get,
            set: $set,
        }
    };
}

/// Every `Config` field, in admin-page order.
pub static CATALOG: &[SettingDef] = &[
    // ── Server ──────────────────────────────────────────────────────────────
    // · Addresses & identity
    setting!(
        "server.addr",
        Server,
        "NANOFILE_SERVER_ADDR",
        Kind::Text,
        Apply::Restart,
        |c| c.server.addr.clone(),
        |c, v| {
            c.server.addr = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "server.port",
        Server,
        "NANOFILE_SERVER_PORT",
        Kind::U16,
        Apply::Restart,
        |c| c.server.port.to_string(),
        |c, v| {
            c.server.port = parse_u16(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.version",
        Server,
        "NANOFILE_SERVER_VERSION",
        Kind::Text,
        Apply::Live,
        |c| c.server.version.clone(),
        |c, v| {
            c.server.version = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "server.site_url",
        Server,
        "NANOFILE_SERVER_SITE_URL",
        Kind::Text,
        Apply::Live,
        |c| c.server.site_url.clone(),
        |c, v| {
            c.server.site_url = v.trim().to_string();
            Ok(())
        }
    ),
    // · Uploads & requests
    setting!(
        "server.max_upload_size_mb",
        Server,
        "NANOFILE_SERVER_MAX_UPLOAD_SIZE_MB",
        Kind::U64,
        Apply::Restart,
        |c| c.server.max_upload_size_mb.to_string(),
        |c, v| {
            c.server.max_upload_size_mb = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.max_json_body_mb",
        Server,
        "NANOFILE_SERVER_MAX_JSON_BODY_MB",
        Kind::U64,
        Apply::Restart,
        |c| c.server.max_json_body_mb.to_string(),
        |c, v| {
            c.server.max_json_body_mb = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.max_chunk_size_mb",
        Server,
        "NANOFILE_SERVER_MAX_CHUNK_SIZE_MB",
        Kind::U64,
        Apply::Live,
        |c| c.server.max_chunk_size_mb.to_string(),
        |c, v| {
            c.server.max_chunk_size_mb = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.request_timeout_secs",
        Server,
        "NANOFILE_SERVER_REQUEST_TIMEOUT_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.server.request_timeout_secs.to_string(),
        |c, v| {
            c.server.request_timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.header_read_timeout_secs",
        Server,
        "NANOFILE_SERVER_HEADER_READ_TIMEOUT_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.server.header_read_timeout_secs.to_string(),
        |c, v| {
            c.server.header_read_timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.body_timeout_secs",
        Server,
        "NANOFILE_SERVER_BODY_TIMEOUT_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.server.body_timeout_secs.to_string(),
        |c, v| {
            c.server.body_timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    // · CORS
    setting!(
        "server.cors_allowed_origins",
        Server,
        "NANOFILE_CORS_ALLOWED_ORIGINS",
        Kind::TextList,
        Apply::Restart,
        |c| fmt_text_list(&c.server.cors_allowed_origins),
        |c, v| {
            c.server.cors_allowed_origins = parse_text_list(v);
            Ok(())
        }
    ),
    setting!(
        "server.cors_max_age_secs",
        Server,
        "NANOFILE_CORS_MAX_AGE_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.server.cors_max_age_secs.to_string(),
        |c, v| {
            c.server.cors_max_age_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    // · Desktop & tray
    setting!(
        "server.desktop_custom_brand",
        Server,
        "NANOFILE_SERVER_DESKTOP_CUSTOM_BRAND",
        Kind::OptText,
        Apply::Live,
        |c| c.server.desktop_custom_brand.clone().unwrap_or_default(),
        |c, v| {
            c.server.desktop_custom_brand = opt_text(v);
            Ok(())
        }
    ),
    setting!(
        "server.desktop_custom_logo",
        Server,
        "NANOFILE_SERVER_DESKTOP_CUSTOM_LOGO",
        Kind::OptText,
        Apply::Live,
        |c| c.server.desktop_custom_logo.clone().unwrap_or_default(),
        |c, v| {
            c.server.desktop_custom_logo = opt_text(v);
            Ok(())
        }
    ),
    setting!(
        "server.tray",
        Server,
        "NANOFILE_SERVER_TRAY",
        Kind::Bool,
        Apply::ProcessRestart,
        |c| fmt_bool(c.server.tray),
        |c, v| {
            c.server.tray = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "ui.tray_language",
        Server,
        "NANOFILE_UI_TRAY_LANGUAGE",
        Kind::Enum(&["auto", "en", "zh"]),
        Apply::ProcessRestart,
        |c| c.ui.tray_language.clone(),
        |c, v| {
            c.ui.tray_language = parse_enum(v, &["auto", "en", "zh"])?;
            Ok(())
        }
    ),
    // · Web interface
    setting!(
        "ui.default_language",
        Server,
        "NANOFILE_UI_DEFAULT_LANGUAGE",
        Kind::Enum(&["en", "zh"]),
        Apply::Live,
        |c| c.ui.default_language.clone(),
        |c, v| {
            c.ui.default_language = parse_enum(v, &["en", "zh"])?;
            Ok(())
        }
    ),
    // ── Security ────────────────────────────────────────────────────────────
    // · Master secret
    setting!(
        "server.secret_key",
        Security,
        "NANOFILE_SERVER_SECRET_KEY",
        Kind::Secret,
        Apply::ReadOnly,
        |c| c.server.secret_key.clone(),
        |c, v| {
            c.server.secret_key = v.trim().to_string();
            Ok(())
        }
    ),
    // · Capabilities
    setting!(
        "server.webdav_enabled",
        Security,
        "NANOFILE_SERVER_WEBDAV_ENABLED",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.server.webdav_enabled),
        |c, v| {
            c.server.webdav_enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.sso_enabled",
        Security,
        "NANOFILE_SERVER_SSO_ENABLED",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.server.sso_enabled),
        |c, v| {
            c.server.sso_enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.file_search_enabled",
        Security,
        "NANOFILE_SERVER_FILE_SEARCH_ENABLED",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.server.file_search_enabled),
        |c, v| {
            c.server.file_search_enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.share_link_enabled",
        Security,
        "NANOFILE_SERVER_SHARE_LINK_ENABLED",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.server.share_link_enabled),
        |c, v| {
            c.server.share_link_enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    // · Hosts & proxies
    setting!(
        "server.trusted_proxies",
        Security,
        "NANOFILE_SERVER_TRUSTED_PROXIES",
        Kind::TextList,
        Apply::Live,
        |c| fmt_text_list(&c.server.trusted_proxies),
        |c, v| {
            c.server.trusted_proxies = parse_text_list(v);
            Ok(())
        }
    ),
    setting!(
        "server.allowed_hosts",
        Security,
        "NANOFILE_SERVER_ALLOWED_HOSTS",
        Kind::TextList,
        Apply::Live,
        |c| fmt_text_list(&c.server.allowed_hosts),
        |c, v| {
            c.server.allowed_hosts = parse_text_list(v);
            Ok(())
        }
    ),
    setting!(
        "server.trust_request_host",
        Security,
        "NANOFILE_SERVER_TRUST_REQUEST_HOST",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.server.trust_request_host),
        |c, v| {
            c.server.trust_request_host = parse_bool(v)?;
            Ok(())
        }
    ),
    // · Response headers & request caps
    setting!(
        "server.hsts_include_subdomains",
        Security,
        "NANOFILE_SERVER_HSTS_INCLUDE_SUBDOMAINS",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.server.hsts_include_subdomains),
        |c, v| {
            c.server.hsts_include_subdomains = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.max_propfind_entries",
        Security,
        "NANOFILE_SERVER_MAX_PROPFIND_ENTRIES",
        Kind::Usize,
        Apply::Live,
        |c| c.server.max_propfind_entries.to_string(),
        |c, v| {
            c.server.max_propfind_entries = parse_usize(v)?;
            Ok(())
        }
    ),
    // ── Authentication ──────────────────────────────────────────────────────
    // · Passwords
    setting!(
        "auth.password_hash_iterations",
        Authentication,
        "NANOFILE_AUTH_PASSWORD_HASH_ITERATIONS",
        Kind::U32,
        Apply::Live,
        |c| c.auth.password_hash_iterations.to_string(),
        |c, v| {
            c.auth.password_hash_iterations = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.password_min_length",
        Authentication,
        "NANOFILE_AUTH_PASSWORD_MIN_LENGTH",
        Kind::U32,
        Apply::Live,
        |c| c.auth.password_min_length.to_string(),
        |c, v| {
            c.auth.password_min_length = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.require_strong_password",
        Authentication,
        "NANOFILE_AUTH_REQUIRE_STRONG_PASSWORD",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.auth.require_strong_password),
        |c, v| {
            c.auth.require_strong_password = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.enable_password_reset",
        Authentication,
        "NANOFILE_AUTH_ENABLE_PASSWORD_RESET",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.auth.enable_password_reset),
        |c, v| {
            c.auth.enable_password_reset = parse_bool(v)?;
            Ok(())
        }
    ),
    // · Registration
    setting!(
        "auth.enable_invitations",
        Authentication,
        "NANOFILE_AUTH_ENABLE_INVITATIONS",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.auth.enable_invitations),
        |c, v| {
            c.auth.enable_invitations = parse_bool(v)?;
            Ok(())
        }
    ),
    // · Token lifetimes
    setting!(
        "auth.api_token_ttl_days",
        Authentication,
        "NANOFILE_AUTH_API_TOKEN_TTL_DAYS",
        Kind::U64,
        Apply::Live,
        |c| c.auth.api_token_ttl_days.to_string(),
        |c, v| {
            c.auth.api_token_ttl_days = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.sync_token_ttl_days",
        Authentication,
        "NANOFILE_AUTH_SYNC_TOKEN_TTL_DAYS",
        Kind::U64,
        Apply::Live,
        |c| c.auth.sync_token_ttl_days.to_string(),
        |c, v| {
            c.auth.sync_token_ttl_days = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.api_key_ttl_presets_days",
        Authentication,
        "NANOFILE_AUTH_API_KEY_TTL_PRESETS_DAYS",
        Kind::NumList,
        Apply::Live,
        |c| fmt_num_list(&c.auth.api_key_ttl_presets_days),
        |c, v| {
            c.auth.api_key_ttl_presets_days = parse_num_list(v);
            Ok(())
        }
    ),
    setting!(
        "auth.api_key_max_ttl_days",
        Authentication,
        "NANOFILE_AUTH_API_KEY_MAX_TTL_DAYS",
        Kind::U64,
        Apply::Live,
        |c| c.auth.api_key_max_ttl_days.to_string(),
        |c, v| {
            c.auth.api_key_max_ttl_days = parse_u64(v)?;
            Ok(())
        }
    ),
    // ── Rate limits ────────────────────────────────────────────────────────
    // · Sign-in
    setting!(
        "auth.max_login_attempts",
        RateLimits,
        "NANOFILE_AUTH_MAX_LOGIN_ATTEMPTS",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.max_login_attempts.to_string(),
        |c, v| {
            c.auth.max_login_attempts = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.lockout_duration_secs",
        RateLimits,
        "NANOFILE_AUTH_LOCKOUT_DURATION_SECS",
        Kind::U64,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.lockout_duration_secs.to_string(),
        |c, v| {
            c.auth.lockout_duration_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.max_distinct_usernames_per_ip",
        RateLimits,
        "NANOFILE_AUTH_MAX_DISTINCT_USERNAMES_PER_IP",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.max_distinct_usernames_per_ip.to_string(),
        |c, v| {
            c.auth.max_distinct_usernames_per_ip = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.totp_max_attempts",
        RateLimits,
        "NANOFILE_AUTH_TOTP_MAX_ATTEMPTS",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.totp_max_attempts.to_string(),
        |c, v| {
            c.auth.totp_max_attempts = parse_u32(v)?;
            Ok(())
        }
    ),
    // · Account flows
    setting!(
        "auth.password_reset_max_per_hour",
        RateLimits,
        "NANOFILE_AUTH_PASSWORD_RESET_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.password_reset_max_per_hour.to_string(),
        |c, v| {
            c.auth.password_reset_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.registration_max_per_hour",
        RateLimits,
        "NANOFILE_AUTH_REGISTRATION_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.registration_max_per_hour.to_string(),
        |c, v| {
            c.auth.registration_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.sso_link_max_per_hour",
        RateLimits,
        "NANOFILE_AUTH_SSO_LINK_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.sso_link_max_per_hour.to_string(),
        |c, v| {
            c.auth.sso_link_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    // · Protected resources
    setting!(
        "auth.link_password_max_per_hour",
        RateLimits,
        "NANOFILE_AUTH_LINK_PASSWORD_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.link_password_max_per_hour.to_string(),
        |c, v| {
            c.auth.link_password_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.repo_password_max_per_hour",
        RateLimits,
        "NANOFILE_AUTH_REPO_PASSWORD_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.repo_password_max_per_hour.to_string(),
        |c, v| {
            c.auth.repo_password_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.webdav_max_failures_per_5min",
        RateLimits,
        "NANOFILE_AUTH_WEBDAV_MAX_FAILURES_PER_5MIN",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.webdav_max_failures_per_5min.to_string(),
        |c, v| {
            c.auth.webdav_max_failures_per_5min = parse_u32(v)?;
            Ok(())
        }
    ),
    // · Content & search
    setting!(
        "auth.share_download_max_per_minute",
        RateLimits,
        "NANOFILE_AUTH_SHARE_DOWNLOAD_MAX_PER_MINUTE",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.share_download_max_per_minute.to_string(),
        |c, v| {
            c.auth.share_download_max_per_minute = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.search_max_per_minute",
        RateLimits,
        "NANOFILE_AUTH_SEARCH_MAX_PER_MINUTE",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.search_max_per_minute.to_string(),
        |c, v| {
            c.auth.search_max_per_minute = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.reindex_max_per_hour",
        RateLimits,
        "NANOFILE_AUTH_REINDEX_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.reindex_max_per_hour.to_string(),
        |c, v| {
            c.auth.reindex_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    // ── Storage ─────────────────────────────────────────────────────────────
    // · Directories
    setting!(
        "storage.block_dir",
        Storage,
        "NANOFILE_STORAGE_BLOCK_DIR",
        Kind::Path,
        Apply::Restart,
        |c| c.storage.block_dir.display().to_string(),
        |c, v| {
            c.storage.block_dir = PathBuf::from(v.trim());
            Ok(())
        }
    ),
    setting!(
        "storage.temp_dir",
        Storage,
        "NANOFILE_STORAGE_TEMP_DIR",
        Kind::Path,
        Apply::Restart,
        |c| c.storage.temp_dir.display().to_string(),
        |c, v| {
            c.storage.temp_dir = PathBuf::from(v.trim());
            Ok(())
        }
    ),
    setting!(
        "storage.thumbnail_dir",
        Storage,
        "NANOFILE_STORAGE_THUMBNAIL_DIR",
        Kind::Path,
        Apply::Restart,
        |c| c.storage.thumbnail_dir.display().to_string(),
        |c, v| {
            c.storage.thumbnail_dir = PathBuf::from(v.trim());
            Ok(())
        }
    ),
    setting!(
        "storage.avatar_dir",
        Storage,
        "NANOFILE_STORAGE_AVATAR_DIR",
        Kind::Path,
        Apply::Restart,
        |c| c.storage.avatar_dir.display().to_string(),
        |c, v| {
            c.storage.avatar_dir = PathBuf::from(v.trim());
            Ok(())
        }
    ),
    // · Quotas & resumable uploads
    setting!(
        "storage.max_storage_bytes",
        Storage,
        "NANOFILE_STORAGE_MAX_STORAGE_BYTES",
        Kind::U64,
        Apply::Live,
        |c| c.storage.max_storage_bytes.to_string(),
        |c, v| {
            c.storage.max_storage_bytes = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "storage.max_temp_uploads",
        Storage,
        "NANOFILE_STORAGE_MAX_TEMP_UPLOADS",
        Kind::U64,
        Apply::Restart,
        |c| c.storage.max_temp_uploads.to_string(),
        |c, v| {
            c.storage.max_temp_uploads = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "storage.max_temp_upload_bytes",
        Storage,
        "NANOFILE_STORAGE_MAX_TEMP_UPLOAD_BYTES",
        Kind::U64,
        Apply::Restart,
        |c| c.storage.max_temp_upload_bytes.to_string(),
        |c, v| {
            c.storage.max_temp_upload_bytes = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "storage.temp_upload_ttl_hours",
        Storage,
        "NANOFILE_STORAGE_TEMP_UPLOAD_TTL_HOURS",
        Kind::U64,
        Apply::Restart,
        |c| c.storage.temp_upload_ttl_hours.to_string(),
        |c, v| {
            c.storage.temp_upload_ttl_hours = parse_u64(v)?;
            Ok(())
        }
    ),
    // · Archives
    setting!(
        "storage.max_zip_entries",
        Storage,
        "NANOFILE_STORAGE_MAX_ZIP_ENTRIES",
        Kind::U64,
        Apply::Live,
        |c| c.storage.max_zip_entries.to_string(),
        |c, v| {
            c.storage.max_zip_entries = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "storage.max_zip_bytes",
        Storage,
        "NANOFILE_STORAGE_MAX_ZIP_BYTES",
        Kind::U64,
        Apply::Live,
        |c| c.storage.max_zip_bytes.to_string(),
        |c, v| {
            c.storage.max_zip_bytes = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "storage.max_zip_task_bytes",
        Storage,
        "NANOFILE_STORAGE_MAX_ZIP_TASK_BYTES",
        Kind::U64,
        Apply::Live,
        |c| c.storage.max_zip_task_bytes.to_string(),
        |c, v| {
            c.storage.max_zip_task_bytes = parse_u64(v)?;
            Ok(())
        }
    ),
    // · Media tools
    setting!(
        "storage.ffmpeg_path",
        Storage,
        "NANOFILE_STORAGE_FFMPEG_PATH",
        Kind::Text,
        Apply::Live,
        |c| c.storage.ffmpeg_path.clone(),
        |c, v| {
            c.storage.ffmpeg_path = v.trim().to_string();
            Ok(())
        }
    ),
    // ── Encryption ──────────────────────────────────────────────────────────
    // · Block storage
    setting!(
        "storage.block_encryption_mode",
        Encryption,
        "NANOFILE_STORAGE_BLOCK_ENCRYPTION_MODE",
        Kind::Enum(&["off", "on", "lazy"]),
        Apply::ReadOnly,
        |c| c.storage.block_encryption_mode.clone(),
        |c, v| {
            c.storage.block_encryption_mode = parse_enum(v, &["off", "on", "lazy"])?;
            Ok(())
        }
    ),
    setting!(
        "storage.encryption_key",
        Encryption,
        "NANOFILE_STORAGE_ENCRYPTION_KEY",
        Some("NANOFILE_STORAGE_ENCRYPTION_KEY_FILE"),
        Kind::Secret,
        Apply::ReadOnly,
        |c| c.storage.encryption_key.clone().unwrap_or_default(),
        |c, v| {
            c.storage.encryption_key = opt_text(v);
            Ok(())
        }
    ),
    // · Encrypted libraries
    setting!(
        "server.encrypted_library_version",
        Encryption,
        "NANOFILE_SERVER_ENCRYPTED_LIBRARY_VERSION",
        Kind::I32,
        Apply::Live,
        |c| c.server.encrypted_library_version.to_string(),
        |c, v| {
            c.server.encrypted_library_version = parse_i32(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.encrypted_library_pwd_hash_algo",
        Encryption,
        "NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_ALGO",
        Kind::Enum(&["", "pbkdf2_sha256", "argon2id"]),
        Apply::Live,
        |c| c
            .server
            .encrypted_library_pwd_hash_algo
            .clone()
            .unwrap_or_default(),
        |c, v| {
            c.server.encrypted_library_pwd_hash_algo = opt_text(v);
            Ok(())
        }
    ),
    setting!(
        "server.encrypted_library_pwd_hash_params",
        Encryption,
        "NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_PARAMS",
        Kind::OptText,
        Apply::Live,
        |c| c
            .server
            .encrypted_library_pwd_hash_params
            .clone()
            .unwrap_or_default(),
        |c, v| {
            c.server.encrypted_library_pwd_hash_params = opt_text(v);
            Ok(())
        }
    ),
    // ── Maintenance ─────────────────────────────────────────────────────────
    // · Search index
    setting!(
        "index.enabled",
        Maintenance,
        "NANOFILE_INDEX_ENABLED",
        Kind::Bool,
        Apply::Restart,
        |c| fmt_bool(c.index.enabled),
        |c, v| {
            c.index.enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "index.index_dir",
        Maintenance,
        "NANOFILE_INDEX_INDEX_DIR",
        Kind::Path,
        Apply::Restart,
        |c| c.index.index_dir.display().to_string(),
        |c, v| {
            c.index.index_dir = PathBuf::from(v.trim());
            Ok(())
        }
    ),
    setting!(
        "index.backfill_enabled",
        Maintenance,
        "NANOFILE_INDEX_BACKFILL_ENABLED",
        Kind::Bool,
        Apply::Restart,
        |c| fmt_bool(c.index.backfill_enabled),
        |c, v| {
            c.index.backfill_enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "index.sandbox",
        Maintenance,
        "NANOFILE_INDEX_SANDBOX",
        Kind::Enum(&["require", "prefer"]),
        Apply::Restart,
        |c| c.index.sandbox.clone(),
        |c, v| {
            c.index.sandbox = parse_enum(v, &["require", "prefer"])?;
            Ok(())
        }
    ),
    // · Garbage collection
    setting!(
        "gc.enabled",
        Maintenance,
        "NANOFILE_GC_ENABLED",
        Kind::Bool,
        Apply::Restart,
        |c| fmt_bool(c.gc.enabled),
        |c, v| {
            c.gc.enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "gc.interval_hours",
        Maintenance,
        "NANOFILE_GC_INTERVAL_HOURS",
        Kind::U64,
        Apply::Restart,
        |c| c.gc.interval_hours.to_string(),
        |c, v| {
            c.gc.interval_hours = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "gc.min_block_age_secs",
        Maintenance,
        "NANOFILE_GC_MIN_BLOCK_AGE_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.gc.min_block_age_secs.to_string(),
        |c, v| {
            c.gc.min_block_age_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    // · Background tasks
    setting!(
        "tasks.max_active_tasks",
        Maintenance,
        "NANOFILE_TASKS_MAX_ACTIVE",
        Kind::U64,
        Apply::LiveWithHook(Hook::TaskSystem),
        |c| c.tasks.max_active_tasks.to_string(),
        |c, v| {
            c.tasks.max_active_tasks = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "tasks.max_active_per_user",
        Maintenance,
        "NANOFILE_TASKS_MAX_ACTIVE_PER_USER",
        Kind::U64,
        Apply::LiveWithHook(Hook::TaskSystem),
        |c| c.tasks.max_active_per_user.to_string(),
        |c, v| {
            c.tasks.max_active_per_user = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "tasks.load_aware",
        Maintenance,
        "NANOFILE_TASKS_LOAD_AWARE",
        Kind::Bool,
        Apply::LiveWithHook(Hook::TaskLoad),
        |c| fmt_bool(c.tasks.load_aware),
        |c, v| {
            c.tasks.load_aware = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "tasks.load_sample_interval_secs",
        Maintenance,
        "NANOFILE_TASKS_LOAD_SAMPLE_INTERVAL_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.tasks.load_sample_interval_secs.to_string(),
        |c, v| {
            c.tasks.load_sample_interval_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "tasks.max_retained_bytes",
        Maintenance,
        "NANOFILE_TASKS_MAX_RETAINED_BYTES",
        Kind::U64,
        Apply::LiveWithHook(Hook::TaskSystem),
        |c| c.tasks.max_retained_bytes.to_string(),
        |c, v| {
            c.tasks.max_retained_bytes = parse_u64(v)?;
            Ok(())
        }
    ),
    // ── Email ───────────────────────────────────────────────────────────────
    // · Delivery
    setting!(
        "email.enabled",
        Email,
        "NANOFILE_EMAIL_ENABLED",
        Kind::Bool,
        Apply::LiveWithHook(Hook::MailDrain),
        |c| fmt_bool(c.email.enabled),
        |c, v| {
            c.email.enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "email.paused",
        Email,
        "NANOFILE_EMAIL_PAUSED",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.email.paused),
        |c, v| {
            c.email.paused = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "email.host",
        Email,
        "NANOFILE_EMAIL_HOST",
        Kind::Text,
        Apply::Live,
        |c| c.email.host.clone(),
        |c, v| {
            c.email.host = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "email.port",
        Email,
        "NANOFILE_EMAIL_PORT",
        Kind::U16,
        Apply::Live,
        |c| c.email.port.to_string(),
        |c, v| {
            c.email.port = parse_u16(v)?;
            Ok(())
        }
    ),
    setting!(
        "email.tls",
        Email,
        "NANOFILE_EMAIL_TLS",
        Kind::Enum(&["starttls", "tls", "none"]),
        Apply::Live,
        |c| c.email.tls.clone(),
        |c, v| {
            c.email.tls = parse_enum(v, &["starttls", "tls", "none"])?;
            Ok(())
        }
    ),
    setting!(
        "email.timeout_secs",
        Email,
        "NANOFILE_EMAIL_TIMEOUT_SECS",
        Kind::U64,
        Apply::Live,
        |c| c.email.timeout_secs.to_string(),
        |c, v| {
            c.email.timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "email.max_attempts",
        Email,
        "NANOFILE_EMAIL_MAX_ATTEMPTS",
        Kind::U32,
        Apply::Live,
        |c| c.email.max_attempts.to_string(),
        |c, v| {
            c.email.max_attempts = parse_u32(v)?;
            Ok(())
        }
    ),
    // · Credentials
    setting!(
        "email.username",
        Email,
        "NANOFILE_EMAIL_USERNAME",
        Kind::Text,
        Apply::Live,
        |c| c.email.username.clone(),
        |c, v| {
            c.email.username = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "email.password",
        Email,
        "NANOFILE_EMAIL_PASSWORD",
        Some("NANOFILE_EMAIL_PASSWORD_FILE"),
        Kind::Secret,
        Apply::Live,
        |c| c.email.password.clone().unwrap_or_default(),
        |c, v| {
            c.email.password = opt_text(v);
            Ok(())
        }
    ),
    // · Sender
    setting!(
        "email.from_address",
        Email,
        "NANOFILE_EMAIL_FROM_ADDRESS",
        Kind::Text,
        Apply::Live,
        |c| c.email.from_address.clone(),
        |c, v| {
            c.email.from_address = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "email.from_name",
        Email,
        "NANOFILE_EMAIL_FROM_NAME",
        Kind::Text,
        Apply::Live,
        |c| c.email.from_name.clone(),
        |c, v| {
            c.email.from_name = v.trim().to_string();
            Ok(())
        }
    ),
    // · Account notifications
    setting!(
        "email.notify_new_device",
        Email,
        "NANOFILE_EMAIL_NOTIFY_NEW_DEVICE",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.email.notify_new_device),
        |c, v| {
            c.email.notify_new_device = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "email.notify_api_key_created",
        Email,
        "NANOFILE_EMAIL_NOTIFY_API_KEY_CREATED",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.email.notify_api_key_created),
        |c, v| {
            c.email.notify_api_key_created = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "email.notify_new_login",
        Email,
        "NANOFILE_EMAIL_NOTIFY_NEW_LOGIN",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.email.notify_new_login),
        |c, v| {
            c.email.notify_new_login = parse_bool(v)?;
            Ok(())
        }
    ),
    // ── Notifications ───────────────────────────────────────────────────────
    // · Service
    setting!(
        "notification.enabled",
        Notifications,
        "NANOFILE_NOTIFICATION_ENABLED",
        Kind::Bool,
        Apply::Restart,
        |c| fmt_bool(c.notification.enabled),
        |c, v| {
            c.notification.enabled = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "notification.private_key",
        Notifications,
        "NANOFILE_NOTIFICATION_PRIVATE_KEY",
        Kind::Secret,
        Apply::Live,
        |c| c.notification.private_key.clone(),
        |c, v| {
            c.notification.private_key = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "notification.accept_legacy_event_tokens",
        Notifications,
        "NANOFILE_NOTIFICATION_ACCEPT_LEGACY_EVENT_TOKENS",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.notification.accept_legacy_event_tokens),
        |c, v| {
            c.notification.accept_legacy_event_tokens = parse_bool(v)?;
            Ok(())
        }
    ),
    // · Connections
    setting!(
        "notification.max_connections",
        Notifications,
        "NANOFILE_NOTIFICATION_MAX_CONNECTIONS",
        Kind::U64,
        Apply::LiveWithHook(Hook::NotificationManager),
        |c| c.notification.max_connections.to_string(),
        |c, v| {
            c.notification.max_connections = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "notification.max_connections_per_ip",
        Notifications,
        "NANOFILE_NOTIFICATION_MAX_CONNECTIONS_PER_IP",
        Kind::U64,
        Apply::LiveWithHook(Hook::NotificationManager),
        |c| c.notification.max_connections_per_ip.to_string(),
        |c, v| {
            c.notification.max_connections_per_ip = parse_u64(v)?;
            Ok(())
        }
    ),
    // · Timeouts
    setting!(
        "notification.ping_interval",
        Notifications,
        "NANOFILE_NOTIFICATION_PING_INTERVAL",
        Kind::U64,
        Apply::Live,
        |c| c.notification.ping_interval.to_string(),
        |c, v| {
            c.notification.ping_interval = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "notification.client_timeout",
        Notifications,
        "NANOFILE_NOTIFICATION_CLIENT_TIMEOUT",
        Kind::U64,
        Apply::Live,
        |c| c.notification.client_timeout.to_string(),
        |c, v| {
            c.notification.client_timeout = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "notification.subscribe_timeout_secs",
        Notifications,
        "NANOFILE_NOTIFICATION_SUBSCRIBE_TIMEOUT_SECS",
        Kind::U64,
        Apply::Live,
        |c| c.notification.subscribe_timeout_secs.to_string(),
        |c, v| {
            c.notification.subscribe_timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    // ── Advanced ────────────────────────────────────────────────────────────
    // · Database
    setting!(
        "database.url",
        Advanced,
        "NANOFILE_DATABASE_URL",
        Kind::Text,
        Apply::ReadOnly,
        |c| c.database.url.clone(),
        |c, v| {
            c.database.url = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "database.max_connections",
        Advanced,
        "NANOFILE_DATABASE_MAX_CONNECTIONS",
        Kind::U32,
        Apply::ReadOnly,
        |c| c.database.max_connections.to_string(),
        |c, v| {
            c.database.max_connections = parse_u32(v)?;
            Ok(())
        }
    ),
    // · First-run bootstrap
    setting!(
        "admin_init.email",
        Advanced,
        "NANOFILE_ADMIN_INIT_EMAIL",
        Kind::OptText,
        Apply::ReadOnly,
        |c| c.admin_init.email.clone().unwrap_or_default(),
        |c, v| {
            c.admin_init.email = opt_text(v);
            Ok(())
        }
    ),
    setting!(
        "admin_init.password",
        Advanced,
        "NANOFILE_ADMIN_INIT_PASSWORD",
        Some("NANOFILE_ADMIN_INIT_PASSWORD_FILE"),
        Kind::Secret,
        Apply::ReadOnly,
        |c| c.admin_init.password.clone().unwrap_or_default(),
        |c, v| {
            c.admin_init.password = opt_text(v);
            Ok(())
        }
    ),
    // · Logging
    setting!(
        "logging.level",
        Advanced,
        "NANOFILE_LOG_LEVEL",
        Kind::Text,
        Apply::ProcessRestart,
        |c| c.logging.level.clone(),
        |c, v| {
            c.logging.level = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "logging.file_enabled",
        Advanced,
        "NANOFILE_LOG_FILE_ENABLED",
        Kind::OptBool,
        Apply::ReadOnly,
        |c| fmt_opt_bool(c.logging.file_enabled),
        |c, v| {
            c.logging.file_enabled = parse_opt_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "logging.file",
        Advanced,
        "NANOFILE_LOG_FILE",
        Kind::OptText,
        Apply::ReadOnly,
        |c| c
            .logging
            .file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        |c, v| {
            c.logging.file = opt_text(v).map(PathBuf::from);
            Ok(())
        }
    ),
    setting!(
        "logging.max_file_size_mb",
        Advanced,
        "NANOFILE_LOG_MAX_FILE_SIZE_MB",
        Kind::U64,
        Apply::ProcessRestart,
        |c| c.logging.max_file_size_mb.to_string(),
        |c, v| {
            c.logging.max_file_size_mb = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "logging.max_backups",
        Advanced,
        "NANOFILE_LOG_MAX_BACKUPS",
        Kind::U32,
        Apply::ProcessRestart,
        |c| c.logging.max_backups.to_string(),
        |c, v| {
            c.logging.max_backups = parse_u32(v)?;
            Ok(())
        }
    ),
    // · Sync protocol
    setting!(
        "sync.verify_fs_objects",
        Advanced,
        "NANOFILE_SYNC_VERIFY_FS_OBJECTS",
        Kind::Enum(&["strict", "log", "off"]),
        Apply::LiveWithHook(Hook::SyncStatics),
        |c| c.sync.verify_fs_objects.clone(),
        |c, v| {
            c.sync.verify_fs_objects = parse_enum(v, &["strict", "log", "off"])?;
            Ok(())
        }
    ),
    setting!(
        "sync.max_tree_depth",
        Advanced,
        "NANOFILE_SYNC_MAX_TREE_DEPTH",
        Kind::Usize,
        Apply::LiveWithHook(Hook::SyncStatics),
        |c| c.sync.max_tree_depth.to_string(),
        |c, v| {
            c.sync.max_tree_depth = parse_usize(v)?;
            Ok(())
        }
    ),
    setting!(
        "sync.max_tree_visits",
        Advanced,
        "NANOFILE_SYNC_MAX_TREE_VISITS",
        Kind::Usize,
        Apply::LiveWithHook(Hook::SyncStatics),
        |c| c.sync.max_tree_visits.to_string(),
        |c, v| {
            c.sync.max_tree_visits = parse_usize(v)?;
            Ok(())
        }
    ),
];

/// One group of rows inside an admin page.
///
/// A page is read under a handful of headings rather than as one flat list:
/// `Security` is the master secret, the capability switches, the hosts that are
/// trusted and the headers that are sent, and an operator looking for one of
/// them should not have to read the other three to find it.
pub struct GroupDef {
    pub section: Section,
    /// Stable identifier, used for the page's `data-setting-group` hook and to
    /// build [`Self::title_key`].
    pub id: &'static str,
    /// The locale key of the group's heading.
    pub title_key: &'static str,
    /// The catalog keys the group holds, in render order.
    pub keys: &'static [&'static str],
}

/// Every page's groups, in navigation order.
///
/// This is a second list beside [`CATALOG`], so a key can be added without a
/// home; `the_groups_partition_the_catalog` fails in that case, and the page
/// falls back to a trailing "Other" group rather than dropping the row.
pub static GROUPS: &[GroupDef] = &[
    GroupDef {
        section: Section::Server,
        id: "server_addresses",
        title_key: "setting.group_server_addresses",
        keys: &[
            "server.addr",
            "server.port",
            "server.version",
            "server.site_url",
        ],
    },
    GroupDef {
        section: Section::Server,
        id: "server_requests",
        title_key: "setting.group_server_requests",
        keys: &[
            "server.max_upload_size_mb",
            "server.max_json_body_mb",
            "server.max_chunk_size_mb",
            "server.request_timeout_secs",
            "server.header_read_timeout_secs",
            "server.body_timeout_secs",
        ],
    },
    GroupDef {
        section: Section::Server,
        id: "server_cors",
        title_key: "setting.group_server_cors",
        keys: &["server.cors_allowed_origins", "server.cors_max_age_secs"],
    },
    GroupDef {
        section: Section::Server,
        id: "server_desktop",
        title_key: "setting.group_server_desktop",
        keys: &[
            "server.desktop_custom_brand",
            "server.desktop_custom_logo",
            "server.tray",
            "ui.tray_language",
        ],
    },
    GroupDef {
        section: Section::Server,
        id: "server_interface",
        title_key: "setting.group_server_interface",
        keys: &["ui.default_language"],
    },
    GroupDef {
        section: Section::Security,
        id: "security_secret",
        title_key: "setting.group_security_secret",
        keys: &["server.secret_key"],
    },
    GroupDef {
        section: Section::Security,
        id: "security_capabilities",
        title_key: "setting.group_security_capabilities",
        keys: &[
            "server.webdav_enabled",
            "server.sso_enabled",
            "server.file_search_enabled",
            "server.share_link_enabled",
        ],
    },
    GroupDef {
        section: Section::Security,
        id: "security_hosts",
        title_key: "setting.group_security_hosts",
        keys: &[
            "server.trusted_proxies",
            "server.allowed_hosts",
            "server.trust_request_host",
        ],
    },
    GroupDef {
        section: Section::Security,
        id: "security_headers",
        title_key: "setting.group_security_headers",
        keys: &[
            "server.hsts_include_subdomains",
            "server.max_propfind_entries",
        ],
    },
    GroupDef {
        section: Section::Authentication,
        id: "authentication_passwords",
        title_key: "setting.group_authentication_passwords",
        keys: &[
            "auth.password_hash_iterations",
            "auth.password_min_length",
            "auth.require_strong_password",
            "auth.enable_password_reset",
        ],
    },
    GroupDef {
        section: Section::Authentication,
        id: "authentication_registration",
        title_key: "setting.group_authentication_registration",
        keys: &["auth.enable_invitations"],
    },
    GroupDef {
        section: Section::Authentication,
        id: "authentication_tokens",
        title_key: "setting.group_authentication_tokens",
        keys: &[
            "auth.api_token_ttl_days",
            "auth.sync_token_ttl_days",
            "auth.api_key_ttl_presets_days",
            "auth.api_key_max_ttl_days",
        ],
    },
    GroupDef {
        section: Section::RateLimits,
        id: "rate_limits_sign_in",
        title_key: "setting.group_rate_limits_sign_in",
        keys: &[
            "auth.max_login_attempts",
            "auth.lockout_duration_secs",
            "auth.max_distinct_usernames_per_ip",
            "auth.totp_max_attempts",
        ],
    },
    GroupDef {
        section: Section::RateLimits,
        id: "rate_limits_account_flows",
        title_key: "setting.group_rate_limits_account_flows",
        keys: &[
            "auth.password_reset_max_per_hour",
            "auth.registration_max_per_hour",
            "auth.sso_link_max_per_hour",
        ],
    },
    GroupDef {
        section: Section::RateLimits,
        id: "rate_limits_protected",
        title_key: "setting.group_rate_limits_protected",
        keys: &[
            "auth.link_password_max_per_hour",
            "auth.repo_password_max_per_hour",
            "auth.webdav_max_failures_per_5min",
        ],
    },
    GroupDef {
        section: Section::RateLimits,
        id: "rate_limits_content",
        title_key: "setting.group_rate_limits_content",
        keys: &[
            "auth.share_download_max_per_minute",
            "auth.search_max_per_minute",
            "auth.reindex_max_per_hour",
        ],
    },
    GroupDef {
        section: Section::Storage,
        id: "storage_directories",
        title_key: "setting.group_storage_directories",
        keys: &[
            "storage.block_dir",
            "storage.temp_dir",
            "storage.thumbnail_dir",
            "storage.avatar_dir",
        ],
    },
    GroupDef {
        section: Section::Storage,
        id: "storage_quotas",
        title_key: "setting.group_storage_quotas",
        keys: &[
            "storage.max_storage_bytes",
            "storage.max_temp_uploads",
            "storage.max_temp_upload_bytes",
            "storage.temp_upload_ttl_hours",
        ],
    },
    GroupDef {
        section: Section::Storage,
        id: "storage_archives",
        title_key: "setting.group_storage_archives",
        keys: &[
            "storage.max_zip_entries",
            "storage.max_zip_bytes",
            "storage.max_zip_task_bytes",
        ],
    },
    GroupDef {
        section: Section::Storage,
        id: "storage_media",
        title_key: "setting.group_storage_media",
        keys: &["storage.ffmpeg_path"],
    },
    GroupDef {
        section: Section::Encryption,
        id: "encryption_blocks",
        title_key: "setting.group_encryption_blocks",
        keys: &["storage.block_encryption_mode", "storage.encryption_key"],
    },
    GroupDef {
        section: Section::Encryption,
        id: "encryption_libraries",
        title_key: "setting.group_encryption_libraries",
        keys: &[
            "server.encrypted_library_version",
            "server.encrypted_library_pwd_hash_algo",
            "server.encrypted_library_pwd_hash_params",
        ],
    },
    GroupDef {
        section: Section::Maintenance,
        id: "maintenance_index",
        title_key: "setting.group_maintenance_index",
        keys: &[
            "index.enabled",
            "index.index_dir",
            "index.backfill_enabled",
            "index.sandbox",
        ],
    },
    GroupDef {
        section: Section::Maintenance,
        id: "maintenance_gc",
        title_key: "setting.group_maintenance_gc",
        keys: &["gc.enabled", "gc.interval_hours", "gc.min_block_age_secs"],
    },
    GroupDef {
        section: Section::Maintenance,
        id: "maintenance_tasks",
        title_key: "setting.group_maintenance_tasks",
        keys: &[
            "tasks.max_active_tasks",
            "tasks.max_active_per_user",
            "tasks.load_aware",
            "tasks.load_sample_interval_secs",
            "tasks.max_retained_bytes",
        ],
    },
    GroupDef {
        section: Section::Email,
        id: "email_delivery",
        title_key: "setting.group_email_delivery",
        keys: &[
            "email.enabled",
            "email.paused",
            "email.host",
            "email.port",
            "email.tls",
            "email.timeout_secs",
            "email.max_attempts",
        ],
    },
    GroupDef {
        section: Section::Email,
        id: "email_credentials",
        title_key: "setting.group_email_credentials",
        keys: &["email.username", "email.password"],
    },
    GroupDef {
        section: Section::Email,
        id: "email_sender",
        title_key: "setting.group_email_sender",
        keys: &["email.from_address", "email.from_name"],
    },
    GroupDef {
        section: Section::Email,
        id: "email_notify",
        title_key: "setting.group_email_notify",
        keys: &[
            "email.notify_new_device",
            "email.notify_api_key_created",
            "email.notify_new_login",
        ],
    },
    GroupDef {
        section: Section::Notifications,
        id: "notifications_service",
        title_key: "setting.group_notifications_service",
        keys: &[
            "notification.enabled",
            "notification.private_key",
            "notification.accept_legacy_event_tokens",
        ],
    },
    GroupDef {
        section: Section::Notifications,
        id: "notifications_connections",
        title_key: "setting.group_notifications_connections",
        keys: &[
            "notification.max_connections",
            "notification.max_connections_per_ip",
        ],
    },
    GroupDef {
        section: Section::Notifications,
        id: "notifications_timeouts",
        title_key: "setting.group_notifications_timeouts",
        keys: &[
            "notification.ping_interval",
            "notification.client_timeout",
            "notification.subscribe_timeout_secs",
        ],
    },
    GroupDef {
        section: Section::Advanced,
        id: "advanced_database",
        title_key: "setting.group_advanced_database",
        keys: &["database.url", "database.max_connections"],
    },
    GroupDef {
        section: Section::Advanced,
        id: "advanced_bootstrap",
        title_key: "setting.group_advanced_bootstrap",
        keys: &["admin_init.email", "admin_init.password"],
    },
    GroupDef {
        section: Section::Advanced,
        id: "advanced_logging",
        title_key: "setting.group_advanced_logging",
        keys: &[
            "logging.level",
            "logging.file_enabled",
            "logging.file",
            "logging.max_file_size_mb",
            "logging.max_backups",
        ],
    },
    GroupDef {
        section: Section::Advanced,
        id: "advanced_sync",
        title_key: "setting.group_advanced_sync",
        keys: &[
            "sync.verify_fs_objects",
            "sync.max_tree_depth",
            "sync.max_tree_visits",
        ],
    },
];

/// The groups of one page, in render order.
pub fn groups(section: Section) -> impl Iterator<Item = &'static GroupDef> {
    GROUPS.iter().filter(move |group| group.section == section)
}
