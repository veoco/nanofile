use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Environment variable overriding the config file path.
pub const CONFIG_PATH_ENV: &str = "NANOFILE_CONFIG";
/// Name of a configuration file, wherever one lives.
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// Default when neither `--config` nor `NANOFILE_CONFIG` is set: `config.toml` in
/// the working directory.
pub const DEFAULT_CONFIG_PATH: &str = CONFIG_FILE_NAME;
/// Directory created under the home directory for a first-run configuration.
///
/// The same name on every platform: "it is at `~/.nanofile/config.toml`" has to
/// be one sentence, not one per platform.
pub const USER_CONFIG_DIR_NAME: &str = ".nanofile";

/// `NANOFILE_*` variables that supply no configuration, so their presence does
/// not count as "the operator configured this".
///
/// First-run setup is gated on variable *names*: a set-but-invalid value is
/// still an explicit choice, and treating it as unconfigured would stand a
/// generated key next to a deployment that meant to supply one.
pub const NON_CONFIG_ENV_VARS: &[&str] = &[
    // Toggles `.env` loading only.
    "NANOFILE_DOTENV",
    // Read by the sandbox test helpers, never by a serving process.
    "NANOFILE_TEST_FFMPEG_PATH",
    // A compile-time stamp exported by `build.rs`, not a setting.
    "NANOFILE_BUILD_TS",
];

/// Catalog keys the environment supplied a value for, mapped to the variable
/// that supplied it. A value's origin cannot be recovered from the value itself,
/// and a retired variable alias must still be named truthfully in the settings
/// page.
pub type EnvKeys = std::collections::BTreeMap<&'static str, &'static str>;

/// A loaded configuration plus where its environment overrides came from.
pub struct LoadedConfig {
    pub config: Config,
    pub env_keys: EnvKeys,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub gc: GcConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub notification: NotificationConfig,
    #[serde(default)]
    pub tasks: TaskConfig,
    #[serde(default)]
    pub admin_init: AdminInitConfig,
    #[serde(default)]
    pub email: EmailConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub settings: SettingsConfig,
}

/// How the config file and the `settings` table layer with each other.
///
/// The file is *bootstrap*: it supplies any key the database has no row for and
/// stops mattering for a key once one is saved. `config_policy = "override"` (or
/// a key in `config_override_keys`) reverses that for the keys it names; the
/// environment always wins over both.
///
/// Deliberately not managed from the admin UI: this policy decides what the UI's
/// saves mean, so it has to come from outside the layer the UI writes to.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SettingsConfig {
    /// `"bootstrap"` (default) or `"override"`.
    /// Env: NANOFILE_SETTINGS_CONFIG_POLICY
    #[serde(default = "default_config_policy")]
    pub config_policy: String,
    /// Keys the config file wins over the database for while
    /// `config_policy = "bootstrap"`, e.g. `["server.site_url"]`.
    /// Env: NANOFILE_SETTINGS_CONFIG_OVERRIDE_KEYS (comma-separated)
    #[serde(default)]
    pub config_override_keys: Vec<String>,
    /// Seconds between re-reads of the `settings` table, so a change from another
    /// instance is picked up without a restart. `0` disables the periodic read.
    /// Env: NANOFILE_SETTINGS_REFRESH_INTERVAL_SECS
    #[serde(default = "default_settings_refresh_interval")]
    pub refresh_interval_secs: u64,
}

fn default_config_policy() -> String {
    "bootstrap".to_string()
}

fn default_settings_refresh_interval() -> u64 {
    30
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            config_policy: default_config_policy(),
            config_override_keys: Vec::new(),
            refresh_interval_secs: default_settings_refresh_interval(),
        }
    }
}

/// Sync-protocol hardening knobs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncConfig {
    /// Verify that a received FS object's id equals `sha1(uncompressed JSON)`.
    ///
    /// `"strict"` (default) rejects a mismatch with 400; `"log"` accepts and warns;
    /// `"off"` skips the check. The official server does not check, but every
    /// legitimate repo-v1 client sends a matching id, so strict is safe.
    /// Env: NANOFILE_SYNC_VERIFY_FS_OBJECTS
    #[serde(default = "default_verify_fs_objects")]
    pub verify_fs_objects: String,
    /// Maximum BFS depth for directory-tree walks (cycle/DoS guard).
    /// Env: NANOFILE_SYNC_MAX_TREE_DEPTH
    #[serde(default = "default_max_tree_depth")]
    pub max_tree_depth: usize,
    /// Maximum number of directory nodes visited per tree walk.
    /// Env: NANOFILE_SYNC_MAX_TREE_VISITS
    #[serde(default = "default_max_tree_visits")]
    pub max_tree_visits: usize,
}

fn default_verify_fs_objects() -> String {
    "strict".to_string()
}
fn default_max_tree_depth() -> usize {
    512
}
fn default_max_tree_visits() -> usize {
    1_000_000
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            verify_fs_objects: default_verify_fs_objects(),
            max_tree_depth: default_max_tree_depth(),
            max_tree_visits: default_max_tree_visits(),
        }
    }
}

impl SyncConfig {
    /// Whether received FS objects should be rejected on an id/content mismatch.
    pub fn reject_fs_object_mismatch(&self) -> bool {
        !matches!(
            self.verify_fs_objects.to_ascii_lowercase().as_str(),
            "log" | "off"
        )
    }

    /// Whether to verify at all (strict or log).
    pub fn verify_fs_objects(&self) -> bool {
        !matches!(self.verify_fs_objects.to_ascii_lowercase().as_str(), "off")
    }
}

/// Web UI localization settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiConfig {
    /// Fallback UI language when the user has no preference and the browser sends no
    /// supported language.
    #[serde(default = "default_ui_language")]
    pub default_language: String,
    /// Tray menu language: `"auto"` follows the OS locale (falling back to
    /// `default_language`), `"en"` / `"zh"` force one.
    /// Env: NANOFILE_UI_TRAY_LANGUAGE
    #[serde(default = "default_tray_language")]
    pub tray_language: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            default_language: default_ui_language(),
            tray_language: default_tray_language(),
        }
    }
}

fn default_ui_language() -> String {
    "en".to_string()
}

fn default_tray_language() -> String {
    "auto".to_string()
}

/// Email delivery configuration.
///
/// Password reset links are **only** delivered to the account owner's inbox,
/// never echoed in an HTTP response — that would let anyone take over any
/// account. `enabled` is the only way to turn mail on: the admin UI can configure
/// how it is sent but never enable it. The connection fields are *bootstrap*
/// values that seed `email_settings` on first start and are superseded by
/// whatever `/sysadmin/settings/email/` saves.
#[derive(Deserialize, Serialize, Clone)]
pub struct EmailConfig {
    /// Master switch. When `false` (default) the mail subsystem is off: password
    /// reset renders a generic page without minting a token, and nothing is queued.
    #[serde(default)]
    pub enabled: bool,
    /// SMTP host. Empty means "not configured": the subsystem stays inert even
    /// with `enabled = true`, and the startup log says so.
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    /// `"starttls"` (default), `"tls"` (implicit TLS, typically 465) or `"none"`.
    /// `"none"` sends credentials and bodies in the clear — only for a same-machine
    /// relay.
    #[serde(default = "default_smtp_tls")]
    pub tls: String,
    #[serde(default)]
    pub username: String,
    /// SMTP password. Prefer `NANOFILE_EMAIL_PASSWORD_FILE` over
    /// `NANOFILE_EMAIL_PASSWORD` so it does not leak via process listings.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub from_address: String,
    #[serde(default)]
    pub from_name: String,
    /// Bounds one delivery attempt (connect + send).
    #[serde(default = "default_smtp_timeout_secs")]
    pub timeout_secs: u64,
    /// Delivery attempts before a queued message is marked failed.
    #[serde(default = "default_smtp_max_attempts")]
    pub max_attempts: u32,
    /// Stop delivery without clearing the SMTP settings; a paused server still mints
    /// password-reset tokens but sends nothing.
    #[serde(default)]
    pub paused: bool,
    /// Notify the account owner when a client device signs in for the first time. The
    /// password-reset link is not a notification and is not gated by these switches.
    #[serde(default = "default_true")]
    pub notify_new_device: bool,
    /// Notify the account owner when an API key is created.
    #[serde(default = "default_true")]
    pub notify_api_key_created: bool,
    /// Notify the account owner when a browser signs in from a fingerprint the
    /// account had not used before.
    #[serde(default = "default_true")]
    pub notify_new_login: bool,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_smtp_tls() -> String {
    "starttls".to_string()
}

fn default_smtp_timeout_secs() -> u64 {
    10
}

fn default_smtp_max_attempts() -> u32 {
    5
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: default_smtp_port(),
            tls: default_smtp_tls(),
            username: String::new(),
            password: None,
            from_address: String::new(),
            from_name: String::new(),
            timeout_secs: default_smtp_timeout_secs(),
            max_attempts: default_smtp_max_attempts(),
            paused: false,
            notify_new_device: true,
            notify_api_key_created: true,
            notify_new_login: true,
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub private_key: String,
    /// Seconds between WebSocket Ping frames (0 = disable keepalive).
    #[serde(default = "default_ping_interval")]
    pub ping_interval: u64,
    /// Seconds without a Pong after which the connection is dropped.
    #[serde(default = "default_client_timeout")]
    pub client_timeout: u64,
    /// Max concurrent WebSocket connections (0 = unlimited). Bounds connection
    /// exhaustion by anonymous clients.
    #[serde(default = "default_notif_max_connections")]
    pub max_connections: u64,
    /// Max concurrent WebSocket connections per client IP (0 = unlimited).
    #[serde(default = "default_notif_max_connections_per_ip")]
    pub max_connections_per_ip: u64,
    /// Seconds a connection may stay unauthenticated before it is dropped (0 =
    /// disabled), so anonymous connections cannot be held open indefinitely.
    #[serde(default = "default_subscribe_timeout_secs")]
    pub subscribe_timeout_secs: u64,
    /// Also accept **event** JWTs signed with the undivided `private_key`, for
    /// external publishers using the pre-subkey scheme. Off by default.
    /// Subscription JWTs (handed to every repository member) are always rejected on
    /// `/notification/events`; this only re-admits raw-key signers.
    #[serde(default)]
    pub accept_legacy_event_tokens: bool,
}

fn default_ping_interval() -> u64 {
    30
}
fn default_client_timeout() -> u64 {
    90
}
fn default_notif_max_connections() -> u64 {
    1024
}
fn default_notif_max_connections_per_ip() -> u64 {
    64
}
fn default_subscribe_timeout_secs() -> u64 {
    60
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            private_key: String::new(),
            ping_interval: default_ping_interval(),
            client_timeout: default_client_timeout(),
            max_connections: default_notif_max_connections(),
            max_connections_per_ip: default_notif_max_connections_per_ip(),
            subscribe_timeout_secs: default_subscribe_timeout_secs(),
            accept_legacy_event_tokens: false,
        }
    }
}

/// Background copy/move task limits.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskConfig {
    /// Max async copy/move tasks in Pending or Processing state (0 = unlimited).
    /// Beyond the cap new tasks get HTTP 429, so authenticated users cannot exhaust
    /// the server with unbounded background copies. Env: NANOFILE_TASKS_MAX_ACTIVE
    #[serde(default = "default_task_max_active")]
    pub max_active_tasks: u64,
    /// Max runs one user may have active at once, across every job (0 = unlimited).
    /// The server-wide cap is shared, so without this one account can take every
    /// slot. Env: NANOFILE_TASKS_MAX_ACTIVE_PER_USER
    #[serde(default = "default_task_max_active_per_user")]
    pub max_active_per_user: u64,
    /// Byte budget for retained job runs (0 = unlimited). A run carries the
    /// request that submitted it, so a count cap alone does not bound memory.
    /// Env: NANOFILE_TASKS_MAX_RETAINED_BYTES
    #[serde(default = "default_task_max_retained_bytes")]
    pub max_retained_bytes: u64,
    /// Let heavy background jobs wait for a quiet server and slow down while it is
    /// busy. Off by default: the thresholds need a week of real traffic first.
    /// Env: NANOFILE_TASKS_LOAD_AWARE
    #[serde(default)]
    pub load_aware: bool,
    /// Seconds between load samples (minimum 1).
    /// Env: NANOFILE_TASKS_LOAD_SAMPLE_INTERVAL_SECS
    #[serde(default = "default_task_load_sample_interval_secs")]
    pub load_sample_interval_secs: u64,
}

fn default_task_max_active() -> u64 {
    100
}

fn default_task_max_active_per_user() -> u64 {
    8
}

fn default_task_max_retained_bytes() -> u64 {
    8 * 1024 * 1024
}

fn default_task_load_sample_interval_secs() -> u64 {
    5
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            max_active_tasks: default_task_max_active(),
            max_active_per_user: default_task_max_active_per_user(),
            max_retained_bytes: default_task_max_retained_bytes(),
            load_aware: false,
            load_sample_interval_secs: default_task_load_sample_interval_secs(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Seafile-compatible server version advertised via `/api2/server-info/`. Clients
    /// parse the version numbers to gate features, so keep it at a version whose
    /// capabilities this server implements. Env: NANOFILE_SERVER_VERSION
    #[serde(default = "default_server_version")]
    pub version: String,
    /// External URL for this server, e.g. "http://127.0.0.1:8082"; used for download,
    /// upload and share URLs and as the default CORS origin. Empty derives it from
    /// addr:port.
    #[serde(default = "default_site_url")]
    pub site_url: String,
    #[serde(default = "default_max_upload_size_mb")]
    pub max_upload_size_mb: u64,
    /// Max JSON/Form request body in MiB, set as the app-wide axum `DefaultBodyLimit`
    /// (upload routes raise their own limit). Stops one request buffering gigabytes.
    /// Env: NANOFILE_SERVER_MAX_JSON_BODY_MB
    #[serde(default = "default_max_json_body_mb")]
    pub max_json_body_mb: u64,
    /// Per-chunk cap for resumable (Content-Range) uploads, enforced from the header
    /// and again on the bytes read. 0 disables it.
    /// Env: NANOFILE_SERVER_MAX_CHUNK_SIZE_MB
    #[serde(default = "default_max_chunk_size_mb")]
    pub max_chunk_size_mb: u64,
    /// Total time a request handler (including reading its body) may run before
    /// the connection is answered with 408. Env: NANOFILE_SERVER_REQUEST_TIMEOUT_SECS
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// How long a client may take to send request headers (slowloris guard); 0
    /// disables the deadline. Env: NANOFILE_SERVER_HEADER_READ_TIMEOUT_SECS
    #[serde(default = "default_header_read_timeout_secs")]
    pub header_read_timeout_secs: u64,
    /// How long a client may take to send the body before 408 (0 = only bounded by
    /// `request_timeout_secs`). Env: NANOFILE_SERVER_BODY_TIMEOUT_SECS
    #[serde(default)]
    pub body_timeout_secs: u64,
    /// Allowed CORS origins. When empty, defaults to the origin of `site_url`.
    /// Set to a comma-separated list for multiple origins (e.g. for API clients).
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// Server-wide secret for CSRF tokens, notification JWTs, the storage encryption
    /// master key and the sync-token encryption key. Must be at least 32 bytes of
    /// entropy; 64 hex chars from `openssl rand -hex 32` is recommended. **Release
    /// builds require an explicit value** — startup fails rather than deriving keys
    /// from an ephemeral secret, which would make encrypted data unreadable after a
    /// restart. Debug builds auto-generate one with a warning.
    /// Env: NANOFILE_SERVER_SECRET_KEY
    #[serde(default)]
    pub secret_key: String,
    /// CORS max-age in seconds (default 86400 = 24h).
    #[serde(default = "default_cors_max_age")]
    pub cors_max_age_secs: u64,
    /// Whether the WebDAV endpoint (`/dav/...`) is enabled; false returns 403 for all.
    #[serde(default = "default_true")]
    pub webdav_enabled: bool,
    /// Whether the SSO local-browser login flow is enabled. When false, its browser
    /// pages error and `client-sso-via-local-browser` is not advertised in
    /// `/api2/server-info/`. Env: NANOFILE_SERVER_SSO_ENABLED
    #[serde(default = "default_true")]
    pub sso_enabled: bool,
    /// Custom brand string shown by the desktop client in its title bar, advertised
    /// verbatim as `desktop-custom-brand`; `None` keeps the key absent.
    /// Env: NANOFILE_SERVER_DESKTOP_CUSTOM_BRAND
    #[serde(default)]
    pub desktop_custom_brand: Option<String>,
    /// Path (relative to the server root) of a custom logo the desktop client fetches,
    /// advertised as `desktop-custom-logo`. The client joins this onto the server URL
    /// itself, so a full URL would be mangled. `None` keeps the key absent.
    /// Env: NANOFILE_SERVER_DESKTOP_CUSTOM_LOGO
    #[serde(default)]
    pub desktop_custom_logo: Option<String>,
    /// Encryption version advertised as `encrypted_library_version` and used when the
    /// **server** generates keys for a library created with a bare `passwd`.
    ///
    /// Clients take it verbatim as `enc_version`. Only the AES-256-CBC versions 2
    /// (fixed salt) and 4 (per-library salt) are implemented; seafile's version 3
    /// uses a different cipher and fails the key schedule, so advertising it breaks
    /// every encrypted-library creation. Seahub's default is 2.
    /// Env: NANOFILE_SERVER_ENCRYPTED_LIBRARY_VERSION
    #[serde(default = "default_encrypted_library_version")]
    pub encrypted_library_version: i32,
    /// Hash algorithm for encrypted-library passwords, advertised as
    /// `encrypted_library_pwd_hash_algo` (seahub's `ENCRYPTED_LIBRARY_PWD_HASH_ALGO`).
    /// The desktop client compares it verbatim and, when present, sends a `pwd_hash`
    /// instead of a `magic`; Android and the web UI let the server hash a bare
    /// `passwd` with it. `None` or empty keeps the legacy `magic` flow.
    ///
    /// Accepted values are exactly `pbkdf2_sha256` and `argon2id`; anything else
    /// fails at startup rather than confusing every client.
    /// Env: NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_ALGO
    #[serde(default)]
    pub encrypted_library_pwd_hash_algo: Option<String>,
    /// Parameters of `encrypted_library_pwd_hash_algo`, advertised alongside it
    /// (seahub's `ENCRYPTED_LIBRARY_PWD_HASH_PARAMS`): an iteration count for
    /// `pbkdf2_sha256`, `t,m,p` for `argon2id`. `None` uses the algorithm defaults
    /// (1000, and `2,102400,8`).
    ///
    /// The desktop client cannot read this key (it looks for
    /// `encrypted_library_pwd_params` and assigns the algorithm to the value), so
    /// client-created libraries always use the defaults; the server honours the
    /// configured value when it hashes a bare `passwd` itself.
    /// Env: NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_PARAMS
    #[serde(default)]
    pub encrypted_library_pwd_hash_params: Option<String>,
    /// Advertise `file-search` (gates the desktop client's search tab and mobile
    /// search); the search API exists either way.
    /// Env: NANOFILE_SERVER_FILE_SEARCH_ENABLED
    #[serde(default = "default_true")]
    pub file_search_enabled: bool,
    /// Whether the download half of sharing is available: share links (`/f/`, `/d/`)
    /// can be created and opened. When false, creating one is rejected, existing links
    /// answer as gone, and the web UI and clients stop offering sharing.
    ///
    /// Split out of the retired master switch `share_link_enabled`, which still sets
    /// both directions when a config file uses the old name.
    /// Env: NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED
    #[serde(default = "default_true")]
    pub share_download_enabled: bool,
    /// Whether the upload half of sharing is available: upload links (`/u/`) can be
    /// created and used. Turning it off rejects new upload links and stops minted
    /// upload tokens from accepting writes.
    ///
    /// Split out of the retired master switch `share_link_enabled`, which still sets
    /// both directions when a config file uses the old name.
    /// Env: NANOFILE_SERVER_SHARE_UPLOAD_ENABLED
    #[serde(default = "default_true")]
    pub share_upload_enabled: bool,
    /// IP addresses of trusted reverse proxies. Forwarded client-address headers
    /// (`CF-Connecting-IP`, `X-Forwarded-For`) are honored for rate limiting only when
    /// the TCP peer is one of these; otherwise the raw peer address is used, so clients
    /// cannot spoof the header to bypass per-IP limits. Empty means exposed directly.
    ///
    /// Each entry is an exact IP, a CIDR range (`172.16.0.0/12`, `fd00::/8`), or one of
    /// `loopback`, `private`, `link-local`, `unique-local`. `cloudflared` reaches the
    /// origin from a loopback or private address and sets `CF-Connecting-IP` to the real
    /// visitor, so `trusted_proxies = ["private"]` restores correct client IPs when the
    /// origin is bound to `127.0.0.1`; on a host reachable by LAN peers, trusting
    /// `private` would let them forge the headers.
    /// Env: NANOFILE_SERVER_TRUSTED_PROXIES (comma-separated)
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// Hosts allowed in generated absolute URLs (download and block links) while
    /// `site_url` is left at its built-in default.
    ///
    /// The request `Host` is echoed into those URLs so LAN clients hitting the server
    /// by IP get a reachable address. A Host the server did not choose is
    /// attacker-influenced (DNS rebinding, wildcard vhost, misconfigured proxy), and a
    /// client that follows the returned URL sends its capability token there. When
    /// this list is non-empty only these values are accepted; when empty the previous
    /// behaviour is kept and startup logs an advisory. Setting `site_url` to the
    /// address clients actually use makes the list unnecessary.
    /// Env: NANOFILE_SERVER_ALLOWED_HOSTS (comma-separated)
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Whether to honour the request `Host` header when building absolute URLs while
    /// `site_url` is left at its built-in default.
    ///
    /// Defaults to `true` because direct LAN access relies on it. The header is only
    /// trusted when it is a syntactically valid authority and, when configured,
    /// matches `allowed_hosts`. Set to `false` to always use `site_url` — recommended
    /// behind a proxy or wildcard vhost, where an attacker-influenced Host would
    /// otherwise be echoed into a token-bearing URL.
    /// Env: NANOFILE_SERVER_TRUST_REQUEST_HOST
    #[serde(default = "default_true")]
    pub trust_request_host: bool,
    /// Max resources one WebDAV `PROPFIND` may return (0 = unlimited).
    ///
    /// RFC 4918 makes `Depth: infinity` the default, so one request can ask for an
    /// entire library and the multistatus document is assembled in memory. Over the
    /// cap the request is answered `507` rather than truncated, telling the client to
    /// retry with `Depth: 1`.
    /// Env: NANOFILE_SERVER_MAX_PROPFIND_ENTRIES
    #[serde(default = "default_max_propfind_entries")]
    pub max_propfind_entries: usize,
    /// Add `includeSubDomains` to the HSTS header (HTTPS deployments only).
    ///
    /// Off by default: a deployment may serve unrelated plain-HTTP sites on sibling
    /// host names. Env: NANOFILE_SERVER_HSTS_INCLUDE_SUBDOMAINS
    #[serde(default)]
    pub hsts_include_subdomains: bool,
    /// Whether the system tray icon is shown (only for `--features tray` builds). Set
    /// to false to run headless in a desktop session, e.g. for auto-started instances
    /// that should stay invisible. Env: NANOFILE_SERVER_TRAY
    #[serde(default = "default_true")]
    pub tray: bool,
}

fn default_addr() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8082
}
fn default_max_upload_size_mb() -> u64 {
    4096
}
fn default_max_json_body_mb() -> u64 {
    64
}
/// The largest single `Content-Range` chunk a client may send. The official
/// clients send 4 MiB chunks; the cap exists so a buggy or hostile client cannot
/// make the server buffer an arbitrary amount for one request. Raise it only for a
/// client that really sends larger chunks.
fn default_max_chunk_size_mb() -> u64 {
    32
}
fn default_header_read_timeout_secs() -> u64 {
    30
}
fn default_request_timeout_secs() -> u64 {
    600
}
fn default_server_version() -> String {
    "8.0.0".to_string()
}
fn default_site_url() -> String {
    "http://127.0.0.1:8082".to_string()
}
fn default_cors_max_age() -> u64 {
    86400
}
fn default_max_propfind_entries() -> usize {
    100_000
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: default_addr(),
            port: default_port(),
            version: default_server_version(),
            site_url: default_site_url(),
            max_upload_size_mb: default_max_upload_size_mb(),
            max_json_body_mb: default_max_json_body_mb(),
            max_chunk_size_mb: default_max_chunk_size_mb(),
            request_timeout_secs: default_request_timeout_secs(),
            header_read_timeout_secs: default_header_read_timeout_secs(),
            body_timeout_secs: 0,
            cors_allowed_origins: Vec::new(),
            secret_key: String::new(),
            cors_max_age_secs: default_cors_max_age(),
            webdav_enabled: default_true(),
            sso_enabled: default_true(),
            desktop_custom_brand: None,
            desktop_custom_logo: None,
            encrypted_library_version: default_encrypted_library_version(),
            encrypted_library_pwd_hash_algo: None,
            encrypted_library_pwd_hash_params: None,
            file_search_enabled: default_true(),
            share_download_enabled: default_true(),
            share_upload_enabled: default_true(),
            trusted_proxies: Vec::new(),
            allowed_hosts: Vec::new(),
            trust_request_host: default_true(),
            max_propfind_entries: default_max_propfind_entries(),
            hsts_include_subdomains: false,
            tray: default_true(),
        }
    }
}

impl ServerConfig {
    /// Validate the encrypted-library wire contract.
    ///
    /// These values are echoed through `/api2/server-info/` and taken verbatim:
    /// `encrypted_library_version` becomes a new library's `enc_version`, and
    /// `encrypted_library_pwd_hash_algo` decides whether the client sends a `pwd_hash`
    /// instead of a `magic`. A value nanofile cannot honour breaks encrypted-library
    /// creation on every client, so it is rejected at startup rather than at the
    /// first request.
    ///
    /// Returns `Err` with a message meant to be shown to the operator.
    pub fn validate_encrypted_library(&self) -> Result<(), String> {
        if !matches!(self.encrypted_library_version, 2 | 4) {
            return Err(format!(
                "server.encrypted_library_version = {} is not supported: only 2 (fixed salt) \
                 and 4 (per-library salt) are implemented (both AES-256-CBC). Seafile's \
                 version 3 wraps the library key with AES-128-ECB, which the key schedule \
                 rejects; clients told to use it would create a library this server cannot \
                 open.",
                self.encrypted_library_version
            ));
        }

        let algo = self
            .encrypted_library_pwd_hash_algo
            .as_deref()
            .map(str::trim)
            .unwrap_or_default();
        if algo.is_empty() {
            // Upstream's default (empty): the legacy magic flow, no `pwd_hash`.
            return Ok(());
        }
        if !crate::crypto::pwd_hash::is_supported_algo(algo) {
            return Err(format!(
                "server.encrypted_library_pwd_hash_algo = '{algo}' is not supported: use \
                 '{}' or '{}'. The value is compared verbatim by the clients, so it must be \
                 lowercase and spelled exactly like seafile spells it.",
                crate::crypto::pwd_hash::ALGO_PBKDF2_SHA256,
                crate::crypto::pwd_hash::ALGO_ARGON2ID,
            ));
        }
        crate::crypto::pwd_hash::validate_params(
            algo,
            self.encrypted_library_pwd_hash_params
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .map_err(|e| format!("server.encrypted_library_pwd_hash_params is invalid: {e}"))
    }

    /// Whether `site_url` is still the built-in default (`http://127.0.0.1:8082`).
    ///
    /// An unconfigured site_url is not reachable from other machines, so URL
    /// construction falls back to the request Host header.
    pub fn site_url_is_default(&self) -> bool {
        self.site_url.trim().trim_end_matches('/') == default_site_url()
    }

    /// URL base (`scheme://host[:port]`) for download / block links.
    ///
    /// Mirrors seahub's `FILE_SERVER_ROOT`: a configured `site_url` always wins,
    /// because only the admin knows the address clients can reach. The request `Host`
    /// is used only while `site_url` is the built-in default, so LAN clients get a
    /// reachable URL; its port is kept only when the client used one, never replaced
    /// by the internal listen port (which produced unreachable `:8082` URLs behind a
    /// reverse proxy). Because the Host is client-supplied, `allowed_hosts` restricts
    /// which values may be echoed.
    pub fn download_url_base(&self, host_header: Option<&str>) -> String {
        let base = self.site_url.trim_end_matches('/');
        if !self.site_url_is_default() {
            return base.to_string();
        }
        if self.trust_request_host
            && let Some(h) = host_header
            && is_valid_host_header(h)
            && (self.allowed_hosts.is_empty()
                || self
                    .allowed_hosts
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(h)))
        {
            // When `allowed_hosts` is empty the Host is only echoed if it names a literal
            // address, because the URL built here **carries a capability token**. A DNS name
            // is attacker-influenceable (DNS rebinding, wildcard vhost, a proxy forwarding
            // arbitrary Host values) and following the link would hand the token to whoever
            // owns the name. Literal addresses are the documented zero-config LAN case; a
            // hostname can be echoed by listing it in `allowed_hosts`.
            if !self.allowed_hosts.is_empty() || host_names_literal_address(h) {
                return format!("{}://{h}", self.site_url_scheme());
            }
        }
        base.to_string()
    }

    /// Extract the scheme (http / https) from `site_url`.
    pub fn site_url_scheme(&self) -> &str {
        if self.site_url_is_https() {
            "https"
        } else {
            "http"
        }
    }

    /// Whether `site_url` uses `https`. Uses `get(..8)` so a short or multi-byte value
    /// cannot panic; the comparison is case-insensitive because schemes are.
    fn site_url_is_https(&self) -> bool {
        self.site_url
            .trim()
            .get(..8)
            .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
    }

    /// Whether cookies carry `Secure`: true when `site_url` uses https.
    pub fn secure_cookies(&self) -> bool {
        self.site_url_is_https()
    }

    /// Extract the origin (scheme + host + port) from `site_url`, e.g.
    /// `http://127.0.0.1:8082/some/path` -> `http://127.0.0.1:8082`.
    ///
    /// Never panics: a malformed value yields the built-in default instead of slicing
    /// at a non-char boundary. [`Config::normalize`] repairs common cases at load time.
    pub fn site_url_origin(&self) -> String {
        let url = self.site_url.trim();
        let (scheme, rest) = match url.split_once("://") {
            Some((scheme, rest)) if !scheme.is_empty() => (format!("{scheme}://"), rest),
            // No scheme: treat the whole value as the authority (a bare
            // `example.com` origin is what the operator meant).
            _ => (String::from("http://"), url),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
        if authority.is_empty() {
            return default_site_url();
        }
        format!("{scheme}{authority}")
    }

    /// CORS origins to allow; defaults to the origin of `site_url` when empty.
    pub fn cors_origins(&self) -> Vec<String> {
        if self.cors_allowed_origins.is_empty() {
            vec![self.site_url_origin()]
        } else {
            self.cors_allowed_origins.clone()
        }
    }
}

/// Whether a `Host` value is safe to echo into an absolute URL.
///
/// Only a plain ASCII `host[:port]` is accepted: no userinfo, path, query,
/// fragment, whitespace or control characters, and a numeric port in range.
pub fn is_valid_host_header(host: &str) -> bool {
    if host.is_empty() || host.len() > 255 || !host.is_ascii() {
        return false;
    }
    if host
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return false;
    }
    if host.contains(['/', '?', '#', '@', '\\', '"', '\'', '<', '>', ';', ',']) {
        return false;
    }

    // Split the optional port. An IPv6 literal keeps its brackets, so the port
    // separator is the colon *after* the closing bracket.
    let (host_ok, port) = if let Some(rest) = host.strip_prefix('[') {
        let Some((inside, after)) = rest.split_once(']') else {
            return false;
        };
        let inside_ok = !inside.is_empty()
            && inside
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
        match after.strip_prefix(':') {
            Some(port) => (inside_ok, Some(port)),
            None => (inside_ok && after.is_empty(), None),
        }
    } else {
        match host.split_once(':') {
            Some((name, port)) => (is_hostname(name), Some(port)),
            None => (is_hostname(host), None),
        }
    };
    if !host_ok {
        return false;
    }
    match port {
        None => true,
        Some(p) => {
            !p.is_empty()
                && p.len() <= 5
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().is_ok_and(|n| n > 0 && n <= 65535)
        }
    }
}

/// A hostname: dot-separated labels of letters, digits, `-` and `_`.
fn is_hostname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

/// Whether a validated `Host` names a literal IP address (or localhost).
///
/// Token-bearing URLs may echo only these when `allowed_hosts` is unset: a DNS
/// name is attacker-influenceable, while LAN deployments reach the server by
/// address anyway.
fn host_names_literal_address(host: &str) -> bool {
    // Strip a `:port` suffix; bracketed IPv6 keeps its own colons.
    let name = if host.starts_with('[') {
        host.split(']')
            .next()
            .unwrap_or(host)
            .trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or(host)
    };
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    name.parse::<std::net::IpAddr>().is_ok()
}

#[derive(Deserialize, Serialize, Clone)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_url")]
    pub url: String,
    /// Pooled SQLite connections (WAL allows many readers but one writer), mainly
    /// improving read throughput. Env: NANOFILE_DATABASE_MAX_CONNECTIONS
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_db_url() -> String {
    "sqlite:data/nanofile.db?mode=rwc".to_string()
}
fn default_max_connections() -> u32 {
    5
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_db_url(),
            max_connections: default_max_connections(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct StorageConfig {
    #[serde(default = "default_block_dir")]
    pub block_dir: PathBuf,
    #[serde(default = "default_temp_dir")]
    pub temp_dir: PathBuf,
    /// Default space quota, in bytes, for an account that has no quota of its own —
    /// a user whose `users.storage_quota` is `NULL`, which is every account created
    /// by self-registration, by `adduser`, or by the admin form with the field left
    /// empty. `0` leaves them unlimited. A finite value bounds every such account,
    /// administrators included, so give one account its own quota to exempt it.
    /// Env: NANOFILE_STORAGE_DEFAULT_USER_QUOTA_BYTES
    #[serde(default)]
    pub default_user_quota_bytes: u64,
    /// `ffmpeg` binary for video thumbnails; when it is not found, videos fall back
    /// to a play-icon placeholder. Env: NANOFILE_STORAGE_FFMPEG_PATH
    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,
    /// Max concurrent in-flight resumable uploads tracked by the temp manager (0 =
    /// unlimited). Bounds memory and disk from abandoned chunked uploads, including
    /// anonymous upload-link ones. Env: NANOFILE_STORAGE_MAX_TEMP_UPLOADS
    #[serde(default = "default_max_temp_uploads")]
    pub max_temp_uploads: u64,
    /// Max declared bytes across active temp uploads (default 32 GiB), so a client
    /// cannot declare a disk-filling set of resumable uploads; 0 = unlimited.
    /// Env: NANOFILE_STORAGE_MAX_TEMP_UPLOAD_BYTES
    #[serde(default = "default_max_temp_upload_bytes")]
    pub max_temp_upload_bytes: u64,
    /// Hours an abandoned resumable-upload temp file is kept before cleanup (0 =
    /// disable the cleanup task). Env: NANOFILE_STORAGE_TEMP_UPLOAD_TTL_HOURS
    #[serde(default = "default_temp_upload_ttl_hours")]
    pub temp_upload_ttl_hours: u64,
    /// Max files in a single zip archive (0 = unlimited). Prevents one zip
    /// request from packaging an unbounded number of files, bounding the CPU,
    /// disk and bandwidth a single download can consume.
    /// Env: NANOFILE_STORAGE_MAX_ZIP_ENTRIES
    #[serde(default = "default_max_zip_entries")]
    pub max_zip_entries: u64,
    /// Max total uncompressed bytes in one zip archive (default 8 GiB; 0 =
    /// unlimited), so one "download as zip" cannot exhaust the disk.
    #[serde(default = "default_max_zip_bytes")]
    pub max_zip_bytes: u64,
    /// Byte budget for all unconsumed zip-download tokens (0 = unlimited). A token
    /// carries every file's block ids, so this bounds memory where a count does not.
    /// Oldest tokens are evicted first; a single oversized archive is rejected.
    /// Env: NANOFILE_STORAGE_MAX_ZIP_TASK_BYTES
    #[serde(default = "default_max_zip_task_bytes")]
    pub max_zip_task_bytes: u64,
    /// Thumbnail cache directory (default `data/thumbnails` for backward
    /// compatibility). Env: NANOFILE_STORAGE_THUMBNAIL_DIR
    #[serde(default = "default_thumbnail_dir")]
    pub thumbnail_dir: PathBuf,
    /// Avatar storage directory (default `data/avatars` for backward compatibility).
    /// Env: NANOFILE_STORAGE_AVATAR_DIR
    #[serde(default = "default_avatar_dir")]
    pub avatar_dir: PathBuf,
    /// Server-side transparent at-rest encryption for file blocks.
    ///
    /// - `off`: blocks are stored as received (legacy plaintext).
    /// - `on`: new writes are encrypted on disk and reads are decrypted; requires
    ///   `encryption_key`.
    /// - `lazy`: new writes are encrypted, reads accept both — the migration window.
    ///
    /// The block id (`sha1` of the logical bytes) is unchanged in every mode, so
    /// Seafile clients and content-addressed dedup keep working.
    /// Env: NANOFILE_STORAGE_BLOCK_ENCRYPTION_MODE
    #[serde(default = "default_block_encryption_mode")]
    pub block_encryption_mode: String,
    /// Master key for block at-rest encryption, set via
    /// `NANOFILE_STORAGE_ENCRYPTION_KEY` or `..._KEY_FILE`. Never serialized into
    /// `config.toml` (or its `.bak`), so secrets never reach disk or backups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_key: Option<String>,
}

fn default_block_dir() -> PathBuf {
    PathBuf::from("data/blocks")
}
fn default_temp_dir() -> PathBuf {
    PathBuf::from("data/temp")
}
fn default_ffmpeg_path() -> String {
    "ffmpeg".to_string()
}
fn default_max_temp_uploads() -> u64 {
    1000
}
/// 32 GiB: a busy client's declared in-flight resumable uploads, without
/// letting a flood of them fill the disk.
fn default_max_temp_upload_bytes() -> u64 {
    32 * 1024 * 1024 * 1024
}
fn default_temp_upload_ttl_hours() -> u64 {
    24
}
fn default_max_zip_entries() -> u64 {
    10000
}
/// 8 GiB: larger than any archive a browser download can usefully stream, so
/// the cap only ever refuses the pathological one.
fn default_max_zip_bytes() -> u64 {
    8 * 1024 * 1024 * 1024
}

/// 32 MiB: roughly a 10k-file listing with a handful of blocks each, times a
/// few concurrently-held tokens.
fn default_max_zip_task_bytes() -> u64 {
    32 * 1024 * 1024
}
fn default_thumbnail_dir() -> PathBuf {
    PathBuf::from("data/thumbnails")
}
fn default_avatar_dir() -> PathBuf {
    PathBuf::from("data/avatars")
}
fn default_block_encryption_mode() -> String {
    "off".to_string()
}

impl StorageConfig {
    /// Parse `block_encryption_mode` into the runtime mode. Callers must have
    /// validated it (see [`Self::validate_block_encryption_mode`]): an unknown
    /// value reads as `Off` here so no code path can panic, but a typo in the
    /// configuration file must not survive startup, or a deployment that meant to
    /// encrypt silently starts writing plaintext blocks.
    pub fn block_encryption_mode(
        &self,
    ) -> crate::storage::encrypting_block_store::BlockEncryptionMode {
        use crate::storage::encrypting_block_store::BlockEncryptionMode;
        match self
            .block_encryption_mode
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "on" => BlockEncryptionMode::On,
            "lazy" => BlockEncryptionMode::Lazy,
            _ => BlockEncryptionMode::Off,
        }
    }

    /// Reject a mode the runtime parser would read as `off` only because of a
    /// typo or stray whitespace. Case and surrounding whitespace are accepted.
    pub fn validate_block_encryption_mode(&self) -> Result<(), String> {
        match self
            .block_encryption_mode
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "off" | "on" | "lazy" => Ok(()),
            _ => Err(format!(
                "storage.block_encryption_mode = {:?} is not one of off, on, lazy",
                self.block_encryption_mode
            )),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            block_dir: default_block_dir(),
            temp_dir: default_temp_dir(),
            default_user_quota_bytes: 0,
            ffmpeg_path: default_ffmpeg_path(),
            max_temp_uploads: default_max_temp_uploads(),
            max_temp_upload_bytes: default_max_temp_upload_bytes(),
            temp_upload_ttl_hours: default_temp_upload_ttl_hours(),
            max_zip_entries: default_max_zip_entries(),
            max_zip_bytes: default_max_zip_bytes(),
            max_zip_task_bytes: default_max_zip_task_bytes(),
            thumbnail_dir: default_thumbnail_dir(),
            avatar_dir: default_avatar_dir(),
            block_encryption_mode: default_block_encryption_mode(),
            encryption_key: None,
        }
    }
}

/// How the anonymous web forms ask for a proof-of-work challenge.
///
/// The challenge is solved in the browser and verified by the server; the API
/// login, WebDAV and sync clients never see it. `Off` keeps the forms as they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptchaMode {
    /// No challenge anywhere.
    #[default]
    Off,
    /// Registration and password reset always challenge; sign-in challenges only
    /// after `captcha_trigger_after` failures from the same client address.
    Adaptive,
    /// Every protected web form challenges.
    Always,
}

/// The lowest accepted [`AuthConfig::captcha_difficulty_bits`].
pub const CAPTCHA_MIN_BITS: u8 = 12;
/// The highest accepted [`AuthConfig::captcha_difficulty_bits`].
pub const CAPTCHA_MAX_BITS: u8 = 22;

impl CaptchaMode {
    /// Every mode, in settings-page order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Adaptive, Self::Always];

    /// The config-file / settings value of this mode.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Adaptive => "adaptive",
            Self::Always => "always",
        }
    }

    /// Parse a config-file / settings value, case-insensitively.
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str().eq_ignore_ascii_case(trimmed))
            .ok_or_else(|| {
                format!(
                    "unknown value {trimmed:?}; use one of {}",
                    Self::ALL
                        .iter()
                        .map(|mode| mode.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthConfig {
    #[serde(default = "default_password_hash_iterations")]
    pub password_hash_iterations: u32,
    /// Lifetime of an account (API/session) token in days. **`0` means never expire**
    /// on the API login path (`POST /api2/auth-token/`), a permanent bearer credential
    /// until a password change or reset revokes it; the web-UI path treats `0` as
    /// "expires immediately" instead. Leave this at its default.
    #[serde(default = "default_api_token_ttl_days")]
    pub api_token_ttl_days: u64,
    /// Lifetime of a repository sync token, in days.
    ///
    /// **`0` means the token never expires.** Leave this at its default.
    #[serde(default = "default_sync_token_ttl_days")]
    pub sync_token_ttl_days: u64,
    /// Max failed login attempts before the client address (and the
    /// address/account pair) is locked out (0 = disable the failed-attempt
    /// lockout). Counted over `lockout_duration_secs`.
    #[serde(default = "default_five")]
    pub max_login_attempts: u32,
    /// How long a login lockout lasts, and the window over which failures are
    /// counted.
    #[serde(default = "default_lockout_duration_secs")]
    pub lockout_duration_secs: u64,
    /// Max distinct account names one client address may fail to log in to
    /// within `lockout_duration_secs` before that address is blocked
    /// (0 = unlimited).
    ///
    /// This is the counter a successful login cannot reset: without it an
    /// attacker holding one valid account could clear their per-address budget
    /// after every few guesses and spray forever.
    #[serde(default = "default_spray_distinct_usernames")]
    pub max_distinct_usernames_per_ip: u32,
    /// Max anonymous SSO link creations per IP per hour (0 = unlimited).
    ///
    /// `POST /api2/client-sso-link/` is anonymous by protocol and writes a row
    /// per call, so without a cap it is a cheap way to fill `sso_login_tokens`
    /// and take the SQLite writer lock.
    /// Env: NANOFILE_AUTH_SSO_LINK_MAX_PER_HOUR
    #[serde(default = "default_sso_link_max_per_hour")]
    pub sso_link_max_per_hour: u32,
    /// Whether visitors may create their own account. With this off only an
    /// administrator can add accounts.
    ///
    /// Renamed from `enable_invitations`; the old name stays accepted as a serde
    /// alias so an existing config file cannot silently reopen registration.
    #[serde(default = "default_true", alias = "enable_invitations")]
    pub enable_registration: bool,
    /// Whether self-registration needs an invitation code. When off, anyone can
    /// register, and the new account must verify its email address before it can
    /// sign in.
    #[serde(default = "default_true")]
    pub require_invitation: bool,
    /// Proof-of-work challenge for the web sign-in, registration and password-reset
    /// forms. The API login (`POST /api2/auth-token/`), WebDAV and sync clients are
    /// never challenged: they cannot run the browser solver.
    #[serde(default)]
    pub captcha_mode: CaptchaMode,
    /// Base challenge difficulty: the number of leading zero bits the browser must
    /// find in `SHA-256(seed || nonce)`. Higher costs more CPU on every visitor.
    /// Accepted range: [`CAPTCHA_MIN_BITS`]–[`CAPTCHA_MAX_BITS`].
    #[serde(default = "default_captcha_difficulty_bits")]
    pub captcha_difficulty_bits: u8,
    /// Failed sign-in attempts from one client address before `adaptive` mode asks
    /// for a challenge. `0` means every sign-in asks. Ignored in the other modes.
    #[serde(default = "default_captcha_trigger_after")]
    pub captcha_trigger_after: u32,
    /// Whether to show the "Forgot password?" link on the login page
    /// and enable the password reset flow.
    #[serde(default = "default_true")]
    pub enable_password_reset: bool,
    /// Minimum password length for new registrations and password changes.
    #[serde(default = "default_password_min_length")]
    pub password_min_length: u32,
    /// Require at least one letter and one digit in passwords.
    #[serde(default)]
    pub require_strong_password: bool,
    /// Max password reset requests per IP per hour (0 = unlimited).
    #[serde(default = "default_five")]
    pub password_reset_max_per_hour: u32,
    /// Max registration attempts per IP per hour (0 = unlimited).
    #[serde(default = "default_five")]
    pub registration_max_per_hour: u32,
    /// Max TOTP verification attempts per user per 5 minutes (0 = unlimited).
    #[serde(default = "default_five")]
    pub totp_max_attempts: u32,
    /// Max password attempts for password-protected share/upload links per IP
    /// per hour (0 = unlimited).
    #[serde(default = "default_five")]
    pub link_password_max_per_hour: u32,
    /// Max failed encrypted-library password checks per (user, repo) per hour
    /// (0 = unlimited). The library KDF iteration count is fixed by the Seafile
    /// wire protocol, so this is the control against online guessing.
    #[serde(default = "default_five")]
    pub repo_password_max_per_hour: u32,
    /// Max anonymous share-link downloads per IP per minute (0 = unlimited).
    #[serde(default = "default_share_download_max_per_minute")]
    pub share_download_max_per_minute: u32,
    /// Max failed WebDAV authentications per IP per 5 minutes (0 = unlimited).
    ///
    /// WebDAV keys are server-generated and high-entropy, so guessing one is
    /// infeasible; this only bounds the work an unauthenticated client can
    /// force, and successful requests are never counted.
    #[serde(default = "default_webdav_failures")]
    pub webdav_max_failures_per_5min: u32,
    /// Max reindex requests per user per hour (0 = unlimited).
    #[serde(default = "default_five")]
    pub reindex_max_per_hour: u32,
    /// Max search requests per user per minute (0 = unlimited).
    #[serde(default = "default_search_max_per_minute")]
    pub search_max_per_minute: u32,
    /// Lifetimes, in days, offered as presets when creating an API key.
    ///
    /// Presentation only: the service accepts any lifetime up to
    /// [`Self::api_key_max_ttl_days`]. Env:
    /// `NANOFILE_AUTH_API_KEY_TTL_PRESETS_DAYS` (comma-separated).
    #[serde(default = "default_api_key_ttl_presets_days")]
    pub api_key_ttl_presets_days: Vec<u64>,
    /// Upper bound on an API key's lifetime, in days (0 = no bound).
    ///
    /// When set, a key may neither outlive this horizon nor be created without
    /// an expiry. Env: `NANOFILE_AUTH_API_KEY_MAX_TTL_DAYS`.
    #[serde(default)]
    pub api_key_max_ttl_days: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            password_hash_iterations: default_password_hash_iterations(),
            api_token_ttl_days: default_api_token_ttl_days(),
            sync_token_ttl_days: default_sync_token_ttl_days(),
            max_login_attempts: default_five(),
            lockout_duration_secs: default_lockout_duration_secs(),
            max_distinct_usernames_per_ip: default_spray_distinct_usernames(),
            sso_link_max_per_hour: default_sso_link_max_per_hour(),
            enable_registration: default_true(),
            require_invitation: default_true(),
            captcha_mode: CaptchaMode::Off,
            captcha_difficulty_bits: default_captcha_difficulty_bits(),
            captcha_trigger_after: default_captcha_trigger_after(),
            enable_password_reset: default_true(),
            password_min_length: default_password_min_length(),
            require_strong_password: false,
            password_reset_max_per_hour: default_five(),
            registration_max_per_hour: default_five(),
            totp_max_attempts: default_five(),
            link_password_max_per_hour: default_five(),
            repo_password_max_per_hour: default_five(),
            share_download_max_per_minute: default_share_download_max_per_minute(),
            webdav_max_failures_per_5min: default_webdav_failures(),
            reindex_max_per_hour: default_five(),
            search_max_per_minute: default_search_max_per_minute(),
            api_key_ttl_presets_days: default_api_key_ttl_presets_days(),
            api_key_max_ttl_days: 0,
        }
    }
}

impl AuthConfig {
    /// The configured base challenge difficulty, clamped to the accepted range.
    ///
    /// Values outside [`CAPTCHA_MIN_BITS`]–[`CAPTCHA_MAX_BITS`] are rejected on
    /// save and reported at startup; clamping here keeps an existing config file
    /// from making every challenge trivial or unusable.
    pub fn captcha_bits(&self) -> u8 {
        self.captcha_difficulty_bits
            .clamp(CAPTCHA_MIN_BITS, CAPTCHA_MAX_BITS)
    }
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct AdminInitConfig {
    pub email: Option<String>,
    pub password: Option<String>,
}

fn default_password_hash_iterations() -> u32 {
    600000
}
fn default_api_token_ttl_days() -> u64 {
    180
}
fn default_sync_token_ttl_days() -> u64 {
    365
}
fn default_api_key_ttl_presets_days() -> Vec<u64> {
    vec![7, 30, 90, 180, 365]
}
fn default_lockout_duration_secs() -> u64 {
    900
}
fn default_captcha_difficulty_bits() -> u8 {
    17
}
fn default_captcha_trigger_after() -> u32 {
    2
}
fn default_true() -> bool {
    true
}

/// Seahub's default. Only 2 and 4 are supported by the key schedule (both
/// AES-256-CBC).
fn default_encrypted_library_version() -> i32 {
    2
}
fn default_five() -> u32 {
    5
}
fn default_sso_link_max_per_hour() -> u32 {
    30
}

fn default_spray_distinct_usernames() -> u32 {
    20
}
fn default_webdav_failures() -> u32 {
    30
}
fn default_share_download_max_per_minute() -> u32 {
    30
}
fn default_search_max_per_minute() -> u32 {
    60
}
fn default_password_min_length() -> u32 {
    8
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Whether logs go to a rotating file instead of stdout. Unset = auto: file for a
    /// desktop (tray) or Windows GUI build, stdout otherwise (servers, Docker).
    /// Env: NANOFILE_LOG_FILE_ENABLED
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_enabled: Option<bool>,
    /// Log file path. Unset: `nanofile.log` next to the binary, written back here
    /// (absolute) so a login-started instance uses the same file. A relative path is
    /// resolved against the binary's directory, never the working directory.
    /// Env: NANOFILE_LOG_FILE
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Per-file size cap in MB before rotation, clamped to a minimum of 1.
    #[serde(default = "default_log_max_file_size_mb")]
    pub max_file_size_mb: u64,
    /// Rotated backups to keep (`nanofile.log.1`, `.2`, …); 0 truncates in place.
    #[serde(default = "default_log_max_backups")]
    pub max_backups: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file_enabled: None,
            file: None,
            max_file_size_mb: default_log_max_file_size_mb(),
            max_backups: default_log_max_backups(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_max_file_size_mb() -> u64 {
    10
}

fn default_log_max_backups() -> u32 {
    3
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GcConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gc_interval_hours")]
    pub interval_hours: u64,
    /// Blocks written less than this many seconds ago are never deleted, even when the
    /// reference snapshot cannot see the commit that will reference them: an upload
    /// writes its blocks before committing them, so a pass started just before that
    /// commit would otherwise delete live data.
    /// Env: NANOFILE_GC_MIN_BLOCK_AGE_SECS
    #[serde(default = "default_gc_min_block_age_secs")]
    pub min_block_age_secs: u64,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: default_gc_interval_hours(),
            min_block_age_secs: default_gc_min_block_age_secs(),
        }
    }
}

fn default_gc_interval_hours() -> u64 {
    24
}

fn default_gc_min_block_age_secs() -> u64 {
    3600
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IndexConfig {
    pub enabled: bool,
    pub index_dir: PathBuf,
    /// Let the background pass index files the index has never seen (or whose
    /// extractor changed) when the server is quiet. Env: NANOFILE_INDEX_BACKFILL_ENABLED
    #[serde(default = "default_true")]
    pub backfill_enabled: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            index_dir: PathBuf::from("data/index"),
            backfill_enabled: true,
        }
    }
}

/// The sandbox that bounds every child process which reads untrusted bytes.
///
/// Documents, images, EXIF and media are parsed or decoded in a separate process
/// with bounded memory/CPU and whatever file, network and process confinement the
/// platform supplies. Which layers a host gives is measured and reported, not
/// configured.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SandboxConfig {
    /// Master switch over every feature that parses untrusted bytes. With it off,
    /// documents are not parsed for the index, thumbnails/EXIF are not produced and
    /// avatars are not processed. Features are *disabled* rather than run unconfined
    /// in the server process. Env: NANOFILE_SANDBOX_ENABLED
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Load a `.env` file, unless `NANOFILE_DOTENV=0`.
///
/// `dotenvy` walks the working directory and its parents, so the file found is not
/// necessarily next to the config file — a `.env` written by another local user in
/// a shared run directory could supply the master `secret_key` or point the
/// database elsewhere. The file is still loaded (existing deployments rely on it),
/// but a permissive mode is called out so the risk is visible.
fn load_dotenv() {
    if std::env::var("NANOFILE_DOTENV").is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false")) {
        return;
    }
    let Ok(path) = dotenvy::dotenv() else {
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                eprintln!(
                    "[nanofile] WARN: {} is readable by other local users (mode {mode:o}); a \
                     .env file can supply the master secret_key or the database URL. Run \
                     `chmod 600 {}` or set NANOFILE_DOTENV=0.",
                    path.display(),
                    path.display()
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// One relative state path handled by [`Config::resolve_state_paths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathResolution {
    /// Config field, e.g. `database.url`, for the startup log.
    pub field: &'static str,
    /// The path the field names (absolute once `rewritten`).
    pub path: PathBuf,
    /// True when the field was rewritten under the base directory.
    pub rewritten: bool,
}

/// Directory that every relative state path in a [`Config`] resolves against.
///
/// The working directory cannot be used: a Windows `Run` value carries no start
/// directory, so a login-started desktop instance inherits the launcher's
/// (`C:\Windows\System32`), and a double-clicked binary depends on the shortcut's
/// "Start in" field. The working directory is only a fallback when the binary's
/// location cannot be determined at all.
pub fn state_path_base() -> PathBuf {
    crate::common::util::exe_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// The per-user configuration file a first-run setup writes
/// (`~/.nanofile/config.toml`), or `None` when home cannot be determined.
pub fn user_config_path() -> Option<PathBuf> {
    crate::common::util::home_dir()
        .map(|home| home.join(USER_CONFIG_DIR_NAME).join(CONFIG_FILE_NAME))
}

/// Why a start reads the configuration file it reads.
///
/// A path the operator *named* must exist (a moved folder leaves an auto-start
/// entry pointing at nothing, and falling back to defaults would start a second
/// instance), while a default path may legitimately be absent for a first run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathOrigin {
    /// `--config <path>`.
    CommandLine,
    /// `NANOFILE_CONFIG=<path>`.
    Environment,
    /// `./config.toml`, kept for compatibility with pre-per-user deployments.
    LegacyDefault,
    /// `<home>/.nanofile/config.toml`.
    UserDefault,
    /// No file named and none exists: `path` is where a first run writes.
    Unconfigured,
}

impl ConfigPathOrigin {
    /// The name to report in a message about a path the operator named.
    pub fn named_by(self) -> Option<&'static str> {
        match self {
            ConfigPathOrigin::CommandLine => Some("--config"),
            ConfigPathOrigin::Environment => Some(CONFIG_PATH_ENV),
            _ => None,
        }
    }
}

/// A resolved configuration path plus why it was chosen.
#[derive(Debug, Clone)]
pub struct ConfigPath {
    pub path: PathBuf,
    pub origin: ConfigPathOrigin,
}

impl ConfigPath {
    /// Whether the operator named this path, which makes a missing file an
    /// error rather than a zero-config start.
    pub fn is_explicit(&self) -> bool {
        self.origin.named_by().is_some()
    }

    /// Whether nothing is configured yet, which is the state a first-run setup
    /// exists for.
    pub fn is_unconfigured(&self) -> bool {
        self.origin == ConfigPathOrigin::Unconfigured
    }
}

/// Resolve which configuration file a start reads.
///
/// Order, first match wins: `--config`, `NANOFILE_CONFIG`, `./config.toml` (the
/// historical default, so existing deployments keep reading the file they always
/// did), `<home>/.nanofile/config.toml`, then "nothing configured" — where `path`
/// is where a first run writes and the loader uses defaults plus the environment.
///
/// Loads `.env` first, so a `NANOFILE_CONFIG` written there counts like an export.
pub fn resolve_config_path(explicit: Option<&Path>) -> ConfigPath {
    load_dotenv();
    let env_config = std::env::var_os(CONFIG_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let legacy = PathBuf::from(DEFAULT_CONFIG_PATH);
    let legacy = legacy.is_file().then_some(legacy);
    let user = user_config_path().filter(|path| path.is_file());
    let write_default = user_config_path().unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));

    let resolved = resolve_config_path_from(explicit, env_config, legacy, user, write_default);
    if resolved.is_unconfigured() && user_config_path().is_none() {
        // `tracing` is not initialized this early, and a machine with no home directory
        // whose configuration will be written into the working directory is worth one
        // line on stderr.
        eprintln!(
            "[nanofile] WARN: no home directory in the environment; a first-run setup will \
             write its configuration to {}",
            resolved.path.display()
        );
    }
    resolved
}

/// [`resolve_config_path`] with every input passed in, so precedence is testable.
fn resolve_config_path_from(
    explicit: Option<&Path>,
    env_config: Option<PathBuf>,
    legacy: Option<PathBuf>,
    user_existing: Option<PathBuf>,
    write_default: PathBuf,
) -> ConfigPath {
    let (path, origin) = match (explicit, env_config, legacy, user_existing) {
        (Some(path), _, _, _) => (path.to_path_buf(), ConfigPathOrigin::CommandLine),
        (None, Some(path), _, _) => (path, ConfigPathOrigin::Environment),
        (None, None, Some(path), _) => (path, ConfigPathOrigin::LegacyDefault),
        (None, None, None, Some(path)) => (path, ConfigPathOrigin::UserDefault),
        (None, None, None, None) => (write_default, ConfigPathOrigin::Unconfigured),
    };
    ConfigPath { path, origin }
}

/// Whether any environment variable that supplies configuration is set, by name
/// only; [`NON_CONFIG_ENV_VARS`] lists the names that are not configuration.
pub fn environment_supplies_config() -> bool {
    names_supply_config(std::env::vars_os().map(|(name, _)| name))
}

fn names_supply_config(names: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    names.into_iter().any(|name| {
        let Some(name) = name.to_str() else {
            return false;
        };
        name.starts_with("NANOFILE_") && !NON_CONFIG_ENV_VARS.contains(&name)
    })
}

/// Whether this start should run the first-run setup instead of serving.
///
/// True only when nothing named a configuration file, no file exists, and the
/// environment supplies nothing. Any `--config`, `NANOFILE_CONFIG`, `./config.toml`,
/// per-user file or `NANOFILE_*` means this installation is configured, even if the
/// configuration turns out to be unusable (that is a different report).
pub fn needs_first_run_setup(resolved: &ConfigPath) -> bool {
    resolved.is_unconfigured() && !environment_supplies_config()
}

/// The database file of a `sqlite:` URL, with its query string, or `None` for a
/// different backend, an in-memory database or no file.
///
/// Split exactly the way sqlx does (`SqliteConnectOptions::from_str`: strip the
/// scheme and the optional `//`, then cut at the first `?`). `trim_start_matches`
/// strips every leading repetition, which is why `sqlite:///var/lib/x.db` keeps
/// its leading slash and stays absolute.
pub(crate) fn sqlite_url_file(url: &str) -> Option<(&str, Option<&str>)> {
    if !url.starts_with("sqlite:") {
        return None;
    }
    let rest = url
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:");
    let (file, params) = match rest.split_once('?') {
        Some((file, params)) => (file, Some(params)),
        None => (rest, None),
    };
    // `:memory:`, `file::memory:?cache=shared` and `?mode=memory` name no file.
    if file.is_empty() || file == ":memory:" || file.starts_with("file::memory") {
        return None;
    }
    Some((file, params))
}

/// Rewrite a relative directory path under `base`, or record that it is kept
/// as configured (see [`Config::resolve_state_paths`]).
fn resolve_dir(
    field: &'static str,
    path: &mut PathBuf,
    base: &Path,
    keep: bool,
    out: &mut Vec<PathResolution>,
) {
    if path.is_absolute() {
        return;
    }
    if !keep {
        *path = base.join(&*path);
    }
    out.push(PathResolution {
        field,
        path: path.clone(),
        rewritten: !keep,
    });
}

impl Config {
    /// Reject a configuration the server could not run with.
    ///
    /// Called before an administrator's save is persisted, so a value that would make
    /// the next start fail is refused on the page that produced it. Also called at
    /// startup, where failure is reported and the process still starts (an existing
    /// deployment must not be bricked by a value it already had).
    ///
    /// Returns a message meant to be shown to the operator. Server-only checks (the
    /// email address parser) are added by the settings service.
    pub fn validate(&self) -> Result<(), String> {
        self.server.validate_encrypted_library()?;
        self.storage.validate_block_encryption_mode()?;

        if self.server.version.trim().is_empty() {
            return Err("server.version must not be empty".to_string());
        }
        if self.server.port == 0 {
            return Err("server.port must be between 1 and 65535".to_string());
        }
        if self.server.max_json_body_mb == 0 {
            return Err(
                "server.max_json_body_mb must be at least 1; 0 would reject every JSON \
                 request"
                    .to_string(),
            );
        }
        if self.server.site_url.trim().is_empty() {
            return Err("server.site_url must not be empty".to_string());
        }
        for origin in &self.server.cors_allowed_origins {
            if !origin.contains("://") {
                return Err(format!(
                    "server.cors_allowed_origins entry {origin:?} is not an origin \
                     (expected scheme://host[:port])"
                ));
            }
        }
        let pwd_hash_algo_is_set = self
            .server
            .encrypted_library_pwd_hash_algo
            .as_deref()
            .is_some_and(|algo| !algo.trim().is_empty());
        if self.server.encrypted_library_pwd_hash_params.is_some() && !pwd_hash_algo_is_set {
            return Err("server.encrypted_library_pwd_hash_params is set without \
                 encrypted_library_pwd_hash_algo; the parameters would be ignored"
                .to_string());
        }

        if self.database.url.trim().is_empty() {
            return Err("database.url must not be empty".to_string());
        }

        for (field, dir) in [
            ("storage.block_dir", &self.storage.block_dir),
            ("storage.temp_dir", &self.storage.temp_dir),
            ("storage.thumbnail_dir", &self.storage.thumbnail_dir),
            ("storage.avatar_dir", &self.storage.avatar_dir),
        ] {
            if dir.as_os_str().is_empty() {
                return Err(format!("{field} must not be empty"));
            }
        }
        if self.storage.ffmpeg_path.trim().is_empty() {
            return Err(
                "storage.ffmpeg_path must not be empty; set it to a command name or a path"
                    .to_string(),
            );
        }

        if self.auth.password_min_length < 1 {
            return Err("auth.password_min_length must be at least 1".to_string());
        }
        if self.auth.password_hash_iterations < 1 {
            return Err("auth.password_hash_iterations must be at least 1".to_string());
        }
        if !(CAPTCHA_MIN_BITS..=CAPTCHA_MAX_BITS).contains(&self.auth.captcha_difficulty_bits) {
            return Err(format!(
                "auth.captcha_difficulty_bits must be between {CAPTCHA_MIN_BITS} and \
                 {CAPTCHA_MAX_BITS}"
            ));
        }

        if self.email.port == 0 {
            return Err("email.port must be between 1 and 65535".to_string());
        }
        if self.email.timeout_secs < 1 {
            return Err("email.timeout_secs must be at least 1 second".to_string());
        }
        if self.email.max_attempts < 1 {
            return Err("email.max_attempts must be at least 1".to_string());
        }
        if !self.email.username.trim().is_empty() && self.email.host.trim().is_empty() {
            return Err(
                "email.username is set without email.host; there is nothing to authenticate to"
                    .to_string(),
            );
        }

        if self.logging.level.trim().is_empty() {
            return Err("logging.level must not be empty".to_string());
        }
        if self.logging.max_file_size_mb < 1 {
            return Err("logging.max_file_size_mb must be at least 1".to_string());
        }

        if self.sync.verify_fs_objects.trim().is_empty() {
            return Err("sync.verify_fs_objects must be strict, log or off".to_string());
        }

        Ok(())
    }

    /// Repair externally visible URL fields at load time.
    ///
    /// Accepts a full `scheme://host[:port]` URL or a bare `host[:port]`, rewriting the
    /// latter to `http://host[:port]`; empty becomes the built-in default. Without this
    /// a schemeless value yielded a garbage origin, so every browser login/2FA request
    /// failed its Origin check with a confusing message.
    ///
    /// `https` must be written explicitly: silently upgrading a schemeless value would
    /// flip `Secure` cookies and HSTS on for a server not served over TLS.
    pub fn normalize(&mut self) {
        let trimmed = self.server.site_url.trim().to_string();
        if trimmed.is_empty() {
            self.server.site_url = default_site_url();
            return;
        }
        if !trimmed.contains("://") {
            // tracing_subscriber is not initialized yet at load time.
            eprintln!(
                "[nanofile] WARN: site_url \"{trimmed}\" has no scheme; assuming http:// \
                 (write https:// explicitly when the server is served over TLS)"
            );
            self.server.site_url = format!("http://{trimmed}");
            return;
        }
        self.server.site_url = trimmed;
    }

    /// Rewrite every relative state path so it names the same file no matter which
    /// directory the process was started from.
    ///
    /// "Relative" means relative to the working directory, which for a login-started
    /// desktop instance is a system directory (see [`state_path_base`]). The database,
    /// block store, thumbnail and avatar caches and search index are resolved against
    /// `base`. `[logging] file` is not touched (`logging::init` resolves it against the
    /// same directory) and neither is `storage.ffmpeg_path`, a `PATH` command.
    ///
    /// An installation whose relative paths already exist under its working directory
    /// is left exactly as configured, so upgrading never moves data; that decision is
    /// taken once for the whole config, because rewriting only missing paths would
    /// split one installation's state across two roots.
    ///
    /// The returned [`PathResolution`]s list every relative field rewritten or kept.
    pub fn resolve_state_paths(&mut self, base: &Path) -> Vec<PathResolution> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.resolve_state_paths_in(base, &cwd)
    }

    /// Every directory the server keeps state in, with the config field naming it.
    ///
    /// The one list of "the server's directories": startup checks and the Windows
    /// preflight walk all of them, and `run_server` creates the subset it owns, so a
    /// state directory cannot be created in one place and left unchecked in another.
    /// A URL that names no file (`:memory:`, another backend) contributes nothing;
    /// otherwise the database's directory is the parent of its file.
    ///
    /// The paths are exactly as configured, so call [`Self::resolve_state_paths`] first.
    pub fn state_dirs(&self) -> Vec<(&'static str, PathBuf)> {
        let mut dirs = Vec::new();
        if let Some((file, _)) = sqlite_url_file(&self.database.url) {
            let parent = Path::new(file).parent().unwrap_or_else(|| Path::new("."));
            dirs.push((
                "database.url",
                if parent.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    parent.to_path_buf()
                },
            ));
        }
        dirs.push(("storage.block_dir", self.storage.block_dir.clone()));
        dirs.push(("storage.temp_dir", self.storage.temp_dir.clone()));
        dirs.push(("storage.thumbnail_dir", self.storage.thumbnail_dir.clone()));
        dirs.push(("storage.avatar_dir", self.storage.avatar_dir.clone()));
        dirs.push(("index.index_dir", self.index.index_dir.clone()));
        dirs
    }

    /// [`Self::resolve_state_paths`] with an explicit working directory, for tests.
    fn resolve_state_paths_in(&mut self, base: &Path, cwd: &Path) -> Vec<PathResolution> {
        // Absolute paths are unambiguous already and are never touched.
        let relative_database = sqlite_url_file(&self.database.url)
            .map(|(file, _)| PathBuf::from(file))
            .filter(|file| !file.is_absolute());
        let keep = relative_database
            .iter()
            .chain([
                &self.storage.block_dir,
                &self.storage.temp_dir,
                &self.storage.thumbnail_dir,
                &self.storage.avatar_dir,
                &self.index.index_dir,
            ])
            .any(|path| cwd.join(path).exists());

        let mut out = Vec::new();

        // `database.url` needs its URL rebuilt, and its query string must survive.
        if !keep && let Some((file, params)) = sqlite_url_file(&self.database.url) {
            let params = params.map(str::to_string);
            let path = base.join(file);
            self.database.url = match &params {
                Some(params) => format!("sqlite:{}?{params}", path.display()),
                None => format!("sqlite:{}", path.display()),
            };
        }
        if let Some(file) = relative_database {
            out.push(PathResolution {
                field: "database.url",
                path: if keep { file } else { base.join(file) },
                rewritten: !keep,
            });
        }

        for (field, path) in [
            ("storage.block_dir", &mut self.storage.block_dir),
            ("storage.temp_dir", &mut self.storage.temp_dir),
            ("storage.thumbnail_dir", &mut self.storage.thumbnail_dir),
            ("storage.avatar_dir", &mut self.storage.avatar_dir),
            ("index.index_dir", &mut self.index.index_dir),
        ] {
            resolve_dir(field, path, base, keep, &mut out);
        }

        out
    }

    pub fn load() -> anyhow::Result<Self> {
        Self::load_tracked().map(|loaded| loaded.config)
    }

    /// [`Self::load`] plus the catalog keys the environment supplied. The server needs
    /// this to say whether a database save would take effect.
    pub fn load_tracked() -> anyhow::Result<LoadedConfig> {
        // The precedence a server start uses, minus "a named file must exist": a tool
        // should still see the per-user file a first-run setup writes.
        let resolved = resolve_config_path(None);
        Self::load_from_tracked(&resolved.path)
    }

    /// Load the configuration a *server start* should use.
    ///
    /// Like [`Self::load_tracked`], but a file the operator named explicitly
    /// (`--config`, or `NANOFILE_CONFIG`, including one set in `.env`) **must exist**.
    ///
    /// These paths outlive a single process: auto-start entries and Windows service
    /// registrations record the path they were created with, so a moved folder leaves
    /// them pointing at nothing. Falling back to defaults there would silently start a
    /// *different* instance on the default port next to the binary. The zero-config
    /// convenience stays for the unnamed default (`./config.toml`, then the per-user
    /// file), where "no file" is a legitimate setup.
    pub fn load_for_start(explicit_path: Option<&Path>) -> anyhow::Result<LoadedConfig> {
        Self::load_for_start_with_path(explicit_path).map(|(loaded, _)| loaded)
    }

    /// [`Self::load_for_start`] plus the path it resolved and why: an auto-start entry
    /// records the path the loader read, and whether anything was named decides
    /// whether a missing file is an error or a first run.
    pub fn load_for_start_with_path(
        explicit_path: Option<&Path>,
    ) -> anyhow::Result<(LoadedConfig, ConfigPath)> {
        let resolved = resolve_config_path(explicit_path);
        if resolved.is_explicit() && !resolved.path.is_file() {
            let named_by = resolved.origin.named_by().unwrap_or("the environment");
            anyhow::bail!(
                "configuration file {} does not exist (named by {named_by}); starting with \
                 built-in defaults would use a different database and listener, so fix the path \
                 or drop the explicit setting",
                resolved.path.display()
            );
        }
        let loaded = Self::load_from_tracked(&resolved.path)?;
        Ok((loaded, resolved))
    }

    /// Load a config file. Keys the file omits keep their built-in defaults in memory;
    /// the file is never rewritten, so what the operator wrote stays on disk. Every key
    /// is discoverable at `/sysadmin/settings/` with its environment variable and
    /// effective value, which makes a partial file a complete configuration.
    ///
    /// A missing file falls back to built-in defaults (plus env overrides) for a
    /// zero-config start; other I/O errors and unparseable files still fail.
    pub fn load_from(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::load_from_tracked(path).map(|loaded| loaded.config)
    }
    /// [`Self::load_from`] plus the environment-supplied catalog keys.
    pub fn load_from_tracked(path: impl AsRef<Path>) -> anyhow::Result<LoadedConfig> {
        let path = path.as_ref();
        let original = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "[nanofile] config file {} not found; using built-in defaults",
                    path.display()
                );
                let mut config = Config::default();
                let mut env_keys = EnvKeys::new();
                config.apply_env_overrides(&mut env_keys);
                config.normalize();
                return Ok(LoadedConfig { config, env_keys });
            }
            Err(e) => {
                return Err(anyhow::anyhow!("failed to read {}: {e}", path.display()));
            }
        };

        // Missing fields are serde defaults in memory; the document is left alone, so a
        // partial file stays as written. The live config carries the master secret, so a
        // permissive mode is worth an explicit warning.
        warn_if_group_or_world_readable(path);

        let mut config: Config = toml::from_str(&original)?;
        // Retired file keys are folded before the environment layer, so a variable
        // still beats a value written for an older release.
        config.fold_retired_file_keys(&original);
        let mut env_keys = EnvKeys::new();
        config.apply_env_overrides(&mut env_keys);
        config.normalize();
        Ok(LoadedConfig { config, env_keys })
    }

    /// Fold retired config-file keys into the settings that replaced them.
    ///
    /// `[server] share_link_enabled` was one master switch over both directions of
    /// sharing. serde cannot map one key onto two fields, so the raw document is
    /// consulted right after deserialization; the value is applied to **both**
    /// directions, or an installation that turned sharing off would silently
    /// reopen anonymous uploads by upgrading.
    ///
    /// `[storage] max_storage_bytes` was renamed to `default_user_quota_bytes`: the
    /// value was always the quota of one account with none of its own, never a cap
    /// on every account together. Reading the raw document (rather than a serde
    /// alias) lets a file carrying both names keep the new one instead of failing on
    /// a duplicate field, and tells the operator the old name is going away.
    fn fold_retired_file_keys(&mut self, original: &str) {
        let Ok(doc) = toml::from_str::<toml::Value>(original) else {
            return;
        };

        if let Some(value) = doc
            .get("server")
            .and_then(|server| server.get("share_link_enabled"))
            .and_then(toml::Value::as_bool)
        {
            tracing::warn!(
                "[server] share_link_enabled is retired and sets both share_download_enabled \
                 and share_upload_enabled; use the two new keys instead"
            );
            self.server.share_download_enabled = value;
            self.server.share_upload_enabled = value;
        }

        // The new name wins when a file carries both, so the rename never means
        // "whichever key serde happened to see last".
        let storage = doc.get("storage");
        if storage
            .and_then(|storage| storage.get("default_user_quota_bytes"))
            .is_none()
            && let Some(value) = storage
                .and_then(|storage| storage.get("max_storage_bytes"))
                .and_then(toml::Value::as_integer)
        {
            tracing::warn!(
                "[storage] max_storage_bytes was renamed to default_user_quota_bytes \
                 (the default quota of an account with none of its own); use the new key"
            );
            self.storage.default_user_quota_bytes = value.max(0) as u64;
        }
    }

    /// Apply every `NANOFILE_*` override.
    ///
    /// Driven from [`crate::settings::CATALOG`], so a setting cannot exist without its
    /// variable being read, and the admin UI shows the variable the server consults.
    /// `env_keys` records which keys the environment supplied, making origins
    /// reportable.
    ///
    /// A value that does not parse is ignored with a warning: a typo must not stop the
    /// process from starting. `*_FILE` secrets are read after the plain variables so
    /// the file wins, keeping secrets out of process listings.
    fn apply_env_overrides(&mut self, env_keys: &mut EnvKeys) {
        for def in crate::settings::CATALOG {
            let Some(var) = def.env else { continue };
            let Ok(value) = std::env::var(var) else {
                continue;
            };
            if def.kind == crate::settings::Kind::Secret
                && let Some(file_var) = def.env_file
            {
                tracing::warn!(
                    "{var} is set via environment variable. Consider using {file_var} \
                     instead, which is less likely to leak via process listings or logs."
                );
            }
            match (def.set)(self, &value) {
                Ok(()) => {
                    env_keys.insert(def.key, var);
                }
                Err(e) => tracing::warn!("ignoring {var}: {e}"),
            }
        }

        for def in crate::settings::CATALOG {
            let Some(var) = def.env_file else { continue };
            let Ok(path) = std::env::var(var) else {
                continue;
            };
            match std::fs::read_to_string(&path) {
                Ok(secret) => match (def.set)(self, secret.trim()) {
                    Ok(()) => {
                        // Report the plain variable, as before, even when a `*_FILE`
                        // supplied the value.
                        env_keys.insert(def.key, def.env.unwrap_or(var));
                    }
                    Err(e) => tracing::warn!("ignoring {var}: {e}"),
                },
                Err(e) => {
                    tracing::error!("Failed to read {var} from {path}: {e}");
                }
            }
        }

        self.apply_retired_env_aliases(env_keys, |name| std::env::var(name).ok());

        // The `[settings]` policy is not a catalog entry: it decides what a
        // database row means, so it can only come from outside that layer.
        if let Ok(v) = std::env::var("NANOFILE_SETTINGS_CONFIG_POLICY") {
            self.settings.config_policy = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_SETTINGS_CONFIG_OVERRIDE_KEYS") {
            self.settings.config_override_keys = v
                .split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Ok(v) = std::env::var("NANOFILE_SETTINGS_REFRESH_INTERVAL_SECS")
            && let Ok(parsed) = v.trim().parse::<u64>()
        {
            self.settings.refresh_interval_secs = parsed;
        }
    }

    /// Read environment names that this release retired as aliases.
    ///
    /// A retired variable may stand for more than one setting: the sharing master
    /// switch `NANOFILE_SERVER_SHARE_LINK_ENABLED` used to close both directions
    /// and now feeds the two directional switches. The alias applies to every key
    /// in its group that no current variable set, so an operator who migrated only
    /// one direction still keeps the old intent for the other. Origins report the
    /// variable that actually supplied the value.
    ///
    /// A plain rename is the same mechanism with a single key
    /// (`NANOFILE_STORAGE_MAX_STORAGE_BYTES` → `..._DEFAULT_USER_QUOTA_BYTES`).
    ///
    /// `lookup` is injected so a unit test can exercise the mapping without
    /// mutating this process's environment (the test suite runs in parallel).
    fn apply_retired_env_aliases(
        &mut self,
        env_keys: &mut EnvKeys,
        lookup: impl Fn(&str) -> Option<String>,
    ) {
        /// A retired variable and the catalog keys it now feeds, plus the name to
        /// recommend in the warning.
        struct RetiredAlias {
            old: &'static str,
            keys: &'static [&'static str],
            replacement: &'static str,
        }

        const RETIRED: &[RetiredAlias] = &[
            RetiredAlias {
                old: "NANOFILE_SERVER_SHARE_LINK_ENABLED",
                keys: &[
                    "server.share_download_enabled",
                    "server.share_upload_enabled",
                ],
                replacement: "NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED / \
                              NANOFILE_SERVER_SHARE_UPLOAD_ENABLED",
            },
            RetiredAlias {
                old: "NANOFILE_AUTH_ENABLE_INVITATIONS",
                keys: &["auth.enable_registration"],
                replacement: "NANOFILE_AUTH_ENABLE_REGISTRATION",
            },
            RetiredAlias {
                old: "NANOFILE_STORAGE_MAX_STORAGE_BYTES",
                keys: &["storage.default_user_quota_bytes"],
                replacement: "NANOFILE_STORAGE_DEFAULT_USER_QUOTA_BYTES",
            },
        ];

        for alias in RETIRED {
            let Some(value) = lookup(alias.old) else {
                continue;
            };
            for key in alias.keys {
                if env_keys.contains_key(key) {
                    tracing::warn!(
                        "{} is deprecated and ignored for {key} because its own \
                         variable is set",
                        alias.old
                    );
                    continue;
                }
                let Some(def) = crate::settings::find(key) else {
                    continue;
                };
                match (def.set)(self, &value) {
                    Ok(()) => {
                        env_keys.insert(def.key, alias.old);
                        tracing::warn!("{} is deprecated; use {}", alias.old, alias.replacement);
                    }
                    Err(e) => tracing::warn!("ignoring {}: {e}", alias.old),
                }
            }
        }
    }
}

/// Persist a single string value into the config file, preserving comments and
/// formatting and creating the section if missing. This is the one write the loader
/// does: a value *resolved* at startup (the log path next to the binary) is baked in
/// so every later run reads the same explicit path. Built-in defaults are never
/// written; an existing value is overwritten only by this explicit call.
pub fn write_config_value(
    path: impl AsRef<Path>,
    section: &str,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    let path = path.as_ref();
    let original = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = original
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    let table = doc
        .entry(section)
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[{section}] in {} is not a table", path.display()))?;
    table.insert(key, toml_edit::value(value));
    persist_with_backup(path, &doc.to_string()).with_context(|| {
        format!(
            "failed to persist [{}] {} in {}",
            section,
            key,
            path.display()
        )
    })
}

/// Why a new configuration file could not be written.
#[derive(Debug, thiserror::Error)]
pub enum WriteConfigError {
    /// A configuration file is already there. Its own variant so first-run setup can
    /// report "somebody already configured this" rather than an I/O failure.
    #[error("{} already exists", .0.display())]
    AlreadyExists(PathBuf),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// A typed value a first run writes. Typed rather than a string: a port written as
/// `"8123"` is a configuration the loader rejects, and guessing the type ("all
/// digits") would break a future string setting that looks numeric.
#[derive(Debug, Clone)]
pub enum ConfigValue<'a> {
    /// TOML string, borrowed when the caller's text outlives the write.
    Text(std::borrow::Cow<'a, str>),
    /// Written as a bare TOML integer.
    Integer(u64),
    /// Written as `true` or `false`.
    Boolean(bool),
}

impl<'a> From<&'a str> for ConfigValue<'a> {
    fn from(value: &'a str) -> Self {
        ConfigValue::Text(std::borrow::Cow::Borrowed(value))
    }
}

impl<'a> From<&'a String> for ConfigValue<'a> {
    fn from(value: &'a String) -> Self {
        ConfigValue::Text(std::borrow::Cow::Borrowed(value.as_str()))
    }
}

impl From<String> for ConfigValue<'_> {
    fn from(value: String) -> Self {
        ConfigValue::Text(std::borrow::Cow::Owned(value))
    }
}

impl From<u16> for ConfigValue<'_> {
    fn from(value: u16) -> Self {
        ConfigValue::Integer(u64::from(value))
    }
}

impl From<bool> for ConfigValue<'_> {
    fn from(value: bool) -> Self {
        ConfigValue::Boolean(value)
    }
}

/// One `[section]` of a configuration file a first run writes.
pub struct ConfigSection<'a> {
    /// Section header, e.g. `server`.
    pub name: &'a str,
    /// Lines written as comments above the header (no `#` needed).
    pub comment: &'a str,
    /// `key = value` pairs in order; `toml_edit` handles quoting and escaping.
    pub entries: Vec<(&'a str, ConfigValue<'a>)>,
}

/// Create a **new** configuration file from scratch. Only a first-run setup calls
/// this; its job is the part that must not be re-implemented: an owner-only parent
/// directory, an owner-only atomic write (the file holds the master secret), and
/// **no overwrite** — enforced by reserving the path with `create_new`, so it holds
/// even if two first runs race.
pub fn write_new_config(
    path: impl AsRef<Path>,
    header: &str,
    sections: &[ConfigSection<'_>],
) -> Result<(), WriteConfigError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        crate::common::util::ensure_private_dir_only_if_created(parent)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", parent.display()))?;
    }

    let mut document = String::new();
    for line in header.lines() {
        if line.is_empty() {
            document.push('\n');
        } else {
            document.push_str("# ");
            document.push_str(line);
            document.push('\n');
        }
    }
    for section in sections {
        if !section.comment.is_empty() {
            for line in section.comment.lines() {
                document.push_str("# ");
                document.push_str(line);
                document.push('\n');
            }
        }
        document.push('[');
        document.push_str(section.name);
        document.push_str("]\n");
        for (key, value) in &section.entries {
            document.push_str(key);
            document.push_str(" = ");
            match value {
                // `toml_edit::value` renders a string the way TOML wants it.
                ConfigValue::Text(text) => {
                    document.push_str(&toml_edit::value(text.as_ref()).to_string())
                }
                ConfigValue::Integer(number) => document.push_str(&number.to_string()),
                ConfigValue::Boolean(value) => {
                    document.push_str(if *value { "true" } else { "false" })
                }
            }
            document.push('\n');
        }
        document.push('\n');
    }

    // `create_new` fails when the path exists, which a concurrent first run cannot
    // get around. The reservation is owner-only from the start, like the document
    // that replaces it.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Err(e) = options.open(path) {
        return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
            WriteConfigError::AlreadyExists(path.to_path_buf())
        } else {
            WriteConfigError::Other(anyhow::anyhow!("cannot create {}: {e}", path.display()))
        });
    }

    // Replace the reservation with the real document: temp file + rename, fsynced.
    atomic_write(path, &document).map_err(|e| {
        WriteConfigError::Other(e.context(format!("failed to write {}", path.display())))
    })
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}
fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(s)
}

/// Create `path` for writing: owner-only from the first byte, refusing to follow a symlink or to
/// reuse a file left behind by a crash.
///
/// The config file holds the master secret. Creating it with the process umask and tightening it
/// afterwards leaves a window in which another local user can read it — and the temporary name is
/// predictable, so plain `create` would also follow a symlink planted at it.
fn open_owner_only_new(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        // A leftover temporary file from a crash is this process's own to replace; nothing else
        // may reuse the name.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            options.open(path)
        }
        Err(e) => Err(e),
    }
}

/// Atomically replace `path` with `filled`, keeping the previous content as
/// `path.bak`. Fails without touching `path` when the backup or the write fails;
/// the backup is taken with `fs::copy` so file permissions carry over.
fn persist_with_backup(path: &Path, filled: &str) -> anyhow::Result<()> {
    std::fs::copy(path, backup_path(path)).with_context(|| {
        format!(
            "failed to back up {} to {}",
            path.display(),
            backup_path(path).display()
        )
    })?;
    // The backup is a second copy of the secrets: tighten its mode explicitly.
    if let Err(e) = restrict_to_owner(&backup_path(path)) {
        tracing::warn!(
            "could not restrict permissions on the config backup {}: {e}",
            backup_path(path).display()
        );
    }
    atomic_write(path, filled)
}

/// Restrict a file to its owner (`0600`) on Unix. The config file holds the master
/// `secret_key` (and possibly encryption keys or an admin password), so it must
/// never be group- or world-readable. No-op without Unix modes.
fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Warn (on stderr, because `tracing` is not initialized yet at load time) when
/// a config file is readable by users other than its owner.
fn warn_if_group_or_world_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                eprintln!(
                    "[nanofile] WARN: {} is accessible to other local users (mode {mode:o}) \
                     but holds the master secret_key; run `chmod 600 {}`.",
                    path.display(),
                    path.display()
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Write `content` via a same-directory temp file + rename, restricted to the owner
/// and fsynced, so the replaced file is never observed half-written. The temp file
/// is cleaned up on failure.
fn atomic_write(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp = tmp_path(path);
    let result = (|| -> anyhow::Result<()> {
        {
            // Owner-only from creation, so the secret is never readable by anyone else.
            let mut f = open_owner_only_new(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
        }
        // Belt and braces: the mode is already set at creation where that is possible.
        restrict_to_owner(&tmp)?;
        #[cfg(windows)]
        {
            // `rename` does not overwrite an existing target on Windows.
            let _ = std::fs::remove_file(path);
        }
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(path.parent().unwrap_or_else(|| Path::new("."))) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a ServerConfig from a partial TOML (required fields only).
    fn cfg(site_url: &str) -> ServerConfig {
        let toml = format!(
            r#"
addr = "0.0.0.0"
port = 8082
site_url = "{site_url}"
max_upload_size_mb = 4096
request_timeout_secs = 600
"#
        );
        toml::from_str(&toml).expect("valid server config")
    }

    #[test]
    fn site_url_is_default_detects_unconfigured() {
        assert!(cfg("http://127.0.0.1:8082").site_url_is_default());
        // Trailing slash variant is still the default.
        assert!(cfg("http://127.0.0.1:8082/").site_url_is_default());
        // Any explicitly configured address is not the default.
        assert!(!cfg("https://seafile.example.com").site_url_is_default());
        assert!(!cfg("http://192.168.1.100:8082").site_url_is_default());
    }

    #[test]
    fn site_url_origin_extracts_scheme_host_port() {
        assert_eq!(
            cfg("http://127.0.0.1:8082/some/path").site_url_origin(),
            "http://127.0.0.1:8082"
        );
        assert_eq!(
            cfg("https://seafile.example.com").site_url_origin(),
            "https://seafile.example.com"
        );
        // A bare host (as written by mistake) is treated as an http origin.
        assert_eq!(cfg("example.com").site_url_origin(), "http://example.com");
        // Query/fragment terminators also end the authority.
        assert_eq!(
            cfg("https://example.com:8443?x=1").site_url_origin(),
            "https://example.com:8443"
        );
    }

    /// Regression: `site_url_origin()` used to slice `&self.site_url[7..]`,
    /// which panicked for values shorter than the scheme or ending on a
    /// non-UTF-8 char boundary. It is reachable from every login / 2FA /
    /// CORS-origin path, so a config typo must never take the server down.
    #[test]
    fn site_url_origin_never_panics_on_malformed_input() {
        for value in ["", "http:/", "http://", "h", "例", "https://例"] {
            let origin = cfg(value).site_url_origin();
            assert!(!origin.is_empty(), "value={value:?}");
        }
        // Empty / scheme-only values fall back to the built-in default origin.
        assert_eq!(cfg("").site_url_origin(), default_site_url());
        assert_eq!(cfg("http://").site_url_origin(), default_site_url());
    }

    #[test]
    fn host_header_validation_accepts_only_authorities() {
        for ok in [
            "example.com",
            "example.com:8082",
            "192.168.1.5:8082",
            "localhost",
            "host_name.internal",
            "[::1]:8080",
            "[2001:db8::1]",
        ] {
            assert!(is_valid_host_header(ok), "{ok:?} should be accepted");
        }
        for bad in [
            "",
            " ",
            "example.com/path",
            "example.com?x=1",
            "example.com#frag",
            "user@example.com",
            "exam ple.com",
            "example.com:99999",
            "example.com:abc",
            "example.com:",
            "a\\b",
            "exa\"mple.com",
            "[::1",
            "host;x",
            "host,x",
            ".leading.dot",
            "-leading-dash",
        ] {
            assert!(!is_valid_host_header(bad), "{bad:?} should be rejected");
        }
    }

    /// The Host echo is a compatibility feature for direct LAN access; it must
    /// never echo anything but a plain authority, and it must be switchable off
    /// for proxied or wildcard-vhost deployments.
    #[test]
    fn trust_request_host_controls_the_fallback() {
        let mut c = cfg("http://127.0.0.1:8082");
        assert!(c.trust_request_host, "LAN compatibility is the default");
        assert_eq!(
            c.download_url_base(Some("192.168.1.100:8082")),
            "http://192.168.1.100:8082"
        );
        // A malformed or hostile value is not echoed.
        assert_eq!(
            c.download_url_base(Some("evil.example/path")),
            "http://127.0.0.1:8082"
        );
        assert_eq!(
            c.download_url_base(Some("user@evil.example")),
            "http://127.0.0.1:8082"
        );

        // Opting out disables the echo entirely.
        c.trust_request_host = false;
        assert_eq!(
            c.download_url_base(Some("192.168.1.100:8082")),
            "http://127.0.0.1:8082"
        );
    }

    /// A configuration the server could not run with must be refused by the
    /// admin page that produced it, not discovered after a restart.
    #[test]
    fn validate_accepts_defaults_and_rejects_unrunnable_values() {
        assert_eq!(Config::default().validate(), Ok(()));

        type Mutate = fn(&mut Config);
        let cases: [(&str, Mutate); 11] = [
            ("server.version", |c| c.server.version = "  ".to_string()),
            ("server.port", |c| c.server.port = 0),
            ("server.max_json_body_mb", |c| c.server.max_json_body_mb = 0),
            ("server.cors_allowed_origins", |c| {
                c.server.cors_allowed_origins = vec!["not-an-origin".to_string()]
            }),
            ("storage.block_encryption_mode", |c| {
                c.storage.block_encryption_mode = "enabled".to_string()
            }),
            ("storage.block_dir", |c| {
                c.storage.block_dir = PathBuf::new()
            }),
            ("auth.password_min_length", |c| {
                c.auth.password_min_length = 0
            }),
            ("auth.captcha_difficulty_bits", |c| {
                c.auth.captcha_difficulty_bits = CAPTCHA_MIN_BITS - 1
            }),
            ("auth.captcha_difficulty_bits", |c| {
                c.auth.captcha_difficulty_bits = CAPTCHA_MAX_BITS + 1
            }),
            ("email.timeout_secs", |c| c.email.timeout_secs = 0),
            ("logging.level", |c| c.logging.level = String::new()),
        ];
        for (expected_field, mutate) in cases {
            let mut config = Config::default();
            mutate(&mut config);
            let error = config.validate().expect_err("must be rejected");
            assert!(
                error.contains(expected_field),
                "error for {expected_field} was {error:?}"
            );
        }

        // The encrypted-library wire contract keeps its own validator.
        let mut config = Config::default();
        config.server.encrypted_library_version = 3;
        assert!(config.validate().is_err());
    }

    /// Case and padding are harmless, a typo is not: read as `off` it would start
    /// a deployment that meant to encrypt with plaintext blocks.
    #[test]
    fn block_encryption_mode_is_normalized_but_not_guessed() {
        use crate::storage::encrypting_block_store::BlockEncryptionMode;

        let mut config = Config::default();
        assert_eq!(
            config.storage.block_encryption_mode(),
            BlockEncryptionMode::Off
        );
        for (value, expected) in [
            ("on", BlockEncryptionMode::On),
            (" On ", BlockEncryptionMode::On),
            ("LAZY", BlockEncryptionMode::Lazy),
            ("off", BlockEncryptionMode::Off),
        ] {
            config.storage.block_encryption_mode = value.to_string();
            assert_eq!(
                config.storage.block_encryption_mode(),
                expected,
                "{value:?}"
            );
            assert!(
                config.storage.validate_block_encryption_mode().is_ok(),
                "{value:?} should be accepted"
            );
        }
        config.storage.block_encryption_mode = "true".to_string();
        assert!(
            config.storage.validate_block_encryption_mode().is_err(),
            "a value the parser would read as off must be rejected"
        );
    }

    #[test]
    fn captcha_mode_parses_case_insensitively_and_clamps_difficulty() {
        assert_eq!(CaptchaMode::parse(" Off "), Ok(CaptchaMode::Off));
        assert_eq!(CaptchaMode::parse("ADAPTIVE"), Ok(CaptchaMode::Adaptive));
        assert_eq!(CaptchaMode::parse("always"), Ok(CaptchaMode::Always));
        assert!(CaptchaMode::parse("sometimes").is_err());
        assert_eq!(CaptchaMode::default(), CaptchaMode::Off);

        // An old/edited file outside the accepted range is clamped rather than
        // making every challenge trivial or unusable.
        let mut auth = AuthConfig::default();
        assert_eq!(auth.captcha_bits(), 17);
        auth.captcha_difficulty_bits = 0;
        assert_eq!(auth.captcha_bits(), CAPTCHA_MIN_BITS);
        auth.captcha_difficulty_bits = 255;
        assert_eq!(auth.captcha_bits(), CAPTCHA_MAX_BITS);
    }

    #[test]
    fn normalize_repairs_schemeless_and_empty_site_url() {
        let mut c = Config::default();
        c.server.site_url = "  example.com:8082/  ".to_string();
        c.normalize();
        assert_eq!(c.server.site_url, "http://example.com:8082/");

        c.server.site_url = "   ".to_string();
        c.normalize();
        assert_eq!(c.server.site_url, default_site_url());

        // An explicit scheme is preserved verbatim (never silently upgraded to https).
        c.server.site_url = " https://files.example.com ".to_string();
        c.normalize();
        assert_eq!(c.server.site_url, "https://files.example.com");
    }

    #[test]
    fn configured_site_url_always_wins() {
        // Reverse-proxy deployment: admin configured the public address.
        let c = cfg("https://seafile.example.com");
        assert_eq!(c.download_url_base(None), "https://seafile.example.com");
        // Host header is ignored entirely — same as seahub's FILE_SERVER_ROOT.
        assert_eq!(
            c.download_url_base(Some("seafile.example.com")),
            "https://seafile.example.com"
        );
        assert_eq!(
            c.download_url_base(Some("192.168.1.100:8082")),
            "https://seafile.example.com"
        );
    }

    #[test]
    fn default_site_url_falls_back_to_host_with_port() {
        // Direct LAN access: Host carries a literal address, keep it verbatim.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(
            c.download_url_base(Some("192.168.1.100:8082")),
            "http://192.168.1.100:8082"
        );
        // A *name* is not echoed while `allowed_hosts` is unset: these URLs carry a
        // capability token, so an unchosen DNS name must not reach the client.
        assert_eq!(
            c.download_url_base(Some("seafile.example.com:8082")),
            "http://127.0.0.1:8082"
        );

        let mut allowed = cfg("http://127.0.0.1:8082");
        allowed.allowed_hosts = vec!["seafile.example.com:8082".to_string()];
        assert_eq!(
            allowed.download_url_base(Some("seafile.example.com:8082")),
            "http://seafile.example.com:8082"
        );
    }

    #[test]
    fn trust_request_host_never_echoes_a_name_by_default() {
        let c = cfg("http://127.0.0.1:8082");
        // Literal addresses (and localhost) keep the zero-config LAN case working.
        for host in [
            "192.168.1.100",
            "10.0.0.5:8082",
            "[fe80::1]",
            "[fe80::1]:8082",
            "localhost",
            "127.0.0.1:8082",
        ] {
            assert_eq!(
                c.download_url_base(Some(host)),
                format!("http://{host}"),
                "host {host}"
            );
        }
        // Names do not.
        for host in ["evil.example", "evil.example:8082", "rebind.attacker.tld"] {
            assert_eq!(
                c.download_url_base(Some(host)),
                "http://127.0.0.1:8082",
                "host {host} must not be echoed into a token-bearing URL"
            );
        }
    }

    #[test]
    fn default_site_url_falls_back_to_host_without_port() {
        // Reverse proxy without a port in Host (80/443): never append the internal
        // listen port. A literal address is echoed; a name needs `allowed_hosts`.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(
            c.download_url_base(Some("192.168.1.100")),
            "http://192.168.1.100"
        );
        assert_eq!(
            c.download_url_base(Some("seafile.example.com")),
            "http://127.0.0.1:8082"
        );
    }

    #[test]
    fn default_site_url_ipv6_host() {
        // IPv6 literals (with or without port) pass through verbatim.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(
            c.download_url_base(Some("[2001:db8::1]:8082")),
            "http://[2001:db8::1]:8082"
        );
        assert_eq!(
            c.download_url_base(Some("[2001:db8::1]")),
            "http://[2001:db8::1]"
        );
    }

    /// Secrets must never appear in `Debug` output: `{:?}` on a config is the obvious
    /// thing to do when diagnosing a startup problem, and it is cloned into `AppState`.
    #[test]
    fn debug_output_does_not_leak_secrets() {
        let mut config = Config::default();
        config.server.secret_key = "super-secret-master-key-value-0123456789".to_string();
        config.notification.private_key = "notification-key-value-0123456789".to_string();
        config.storage.encryption_key = Some("storage-key-value-0123456789".to_string());
        config.database.url = "sqlite://user:hunter2@localhost/nanofile.db".to_string();
        config.admin_init.password = Some("admin-password-value".to_string());

        let rendered = format!("{config:?}");
        for secret in [
            "super-secret-master-key-value-0123456789",
            "notification-key-value-0123456789",
            "storage-key-value-0123456789",
            "hunter2",
            "admin-password-value",
        ] {
            assert!(
                !rendered.contains(secret),
                "Debug leaked a secret: {rendered}"
            );
        }
        // The sub-structs are redacted on their own too.
        assert!(!format!("{:?}", config.server).contains("super-secret"));
        assert!(!format!("{:?}", config.storage).contains("storage-key"));
        assert!(!format!("{:?}", config.database).contains("hunter2"));
        assert!(
            !format!("{:?}", config.notification).contains("notification-key"),
            "notification key leaked"
        );
    }

    #[test]
    fn default_site_url_without_host_header() {
        // No Host header at all: fall back to site_url itself.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(c.download_url_base(None), "http://127.0.0.1:8082");
    }

    #[test]
    fn load_complete_config_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let complete = toml::to_string_pretty(&Config::default()).unwrap();
        std::fs::write(&path, &complete).unwrap();

        let config = Config::load_from(&path).expect("load complete config");
        assert_eq!(config.server.port, 8082);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), complete);
        assert!(!std::path::Path::new(&format!("{}.bak", path.display())).exists());
    }

    /// A partial file is a complete configuration: missing keys keep their built-in
    /// defaults in memory, and the file is left exactly as written.
    #[test]
    fn load_incomplete_config_takes_defaults_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = "[server]\naddr = \"1.2.3.4\"\n";
        std::fs::write(&path, partial).unwrap();

        let config = Config::load_from(&path).expect("load incomplete config");
        assert_eq!(config.server.addr, "1.2.3.4");
        assert_eq!(config.server.port, 8082, "a missing key takes its default");
        assert_eq!(config.database.url, default_db_url());

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            partial,
            "the loader must not rewrite what the operator wrote"
        );
        assert!(!std::path::Path::new(&format!("{}.bak", path.display())).exists());
    }

    #[cfg(unix)]
    mod persist {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn writes_backup_and_replaces() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "old").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

            persist_with_backup(&path, "new").unwrap();

            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
            assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), "old");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "permissions must survive the atomic write");
            let backup_mode = std::fs::metadata(backup_path(&path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                backup_mode, 0o600,
                "the backup is a second copy of the same secrets and must be private too"
            );
        }

        /// The config file holds the master `secret_key`: a permissive mode must be
        /// tightened by the config write rather than inherited.
        #[test]
        fn permissive_config_is_tightened() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "old").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

            persist_with_backup(&path, "new").unwrap();

            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "a world-readable config must be tightened");
            let backup_mode = std::fs::metadata(backup_path(&path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(backup_mode, 0o600, "the backup must be tightened as well");
        }

        /// The replacement file is created owner-only, not tightened afterwards, so the master
        /// secret is never briefly readable — and a symlink planted at the predictable temporary
        /// name is not followed.
        #[test]
        fn the_replacement_file_is_owner_only_from_creation() {
            let dir = tempfile::tempdir().unwrap();
            let tmp = dir.path().join("config.toml.123.tmp");

            let file = open_owner_only_new(&tmp).expect("a fresh temp file must be created");
            drop(file);
            let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the temp file holds the master secret");

            // A leftover file is replaced, not appended to.
            std::fs::write(&tmp, "stale").unwrap();
            let mut file = open_owner_only_new(&tmp).unwrap();
            use std::io::Write as _;
            file.write_all(b"fresh").unwrap();
            drop(file);
            assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "fresh");

            // A symlink at the temp name is removed rather than followed.
            std::fs::remove_file(&tmp).unwrap();
            let target = dir.path().join("target");
            std::fs::write(&target, "secret").unwrap();
            std::os::unix::fs::symlink(&target, &tmp).unwrap();
            let file = open_owner_only_new(&tmp).expect("the planted symlink must be replaced");
            drop(file);
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                "secret",
                "the symlink target must not be written through"
            );
            assert!(
                !std::fs::symlink_metadata(&tmp)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "the temp path must be a regular file afterwards"
            );
        }

        #[test]
        fn fails_on_readonly_dir() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "old").unwrap();
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

            // Root bypasses directory permissions — skip the assertion there.
            let is_root = libc_geteuid() == 0;
            let result = persist_with_backup(&path, "new");
            if is_root {
                let _ = result;
            } else {
                assert!(result.is_err(), "write into read-only dir must fail");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
                assert!(!tmp_path(&path).exists());
            }
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[cfg(unix)]
    fn libc_geteuid() -> u32 {
        // Minimal euid check without pulling in a libc dev-dependency.
        let s = std::process::Command::new("id").arg("-u").output().unwrap();
        String::from_utf8_lossy(&s.stdout)
            .trim()
            .parse()
            .unwrap_or(1000)
    }

    #[test]
    fn load_missing_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        let config = Config::load_from(&path).expect("missing file falls back to defaults");
        assert_eq!(config.server.port, 8082);
        assert_eq!(config.server.addr, "0.0.0.0");
        assert_eq!(config.database.url, "sqlite:data/nanofile.db?mode=rwc");
        assert_eq!(config.storage.block_dir, PathBuf::from("data/blocks"));
        assert_eq!(config.auth.password_hash_iterations, 600000);
        // Loading leaves relative paths relative: resolving them is an explicit startup
        // step, so a test or tool never silently points somewhere else.
        assert_eq!(config.index.index_dir, PathBuf::from("data/index"));
    }

    #[test]
    fn sqlite_url_file_splits_urls_the_way_sqlx_does() {
        assert_eq!(
            sqlite_url_file("sqlite:data/x.db?mode=rwc"),
            Some(("data/x.db", Some("mode=rwc")))
        );
        // The URL form keeps a relative file name relative...
        assert_eq!(
            sqlite_url_file("sqlite://data/x.db?mode=rwc"),
            Some(("data/x.db", Some("mode=rwc")))
        );
        // ...and a rooted one rooted.
        assert_eq!(
            sqlite_url_file("sqlite:/var/lib/x.db"),
            Some(("/var/lib/x.db", None))
        );
        assert_eq!(
            sqlite_url_file("sqlite:///var/lib/x.db"),
            Some(("/var/lib/x.db", None))
        );
        // No file on disk: in-memory forms and every other backend.
        assert_eq!(sqlite_url_file("sqlite::memory:"), None);
        assert_eq!(sqlite_url_file("sqlite://?mode=memory"), None);
        assert_eq!(sqlite_url_file("sqlite:file::memory:?cache=shared"), None);
        assert_eq!(sqlite_url_file("postgres://localhost/nanofile"), None);
        assert_eq!(sqlite_url_file(""), None);
    }

    /// Regression: a desktop instance started at login runs with a working directory
    /// that has nothing to do with the installation (`C:\Windows\System32`), where the
    /// relative defaults (`sqlite:data/nanofile.db?mode=rwc`, `data/blocks`, …) name
    /// nothing.
    #[test]
    fn resolve_state_paths_rewrites_relative_paths_under_the_base() {
        let base = PathBuf::from("/opt/nanofile");
        let cwd = tempfile::tempdir().unwrap();
        let mut config = Config::default();

        let resolved = config.resolve_state_paths_in(&base, cwd.path());

        assert_eq!(
            config.database.url,
            format!(
                "sqlite:{}?mode=rwc",
                base.join("data/nanofile.db").display()
            ),
            "the query string has to survive the rewrite"
        );
        assert_eq!(config.storage.block_dir, base.join("data/blocks"));
        assert_eq!(config.storage.temp_dir, base.join("data/temp"));
        assert_eq!(config.storage.thumbnail_dir, base.join("data/thumbnails"));
        assert_eq!(config.storage.avatar_dir, base.join("data/avatars"));
        assert_eq!(config.index.index_dir, base.join("data/index"));

        assert_eq!(resolved.len(), 6, "every relative field is reported");
        assert!(resolved.iter().all(|r| r.rewritten));
        assert!(resolved.iter().any(|r| r.field == "database.url"));
    }

    #[test]
    fn state_dirs_names_the_database_directory_and_every_cache() {
        let base = PathBuf::from("/opt/nanofile");
        let cwd = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.resolve_state_paths_in(&base, cwd.path());

        let dirs: Vec<_> = config.state_dirs();
        let fields: Vec<_> = dirs.iter().map(|(field, _)| *field).collect();
        assert_eq!(
            fields,
            vec![
                "database.url",
                "storage.block_dir",
                "storage.temp_dir",
                "storage.thumbnail_dir",
                "storage.avatar_dir",
                "index.index_dir",
            ]
        );
        assert_eq!(dirs[0].1, base.join("data"));
        assert_eq!(dirs[1].1, base.join("data/blocks"));
    }

    #[test]
    fn state_dirs_has_no_directory_for_an_in_memory_database() {
        let mut config = Config::default();
        config.database.url = "sqlite::memory:".to_string();
        let fields: Vec<_> = config.state_dirs().iter().map(|(f, _)| *f).collect();
        assert!(!fields.contains(&"database.url"));
        assert!(fields.contains(&"storage.block_dir"));
    }

    /// The `sqlite://` URL spelling is relative too, so it is rewritten as well.
    #[test]
    fn resolve_state_paths_rewrites_the_sqlite_url_form() {
        let base = PathBuf::from("/opt/nanofile");
        let cwd = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.database.url = "sqlite://data/nanofile.db?mode=rwc".to_string();

        config.resolve_state_paths_in(&base, cwd.path());

        assert_eq!(
            config.database.url,
            format!(
                "sqlite:{}?mode=rwc",
                base.join("data/nanofile.db").display()
            )
        );
    }

    /// An installation whose state already lives next to its working directory keeps
    /// every relative path: upgrading must not move a running deployment.
    #[test]
    fn resolve_state_paths_keeps_state_that_is_already_in_the_working_directory() {
        let base = PathBuf::from("/opt/nanofile");
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join("data")).unwrap();
        std::fs::write(cwd.path().join("data/nanofile.db"), b"").unwrap();
        let mut config = Config::default();

        let resolved = config.resolve_state_paths_in(&base, cwd.path());

        assert_eq!(config.database.url, "sqlite:data/nanofile.db?mode=rwc");
        // Not even the paths that do not exist yet move.
        assert_eq!(config.storage.block_dir, PathBuf::from("data/blocks"));
        assert_eq!(config.index.index_dir, PathBuf::from("data/index"));
        assert_eq!(resolved.len(), 6);
        assert!(resolved.iter().all(|r| !r.rewritten));
    }

    #[test]
    fn resolve_state_paths_leaves_absolute_and_non_file_paths_alone() {
        let base = PathBuf::from("/opt/nanofile");
        let cwd = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.database.url = "postgres://localhost/nanofile".to_string();
        // An absolute path for *this* platform: `/srv/blocks` is relative on Windows,
        // where absolute paths carry a drive letter.
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\srv\blocks")
        } else {
            PathBuf::from("/srv/blocks")
        };
        config.storage.block_dir = absolute.clone();
        config.logging.file = Some(PathBuf::from("logs/nanofile.log"));

        let resolved = config.resolve_state_paths_in(&base, cwd.path());

        assert_eq!(config.database.url, "postgres://localhost/nanofile");
        assert_eq!(config.storage.block_dir, absolute);
        // `[logging] file` resolves inside `logging::init`, which writes it back.
        assert_eq!(
            config.logging.file,
            Some(PathBuf::from("logs/nanofile.log"))
        );
        // Only the relative directories are reported (temp, thumbnails, avatars, index).
        assert!(resolved.iter().all(|r| r.field != "database.url"));
        assert_eq!(resolved.len(), 4);
    }

    #[test]
    fn state_path_base_is_an_existing_absolute_directory() {
        let base = state_path_base();
        assert!(base.is_absolute(), "{base:?}");
        assert!(base.is_dir(), "{base:?}");
    }

    /// The reference an auto-start entry or service registration records has to be an
    /// error when it is gone: defaults here would start a second instance.
    #[test]
    fn a_config_named_explicitly_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("moved-away.toml");
        let error = match Config::load_for_start(Some(&missing)) {
            Ok(_) => panic!("a named config that is gone must not fall back to defaults"),
            Err(e) => e.to_string(),
        };
        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("moved-away.toml"), "{error}");
        assert!(error.contains("--config"), "{error}");

        // An existing one still loads; a key it omits keeps its default.
        let present = dir.path().join("config.toml");
        std::fs::write(&present, "[server]\nport = 1234\n").unwrap();
        let loaded = Config::load_for_start(Some(&present)).unwrap();
        assert_eq!(loaded.config.server.port, 1234);
        assert!(!loaded.config.server.site_url.is_empty(), "defaults filled");
    }

    #[test]
    fn logging_defaults_are_auto_and_capped() {
        let c = LoggingConfig::default();
        assert_eq!(c.level, "info");
        assert_eq!(c.file_enabled, None);
        assert_eq!(c.file, None);
        assert_eq!(c.max_file_size_mb, 10);
        assert_eq!(c.max_backups, 3);
    }

    /// The two byte caps that bound what one client can make the server buffer are
    /// finite by default (deliberately generous), rather than "unlimited".
    #[test]
    fn storage_byte_caps_default_to_finite_limits() {
        let c = StorageConfig::default();
        assert_eq!(c.max_temp_upload_bytes, 32 * 1024 * 1024 * 1024);
        assert_eq!(c.max_zip_bytes, 8 * 1024 * 1024 * 1024);

        // The serde default and the struct default must agree.
        let parsed: StorageConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.max_temp_upload_bytes, c.max_temp_upload_bytes);
        assert_eq!(parsed.max_zip_bytes, c.max_zip_bytes);
    }

    #[test]
    fn logging_section_takes_defaults_for_keys_it_omits() {
        // An old config with only `[logging] level` keeps it and defaults the rest.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[logging]\nlevel = \"debug\"\n").unwrap();

        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.file_enabled, None);
        assert_eq!(config.logging.file, None);
        assert_eq!(config.logging.max_file_size_mb, 10);
        assert_eq!(config.logging.max_backups, 3);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[logging]\nlevel = \"debug\"\n"
        );
    }

    #[test]
    fn write_config_value_persists_and_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# header comment\n[logging]\n# keep me\nlevel = \"info\"\n",
        )
        .unwrap();

        write_config_value(&path, "logging", "file", "/opt/nanofile/nanofile.log").unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# header comment"));
        assert!(out.contains("# keep me"));
        assert!(out.contains("level = \"info\""));
        assert!(out.contains("file = \"/opt/nanofile/nanofile.log\""));

        // A missing section is created.
        write_config_value(&path, "ui", "tray_language", "zh").unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[ui]"));
        assert!(out.contains("tray_language = \"zh\""));
        // The [logging] values survive the second write.
        assert!(out.contains("file = \"/opt/nanofile/nanofile.log\""));

        // A .bak of the pre-write state is kept, like every config write.
        assert!(dir.path().join("config.toml.bak").exists());
    }

    #[test]
    fn email_defaults_are_off_with_an_undeliverable_shape() {
        let c = EmailConfig::default();
        assert!(!c.enabled, "mail is opt-in");
        assert!(c.host.is_empty());
        assert!(c.from_address.is_empty());
        assert!(c.password.is_none());
        assert_eq!(c.port, 587);
        assert_eq!(c.tls, "starttls");
        assert_eq!(c.timeout_secs, 10);
        assert_eq!(c.max_attempts, 5);
    }

    #[test]
    fn email_section_roundtrips_and_leaves_the_file_alone() {
        // A config with only the switch reads every other email key from its default, and
        // the file keeps exactly what the operator wrote: no `password` line is invented.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[email]\nenabled = true\n").unwrap();

        let config = Config::load_from(&path).unwrap();
        assert!(config.email.enabled);
        assert_eq!(config.email.port, 587);
        assert_eq!(config.email.tls, "starttls");
        assert_eq!(config.email.max_attempts, 5);
        assert_eq!(config.email.timeout_secs, 10);
        assert!(config.email.password.is_none());

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[email]\nenabled = true\n"
        );
    }

    #[test]
    fn email_config_never_debugs_its_password() {
        // `EmailConfig` carries the SMTP password; its `Debug` is redacted.
        let c = EmailConfig {
            password: Some("smtp-secret".to_string()),
            ..Default::default()
        };
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("smtp-secret"));
    }

    fn os(name: &str) -> std::ffi::OsString {
        std::ffi::OsString::from(name)
    }

    #[test]
    fn a_named_config_path_always_wins() {
        let named = PathBuf::from("/tmp/named.toml");
        let resolved = resolve_config_path_from(
            Some(&named),
            Some(PathBuf::from("/tmp/env.toml")),
            Some(PathBuf::from("config.toml")),
            Some(PathBuf::from("/home/u/.nanofile/config.toml")),
            PathBuf::from("/home/u/.nanofile/config.toml"),
        );
        assert_eq!(resolved.path, named);
        assert_eq!(resolved.origin, ConfigPathOrigin::CommandLine);
        assert!(resolved.is_explicit());
        assert!(!resolved.is_unconfigured());
        assert_eq!(resolved.origin.named_by(), Some("--config"));
    }

    #[test]
    fn the_environment_names_a_config_before_any_default_does() {
        let resolved = resolve_config_path_from(
            None,
            Some(PathBuf::from("/etc/nanofile/config.toml")),
            Some(PathBuf::from("config.toml")),
            None,
            PathBuf::from("/home/u/.nanofile/config.toml"),
        );
        assert_eq!(resolved.path, PathBuf::from("/etc/nanofile/config.toml"));
        assert_eq!(resolved.origin, ConfigPathOrigin::Environment);
        assert_eq!(resolved.origin.named_by(), Some(CONFIG_PATH_ENV));
    }

    #[test]
    fn the_working_directory_default_precedes_the_per_user_file() {
        // Both exist: the historical default wins, so an upgraded deployment keeps it.
        let resolved = resolve_config_path_from(
            None,
            None,
            Some(PathBuf::from("config.toml")),
            Some(PathBuf::from("/home/u/.nanofile/config.toml")),
            PathBuf::from("/home/u/.nanofile/config.toml"),
        );
        assert_eq!(resolved.path, PathBuf::from("config.toml"));
        assert_eq!(resolved.origin, ConfigPathOrigin::LegacyDefault);
        assert!(!resolved.is_explicit());

        // Only the per-user file exists: that is this build's default.
        let resolved = resolve_config_path_from(
            None,
            None,
            None,
            Some(PathBuf::from("/home/u/.nanofile/config.toml")),
            PathBuf::from("/home/u/.nanofile/config.toml"),
        );
        assert_eq!(
            resolved.path,
            PathBuf::from("/home/u/.nanofile/config.toml")
        );
        assert_eq!(resolved.origin, ConfigPathOrigin::UserDefault);
    }

    #[test]
    fn nothing_at_all_leaves_the_write_default_unconfigured() {
        let resolved = resolve_config_path_from(
            None,
            None,
            None,
            None,
            PathBuf::from("/home/u/.nanofile/config.toml"),
        );
        assert_eq!(
            resolved.path,
            PathBuf::from("/home/u/.nanofile/config.toml")
        );
        assert!(resolved.is_unconfigured());
        assert!(!resolved.is_explicit());
    }

    #[test]
    fn only_configuration_variables_count_as_environment() {
        // Anything that can carry a setting suppresses the first run...
        assert!(names_supply_config([
            os("PATH"),
            os("NANOFILE_LOG_LEVEL"),
            os("NANOFILE_CONFIG"),
        ]));
        assert!(names_supply_config([os("NANOFILE_SERVER_SECRET_KEY")]));
        // ...and the names that decide something else do not.
        assert!(!names_supply_config(
            std::iter::empty::<std::ffi::OsString>()
        ));
        assert!(!names_supply_config([os("PATH"), os("HOME")]));
        assert!(!names_supply_config([
            os("NANOFILE_DOTENV"),
            os("NANOFILE_TEST_FFMPEG_PATH"),
            os("NANOFILE_BUILD_TS"),
        ]));
    }

    #[test]
    fn a_first_run_is_the_unconfigured_case_with_a_clean_environment() {
        let unconfigured = ConfigPath {
            path: PathBuf::from("/home/u/.nanofile/config.toml"),
            origin: ConfigPathOrigin::Unconfigured,
        };
        let legacy = ConfigPath {
            path: PathBuf::from("config.toml"),
            origin: ConfigPathOrigin::LegacyDefault,
        };
        // The gate and the environment half are covered elsewhere; the process
        // environment is deliberately not mutated (the suite runs tests in parallel).
        assert!(unconfigured.is_unconfigured());
        assert!(!legacy.is_unconfigured());
        assert!(!needs_first_run_setup(&legacy) || !environment_supplies_config());
    }

    #[test]
    fn write_new_config_creates_a_private_file_that_parses() {
        let dir = tempfile::tempdir().unwrap();
        // The per-user directory does not exist yet: it is created owner-only.
        let config_dir = dir.path().join(".nanofile");
        let path = config_dir.join(CONFIG_FILE_NAME);
        write_new_config(
            &path,
            "Nanofile configuration.\n\nWritten by the first-run setup.",
            &[
                ConfigSection {
                    name: "server",
                    comment: "Listener and master secret.",
                    entries: vec![
                        ("addr", "0.0.0.0".into()),
                        ("port", 8123.into()),
                        ("secret_key", "abc".into()),
                    ],
                },
                ConfigSection {
                    name: "database",
                    comment: "",
                    entries: vec![("url", "sqlite:/data/nanofile.db?mode=rwc".into())],
                },
            ],
        )
        .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Nanofile configuration."));
        assert!(text.contains("# Listener and master secret.\n[server]\n"));
        assert!(text.contains("port = 8123"), "{text}");

        let parsed: Config = toml::from_str(&text).expect("the document parses");
        assert_eq!(parsed.server.port, 8123);
        assert_eq!(parsed.server.secret_key, "abc");
        assert_eq!(parsed.database.url, "sqlite:/data/nanofile.db?mode=rwc");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the config holds the master secret");
            let dir_mode = std::fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
    }

    #[test]
    fn write_new_config_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE_NAME);
        std::fs::write(&path, "[server]\nport = 1\n").unwrap();

        let error = write_new_config(&path, "header", &[]).unwrap_err();
        assert!(
            matches!(error, WriteConfigError::AlreadyExists(ref p) if *p == path),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[server]\nport = 1\n",
            "the existing file must be untouched"
        );
    }

    #[test]
    fn write_new_config_carries_the_first_run_values_into_a_config() {
        // The document the setup writes has to be a configuration the loader accepts
        // with the values it was built from.
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let path = dir.path().join(CONFIG_FILE_NAME);
        let db_url = format!("sqlite:{}/nanofile.db?mode=rwc", data_dir.display());
        write_new_config(
            &path,
            "header",
            &[
                ConfigSection {
                    name: "server",
                    comment: "",
                    entries: vec![("port", 8123.into())],
                },
                ConfigSection {
                    name: "database",
                    comment: "",
                    entries: vec![("url", db_url.as_str().into())],
                },
            ],
        )
        .unwrap();

        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.server.port, 8123);
        assert_eq!(config.database.url, db_url);
        assert_eq!(
            sqlite_url_file(&config.database.url).map(|(file, _)| PathBuf::from(file)),
            Some(data_dir.join("nanofile.db"))
        );
    }

    /// The old key still closes registration; a file written for an older
    /// release must not reopen it by falling back to the default.
    #[test]
    fn the_retired_registration_key_is_still_accepted() {
        let old: Config = toml::from_str("[auth]\nenable_invitations = false\n").unwrap();
        assert!(!old.auth.enable_registration);

        let new: Config = toml::from_str("[auth]\nenable_registration = false\n").unwrap();
        assert!(!new.auth.enable_registration);
        assert!(Config::default().auth.enable_registration);
        assert!(Config::default().auth.require_invitation);
    }

    /// The retired variable still closes registration, and reports itself as the
    /// origin rather than claiming the new name was set.
    #[test]
    fn the_retired_registration_variable_is_still_read() {
        let mut config = Config::default();
        let mut env_keys = EnvKeys::new();
        let lookup =
            |name: &str| (name == "NANOFILE_AUTH_ENABLE_INVITATIONS").then(|| "false".to_string());

        config.apply_retired_env_aliases(&mut env_keys, lookup);

        assert!(!config.auth.enable_registration);
        assert_eq!(
            env_keys.get("auth.enable_registration"),
            Some(&"NANOFILE_AUTH_ENABLE_INVITATIONS")
        );
    }

    /// The new variable wins when both names are present.
    #[test]
    fn the_new_registration_variable_beats_the_retired_one() {
        let mut config = Config::default();
        let mut env_keys = EnvKeys::new();
        env_keys.insert(
            "auth.enable_registration",
            "NANOFILE_AUTH_ENABLE_REGISTRATION",
        );
        let lookup =
            |name: &str| (name == "NANOFILE_AUTH_ENABLE_INVITATIONS").then(|| "false".to_string());

        config.apply_retired_env_aliases(&mut env_keys, lookup);

        assert!(config.auth.enable_registration, "the new variable must win");
        assert_eq!(
            env_keys.get("auth.enable_registration"),
            Some(&"NANOFILE_AUTH_ENABLE_REGISTRATION")
        );
    }

    /// The retired sharing master switch in a config file closes **both**
    /// directions: mapping it to the download half alone would silently reopen
    /// anonymous uploads on an installation that had turned sharing off.
    #[test]
    fn the_retired_sharing_key_sets_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\nshare_link_enabled = false\n").unwrap();

        let config = Config::load_from(&path).expect("load the retired key");

        assert!(!config.server.share_download_enabled);
        assert!(!config.server.share_upload_enabled);
        assert!(Config::default().server.share_download_enabled);
        assert!(Config::default().server.share_upload_enabled);
    }

    /// The split keys are independent; the retired name is the only thing that
    /// still means "both".
    #[test]
    fn the_split_sharing_keys_are_independent() {
        let config: Config = toml::from_str("[server]\nshare_download_enabled = false\n").unwrap();
        assert!(!config.server.share_download_enabled);
        assert!(config.server.share_upload_enabled);

        let config: Config = toml::from_str("[server]\nshare_upload_enabled = false\n").unwrap();
        assert!(config.server.share_download_enabled);
        assert!(!config.server.share_upload_enabled);
    }

    /// The retired variable still closes both halves and reports itself as the
    /// origin rather than claiming the new names were set.
    #[test]
    fn the_retired_sharing_variable_closes_both() {
        let mut config = Config::default();
        let mut env_keys = EnvKeys::new();
        let lookup = |name: &str| {
            (name == "NANOFILE_SERVER_SHARE_LINK_ENABLED").then(|| "false".to_string())
        };

        config.apply_retired_env_aliases(&mut env_keys, lookup);

        assert!(!config.server.share_download_enabled);
        assert!(!config.server.share_upload_enabled);
        for key in [
            "server.share_download_enabled",
            "server.share_upload_enabled",
        ] {
            assert_eq!(
                env_keys.get(key),
                Some(&"NANOFILE_SERVER_SHARE_LINK_ENABLED"),
                "{key} must report the variable that supplied it"
            );
        }
    }

    /// A current variable is authoritative for its own direction; the retired
    /// master still covers the direction nobody configured.
    #[test]
    fn the_new_sharing_variable_wins_over_the_retired_one() {
        let mut config = Config::default();
        let mut env_keys = EnvKeys::new();
        env_keys.insert(
            "server.share_download_enabled",
            "NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED",
        );
        let lookup = |name: &str| {
            (name == "NANOFILE_SERVER_SHARE_LINK_ENABLED").then(|| "false".to_string())
        };

        config.apply_retired_env_aliases(&mut env_keys, lookup);

        assert!(
            config.server.share_download_enabled,
            "the direction with its own variable keeps it"
        );
        assert!(
            !config.server.share_upload_enabled,
            "the retired master still covers the other direction"
        );
        assert_eq!(
            env_keys.get("server.share_download_enabled"),
            Some(&"NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED")
        );
        assert_eq!(
            env_keys.get("server.share_upload_enabled"),
            Some(&"NANOFILE_SERVER_SHARE_LINK_ENABLED")
        );
    }

    /// A config file written before the rename still sets the default quota: the
    /// key moved because it was never a cap on every account together, not because
    /// the value changed meaning.
    #[test]
    fn the_renamed_quota_key_in_the_file_still_applies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[storage]\nmax_storage_bytes = 128\n").unwrap();

        let config = Config::load_from(&path).expect("load the retired key");

        assert_eq!(config.storage.default_user_quota_bytes, 128);
        assert_eq!(Config::default().storage.default_user_quota_bytes, 0);
    }

    /// A file that carries both names keeps the new one; serde would fail on a
    /// duplicate field, so the fold has to decide.
    #[test]
    fn the_new_quota_key_wins_over_the_retired_file_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[storage]\nmax_storage_bytes = 128\ndefault_user_quota_bytes = 256\n",
        )
        .unwrap();

        let config = Config::load_from(&path).expect("load both keys");

        assert_eq!(config.storage.default_user_quota_bytes, 256);
    }

    /// The retired variable feeds the renamed setting and reports itself as the
    /// origin, so the settings page can say where the value came from.
    #[test]
    fn the_retired_quota_variable_renames() {
        let mut config = Config::default();
        let mut env_keys = EnvKeys::new();
        let lookup =
            |name: &str| (name == "NANOFILE_STORAGE_MAX_STORAGE_BYTES").then(|| "128".to_string());

        config.apply_retired_env_aliases(&mut env_keys, lookup);

        assert_eq!(config.storage.default_user_quota_bytes, 128);
        assert_eq!(
            env_keys.get("storage.default_user_quota_bytes"),
            Some(&"NANOFILE_STORAGE_MAX_STORAGE_BYTES")
        );
    }

    /// The current variable is authoritative; the retired name is skipped rather
    /// than racing it.
    #[test]
    fn the_new_quota_variable_wins_over_the_retired_one() {
        let mut config = Config::default();
        let mut env_keys = EnvKeys::new();
        env_keys.insert(
            "storage.default_user_quota_bytes",
            "NANOFILE_STORAGE_DEFAULT_USER_QUOTA_BYTES",
        );
        let lookup =
            |name: &str| (name == "NANOFILE_STORAGE_MAX_STORAGE_BYTES").then(|| "128".to_string());

        config.apply_retired_env_aliases(&mut env_keys, lookup);

        assert_eq!(config.storage.default_user_quota_bytes, 0);
        assert_eq!(
            env_keys.get("storage.default_user_quota_bytes"),
            Some(&"NANOFILE_STORAGE_DEFAULT_USER_QUOTA_BYTES")
        );
    }

    /// One chunk is buffered while it is validated and written, so the default
    /// must stay small enough that a handful of simultaneous uploads cannot
    /// exhaust a modest server. The official clients send 4 MiB chunks.
    #[test]
    fn resumable_upload_chunk_default_is_bounded() {
        assert_eq!(Config::default().server.max_chunk_size_mb, 32);
    }
}

// `Config` derives `Debug` so a whole configuration can be logged, but several
// fields are secrets (`server.secret_key`, `notification.private_key`,
// `storage.encryption_key`, `database.url`, `admin_init.password`). These impls
// print no fields at all, so `{:?}` cannot leak a secret; use the accessors.
macro_rules! redacting_debug {
    ($($ty:ident),+ $(,)?) => {
        $(
            impl std::fmt::Debug for $ty {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.debug_struct(stringify!($ty)).finish_non_exhaustive()
                }
            }
        )+
    };
}

redacting_debug!(
    Config,
    NotificationConfig,
    ServerConfig,
    DatabaseConfig,
    StorageConfig,
    AdminInitConfig,
    // Holds the SMTP password, so it joins the redacted set for the same reason.
    EmailConfig,
);
