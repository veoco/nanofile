//! The settings catalog: one entry per `Config` field.
//!
//! Every entry names its `NANOFILE_*` variable, how the value is parsed, and
//! when a change takes effect ([`Apply`]). The environment layer is applied
//! *from* this table, so a setting cannot be added without wiring its variable,
//! and the admin UI cannot advertise a variable the server does not read.
//!
//! Entries are grouped by the admin page they appear on
//! ([`Section`]), and within a page by the config file's own ordering — the page
//! then reads like the file an operator already knows.

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
    // ── General ────────────────────────────────────────────────────────────
    setting!(
        "server.addr",
        General,
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
        General,
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
        General,
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
        General,
        "NANOFILE_SERVER_SITE_URL",
        Kind::Text,
        Apply::Live,
        |c| c.server.site_url.clone(),
        |c, v| {
            c.server.site_url = v.trim().to_string();
            Ok(())
        }
    ),
    setting!(
        "server.max_upload_size_mb",
        General,
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
        General,
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
        General,
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
        General,
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
        General,
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
        General,
        "NANOFILE_SERVER_BODY_TIMEOUT_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.server.body_timeout_secs.to_string(),
        |c, v| {
            c.server.body_timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.cors_allowed_origins",
        General,
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
        General,
        "NANOFILE_CORS_MAX_AGE_SECS",
        Kind::U64,
        Apply::Restart,
        |c| c.server.cors_max_age_secs.to_string(),
        |c, v| {
            c.server.cors_max_age_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "server.desktop_custom_brand",
        General,
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
        General,
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
        General,
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
        "ui.default_language",
        General,
        "NANOFILE_UI_DEFAULT_LANGUAGE",
        Kind::Enum(&["en", "zh"]),
        Apply::Live,
        |c| c.ui.default_language.clone(),
        |c, v| {
            c.ui.default_language = parse_enum(v, &["en", "zh"])?;
            Ok(())
        }
    ),
    setting!(
        "ui.tray_language",
        General,
        "NANOFILE_UI_TRAY_LANGUAGE",
        Kind::Enum(&["auto", "en", "zh"]),
        Apply::ProcessRestart,
        |c| c.ui.tray_language.clone(),
        |c, v| {
            c.ui.tray_language = parse_enum(v, &["auto", "en", "zh"])?;
            Ok(())
        }
    ),
    // ── Security ───────────────────────────────────────────────────────────
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
        "server.encrypted_library_version",
        Security,
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
        Security,
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
        Security,
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
    setting!(
        "auth.password_hash_iterations",
        Security,
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
        "auth.api_token_ttl_days",
        Security,
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
        Security,
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
        "auth.enable_invitations",
        Security,
        "NANOFILE_AUTH_ENABLE_INVITATIONS",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.auth.enable_invitations),
        |c, v| {
            c.auth.enable_invitations = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.enable_password_reset",
        Security,
        "NANOFILE_AUTH_ENABLE_PASSWORD_RESET",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.auth.enable_password_reset),
        |c, v| {
            c.auth.enable_password_reset = parse_bool(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.password_min_length",
        Security,
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
        Security,
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
        "auth.api_key_ttl_presets_days",
        Security,
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
        Security,
        "NANOFILE_AUTH_API_KEY_MAX_TTL_DAYS",
        Kind::U64,
        Apply::Live,
        |c| c.auth.api_key_max_ttl_days.to_string(),
        |c, v| {
            c.auth.api_key_max_ttl_days = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.max_login_attempts",
        Security,
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
        Security,
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
        Security,
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
        "auth.sso_link_max_per_hour",
        Security,
        "NANOFILE_AUTH_SSO_LINK_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.sso_link_max_per_hour.to_string(),
        |c, v| {
            c.auth.sso_link_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.password_reset_max_per_hour",
        Security,
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
        Security,
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
        "auth.totp_max_attempts",
        Security,
        "NANOFILE_AUTH_TOTP_MAX_ATTEMPTS",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.totp_max_attempts.to_string(),
        |c, v| {
            c.auth.totp_max_attempts = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.link_password_max_per_hour",
        Security,
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
        Security,
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
        "auth.share_download_max_per_minute",
        Security,
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
        "auth.webdav_max_failures_per_5min",
        Security,
        "NANOFILE_AUTH_WEBDAV_MAX_FAILURES_PER_5MIN",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.webdav_max_failures_per_5min.to_string(),
        |c, v| {
            c.auth.webdav_max_failures_per_5min = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.reindex_max_per_hour",
        Security,
        "NANOFILE_AUTH_REINDEX_MAX_PER_HOUR",
        Kind::U32,
        Apply::LiveWithHook(Hook::RateLimits),
        |c| c.auth.reindex_max_per_hour.to_string(),
        |c, v| {
            c.auth.reindex_max_per_hour = parse_u32(v)?;
            Ok(())
        }
    ),
    setting!(
        "auth.search_max_per_minute",
        Security,
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
        "notification.private_key",
        Security,
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
        Security,
        "NANOFILE_NOTIFICATION_ACCEPT_LEGACY_EVENT_TOKENS",
        Kind::Bool,
        Apply::Live,
        |c| fmt_bool(c.notification.accept_legacy_event_tokens),
        |c, v| {
            c.notification.accept_legacy_event_tokens = parse_bool(v)?;
            Ok(())
        }
    ),
    // ── Storage ────────────────────────────────────────────────────────────
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
        "storage.block_encryption_mode",
        Storage,
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
        Storage,
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
    setting!(
        "index.enabled",
        Storage,
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
        Storage,
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
        "gc.enabled",
        Storage,
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
        Storage,
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
        "tasks.max_active_tasks",
        Storage,
        "NANOFILE_TASKS_MAX_ACTIVE",
        Kind::U64,
        Apply::LiveWithHook(Hook::TaskManager),
        |c| c.tasks.max_active_tasks.to_string(),
        |c, v| {
            c.tasks.max_active_tasks = parse_u64(v)?;
            Ok(())
        }
    ),
    // ── Email ──────────────────────────────────────────────────────────────
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
    setting!(
        "notification.enabled",
        Email,
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
        "notification.ping_interval",
        Email,
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
        Email,
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
        "notification.max_connections",
        Email,
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
        Email,
        "NANOFILE_NOTIFICATION_MAX_CONNECTIONS_PER_IP",
        Kind::U64,
        Apply::LiveWithHook(Hook::NotificationManager),
        |c| c.notification.max_connections_per_ip.to_string(),
        |c, v| {
            c.notification.max_connections_per_ip = parse_u64(v)?;
            Ok(())
        }
    ),
    setting!(
        "notification.subscribe_timeout_secs",
        Email,
        "NANOFILE_NOTIFICATION_SUBSCRIBE_TIMEOUT_SECS",
        Kind::U64,
        Apply::Live,
        |c| c.notification.subscribe_timeout_secs.to_string(),
        |c, v| {
            c.notification.subscribe_timeout_secs = parse_u64(v)?;
            Ok(())
        }
    ),
    // ── Advanced ───────────────────────────────────────────────────────────
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
