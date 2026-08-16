use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Environment variable overriding the config file path.
pub const CONFIG_PATH_ENV: &str = "NANOFILE_CONFIG";
/// Default config path when neither `--config` nor `NANOFILE_CONFIG` is set.
pub const DEFAULT_CONFIG_PATH: &str = "config.toml";

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
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
    pub notification: NotificationConfig,
    #[serde(default)]
    pub admin_init: AdminInitConfig,
    #[serde(default)]
    pub email: EmailConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

/// Web UI localization settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiConfig {
    /// Fallback UI language when the user hasn't set a preference and the
    /// browser's Accept-Language doesn't match a supported language.
    #[serde(default = "default_ui_language")]
    pub default_language: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            default_language: default_ui_language(),
        }
    }
}

fn default_ui_language() -> String {
    "en".to_string()
}

/// Email delivery configuration.
///
/// Password reset links are **only** delivered to the account owner's inbox.
/// Without a configured email backend the reset feature is disabled: the server
/// must never echo the reset link back in the HTTP response, since that would
/// let anyone take over any account.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct EmailConfig {
    /// Master switch. When `false` (default) the password-reset flow is
    /// disabled and requests render a generic page without minting a token.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub private_key: String,
    /// Seconds between WebSocket Ping frames (0 = disable keepalive).
    #[serde(default = "default_ping_interval")]
    pub ping_interval: u64,
    /// Seconds without a Pong after which the connection is dropped.
    #[serde(default = "default_client_timeout")]
    pub client_timeout: u64,
}

fn default_ping_interval() -> u64 {
    30
}
fn default_client_timeout() -> u64 {
    90
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            private_key: String::new(),
            ping_interval: default_ping_interval(),
            client_timeout: default_client_timeout(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Seafile-compatible server version advertised to clients via
    /// `/api2/server-info/`. Clients parse the major/minor/patch numbers to
    /// gate feature availability, so keep it at a version whose capabilities
    /// this server actually implements.
    /// Env: NANOFILE_SERVER_VERSION
    #[serde(default = "default_server_version")]
    pub version: String,
    /// External URL for this server, e.g. "http://127.0.0.1:8082".
    /// Used to construct download/upload/share URLs and as the default CORS origin.
    /// If empty at startup, derived from addr:port as http://{addr}:{port}.
    #[serde(default = "default_site_url")]
    pub site_url: String,
    #[serde(default = "default_max_upload_size_mb")]
    pub max_upload_size_mb: u64,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Allowed CORS origins. When empty, defaults to the origin of `site_url`.
    /// Set to a comma-separated list for multiple origins (e.g. for API clients).
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// Server-wide secret key for cryptographic operations (CSRF tokens,
    /// notification JWTs, etc.). Must be a hex-encoded string; recommend 64
    /// hex characters from `openssl rand -hex 32`. When empty, auto-generated
    /// on startup with a warning (sessions won't survive a restart).
    /// Env: NANOFILE_SERVER_SECRET_KEY
    #[serde(default)]
    pub secret_key: String,
    /// CORS max-age in seconds (default 86400 = 24h).
    #[serde(default = "default_cors_max_age")]
    pub cors_max_age_secs: u64,
    /// Whether the WebDAV endpoint (`/dav/...`) is enabled.
    /// When false, all WebDAV requests return 403.
    #[serde(default = "default_true")]
    pub webdav_enabled: bool,
    /// Whether the SSO local-browser login flow is enabled.
    /// When false, the `/client-sso/{token}/` browser pages return an error and
    /// `client-sso-via-local-browser` is not advertised in `/api2/server-info/`.
    /// Env: NANOFILE_SERVER_SSO_ENABLED
    #[serde(default = "default_true")]
    pub sso_enabled: bool,
    /// Custom brand string shown by the desktop client in its title bar
    /// (`desktop-custom-brand` in `/api2/server-info/`). When set, the value is
    /// advertised verbatim; `None` keeps the key absent.
    /// Env: NANOFILE_SERVER_DESKTOP_CUSTOM_BRAND
    #[serde(default)]
    pub desktop_custom_brand: Option<String>,
    /// Path (relative to the server root) of a custom logo the desktop client
    /// fetches and shows (`desktop-custom-logo` in `/api2/server-info/`). The
    /// client joins this onto the server URL itself, so a full URL here would
    /// be mangled. `None` keeps the key absent.
    /// Env: NANOFILE_SERVER_DESKTOP_CUSTOM_LOGO
    #[serde(default)]
    pub desktop_custom_logo: Option<String>,
    /// Hash algorithm used for encrypted-library passwords, advertised as
    /// `encrypted_library_pwd_hash_algo` in `/api2/server-info/`. The desktop
    /// and Android clients use it (with `encrypted_library_pwd_hash_params`)
    /// when creating encrypted libraries. `None` keeps the key absent.
    /// Env: NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_ALGO
    #[serde(default)]
    pub encrypted_library_pwd_hash_algo: Option<String>,
    /// Hash algorithm parameters (e.g. `iterations=1000`), advertised as
    /// `encrypted_library_pwd_hash_params` alongside the algo. See
    /// `encrypted_library_pwd_hash_algo`.
    /// Env: NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_PARAMS
    #[serde(default)]
    pub encrypted_library_pwd_hash_params: Option<String>,
    /// Advertise the `file-search` feature (gates the desktop client's search
    /// tab and mobile search). The backend has a search API either way.
    /// Env: NANOFILE_SERVER_FILE_SEARCH_ENABLED
    #[serde(default = "default_true")]
    pub file_search_enabled: bool,
    /// IP addresses of trusted reverse proxies. The `X-Forwarded-For` header is
    /// only honored for rate limiting when the TCP peer is one of these. When
    /// empty (default) rate limiting uses the raw TCP peer address, so clients
    /// cannot spoof the header to bypass per-IP limits.
    /// Env: NANOFILE_SERVER_TRUSTED_PROXIES (comma-separated)
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
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

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: default_addr(),
            port: default_port(),
            version: default_server_version(),
            site_url: default_site_url(),
            max_upload_size_mb: default_max_upload_size_mb(),
            request_timeout_secs: default_request_timeout_secs(),
            cors_allowed_origins: Vec::new(),
            secret_key: String::new(),
            cors_max_age_secs: default_cors_max_age(),
            webdav_enabled: default_true(),
            sso_enabled: default_true(),
            desktop_custom_brand: None,
            desktop_custom_logo: None,
            encrypted_library_pwd_hash_algo: None,
            encrypted_library_pwd_hash_params: None,
            file_search_enabled: default_true(),
            trusted_proxies: Vec::new(),
        }
    }
}

impl ServerConfig {
    /// Whether `site_url` is still the built-in default (`http://127.0.0.1:8082`),
    /// i.e. the admin has not configured an external address.
    ///
    /// Used by download/block URL construction: an unconfigured site_url is not
    /// reachable from other machines, so the URL falls back to the request Host
    /// header instead (mirroring seahub's FILE_SERVER_ROOT semantics, where the
    /// admin-provided root always wins).
    pub fn site_url_is_default(&self) -> bool {
        self.site_url.trim_end_matches('/') == default_site_url()
    }

    /// URL base (`scheme://host[:port]`) for download / block links.
    ///
    /// Mirrors seahub's `FILE_SERVER_ROOT` semantics: a configured `site_url`
    /// (the admin-provided external address) always wins, because only the
    /// admin knows the address clients can actually reach — e.g. the public
    /// hostname behind a reverse proxy. The request Host header is only used
    /// as a fallback while `site_url` is still the built-in default
    /// (`http://127.0.0.1:8082`), so LAN clients hitting the server by IP get
    /// a reachable URL. In that fallback the Host value is used verbatim
    /// (its port is kept when the client actually used one, omitted when it
    /// used the default 80/443) — the server's internal listen port is never
    /// appended, which used to produce unreachable URLs like
    /// `http://host:8082/...` behind a reverse proxy.
    pub fn download_url_base(&self, host_header: Option<&str>) -> String {
        let base = self.site_url.trim_end_matches('/');
        if !self.site_url_is_default() {
            return base.to_string();
        }
        if let Some(h) = host_header {
            return format!("{}://{h}", self.site_url_scheme());
        }
        base.to_string()
    }

    /// Extract the scheme (http / https) from `site_url`.
    pub fn site_url_scheme(&self) -> &str {
        if self.site_url.starts_with("https://") {
            "https"
        } else {
            "http"
        }
    }

    /// Whether cookies should include the `Secure` flag.
    /// Enabled when the site_url scheme is `https`.
    pub fn secure_cookies(&self) -> bool {
        self.site_url.starts_with("https://")
    }

    /// Extract the origin (scheme + host + port) from `site_url`.
    /// e.g. "http://127.0.0.1:8082/some/path" -> "http://127.0.0.1:8082"
    pub fn site_url_origin(&self) -> String {
        let http_prefix = "http://";
        let https_prefix = "https://";
        let prefix = if self.site_url.starts_with(https_prefix) {
            https_prefix.len()
        } else {
            http_prefix.len()
        };
        // Take everything after scheme:// up to the next '/' or end-of-string.
        let rest = &self.site_url[prefix..];
        if let Some(pos) = rest.find('/') {
            format!(
                "{}{}",
                if self.site_url.starts_with(https_prefix) {
                    https_prefix
                } else {
                    http_prefix
                },
                &rest[..pos]
            )
        } else {
            self.site_url.clone()
        }
    }

    /// Return the list of CORS origins to allow.
    /// If `cors_allowed_origins` is empty, uses the origin of `site_url`.
    pub fn cors_origins(&self) -> Vec<String> {
        if self.cors_allowed_origins.is_empty() {
            vec![self.site_url_origin()]
        } else {
            self.cors_allowed_origins.clone()
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_url")]
    pub url: String,
    /// Number of pooled SQLite connections. Under WAL, SQLite allows many
    /// concurrent readers (but a single writer), so this mainly improves read
    /// throughput. Env: NANOFILE_DATABASE_MAX_CONNECTIONS
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StorageConfig {
    #[serde(default = "default_block_dir")]
    pub block_dir: PathBuf,
    #[serde(default = "default_temp_dir")]
    pub temp_dir: PathBuf,
    #[serde(default)]
    pub max_storage_bytes: u64,
    /// Path to the `ffmpeg` binary used to generate video thumbnails. When the
    /// binary isn't found, video files fall back to a play-icon placeholder.
    /// Env: NANOFILE_STORAGE_FFMPEG_PATH
    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,
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

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            block_dir: default_block_dir(),
            temp_dir: default_temp_dir(),
            max_storage_bytes: 0,
            ffmpeg_path: default_ffmpeg_path(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthConfig {
    #[serde(default = "default_password_hash_iterations")]
    pub password_hash_iterations: u32,
    #[serde(default = "default_api_token_ttl_days")]
    pub api_token_ttl_days: u64,
    #[serde(default = "default_sync_token_ttl_days")]
    pub sync_token_ttl_days: u64,
    #[serde(default = "default_five")]
    pub max_login_attempts: u32,
    #[serde(default = "default_lockout_duration_secs")]
    pub lockout_duration_secs: u64,
    /// Whether to show the "Create Account" link on the login page and
    /// allow invitation-code-based registration.
    #[serde(default = "default_true")]
    pub enable_invitations: bool,
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
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            password_hash_iterations: default_password_hash_iterations(),
            api_token_ttl_days: default_api_token_ttl_days(),
            sync_token_ttl_days: default_sync_token_ttl_days(),
            max_login_attempts: default_five(),
            lockout_duration_secs: default_lockout_duration_secs(),
            enable_invitations: default_true(),
            enable_password_reset: default_true(),
            password_min_length: default_password_min_length(),
            require_strong_password: false,
            password_reset_max_per_hour: default_five(),
            registration_max_per_hour: default_five(),
            totp_max_attempts: default_five(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
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
fn default_lockout_duration_secs() -> u64 {
    900
}
fn default_true() -> bool {
    true
}
fn default_five() -> u32 {
    5
}
fn default_password_min_length() -> u32 {
    8
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GcConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gc_interval_hours")]
    pub interval_hours: u64,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: default_gc_interval_hours(),
        }
    }
}

fn default_gc_interval_hours() -> u64 {
    24
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IndexConfig {
    pub enabled: bool,
    pub index_dir: PathBuf,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            index_dir: PathBuf::from("data/index"),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        let path =
            std::env::var(CONFIG_PATH_ENV).unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
        Self::load_from(&path)
    }

    /// Load a config file. Missing fields are filled with built-in defaults and
    /// written back in place (comments preserved), so an upgrade leaves a
    /// visible trace of newly added options. A write failure (e.g. a read-only
    /// mount) only degrades to a warning — the in-memory config is already
    /// complete.
    ///
    /// When the file does not exist, falls back to built-in defaults (plus env
    /// overrides) so the server can start with zero config; other I/O errors
    /// (and unparseable/corrupt files) still fail.
    pub fn load_from(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let original = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "[nanofile] config file {} not found; using built-in defaults",
                    path.display()
                );
                let mut config = Config::default();
                config.apply_env_overrides();
                return Ok(config);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("failed to read {}: {e}", path.display()));
            }
        };

        // Deserialize first (missing fields get serde defaults in memory), then
        // fill the same defaults into the document and write it back if anything
        // was missing. NANOFILE_* overrides are applied last and never written.
        let mut config: Config = toml::from_str(&original)?;
        let mut doc: toml_edit::DocumentMut = original.parse()?;
        let defaults = toml::to_string_pretty(&Config::default())?;
        let defaults_doc: toml_edit::DocumentMut = defaults.parse()?;
        if fill_missing(doc.as_table_mut(), defaults_doc.as_table()) {
            let filled = doc.to_string();
            match persist_with_backup(path, &filled) {
                Ok(()) => tracing::info!("config.toml filled with missing defaults"),
                Err(e) => {
                    tracing::warn!(
                        "config.toml fill succeeded but write-back failed; \
                         using in-memory config: {e}"
                    );
                    // tracing_subscriber is not initialized yet at load time
                    // (it needs config.logging.level), so also echo to stderr.
                    eprintln!(
                        "[nanofile] WARN: config fill applied in memory but \
                         could not write back: {e}"
                    );
                }
            }
        }
        config.apply_env_overrides();
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        macro_rules! env_str {
            ($name:expr, $target:expr) => {
                if let Ok(v) = std::env::var($name) {
                    $target = v;
                }
            };
        }

        macro_rules! env_parse {
            ($name:expr, $target:expr) => {
                if let Ok(v) = std::env::var($name)
                    && let Ok(p) = v.parse()
                {
                    $target = p;
                }
            };
        }

        macro_rules! env_path {
            ($name:expr, $target:expr) => {
                if let Ok(v) = std::env::var($name) {
                    $target = PathBuf::from(v);
                }
            };
        }

        env_str!("NANOFILE_SERVER_ADDR", self.server.addr);
        env_str!("NANOFILE_SERVER_VERSION", self.server.version);
        env_parse!("NANOFILE_SERVER_PORT", self.server.port);
        env_parse!(
            "NANOFILE_SERVER_MAX_UPLOAD_SIZE_MB",
            self.server.max_upload_size_mb
        );
        env_str!("NANOFILE_SERVER_SITE_URL", self.server.site_url);
        env_str!("NANOFILE_SERVER_SECRET_KEY", self.server.secret_key);
        env_parse!(
            "NANOFILE_SERVER_REQUEST_TIMEOUT_SECS",
            self.server.request_timeout_secs
        );
        env_str!("NANOFILE_DATABASE_URL", self.database.url);
        env_parse!(
            "NANOFILE_DATABASE_MAX_CONNECTIONS",
            self.database.max_connections
        );
        env_path!("NANOFILE_STORAGE_BLOCK_DIR", self.storage.block_dir);
        env_path!("NANOFILE_STORAGE_TEMP_DIR", self.storage.temp_dir);
        env_str!("NANOFILE_STORAGE_FFMPEG_PATH", self.storage.ffmpeg_path);
        env_parse!(
            "NANOFILE_STORAGE_MAX_STORAGE_BYTES",
            self.storage.max_storage_bytes
        );
        env_parse!(
            "NANOFILE_AUTH_PASSWORD_HASH_ITERATIONS",
            self.auth.password_hash_iterations
        );
        env_parse!(
            "NANOFILE_AUTH_API_TOKEN_TTL_DAYS",
            self.auth.api_token_ttl_days
        );
        env_parse!(
            "NANOFILE_AUTH_SYNC_TOKEN_TTL_DAYS",
            self.auth.sync_token_ttl_days
        );
        env_parse!(
            "NANOFILE_AUTH_MAX_LOGIN_ATTEMPTS",
            self.auth.max_login_attempts
        );
        env_parse!(
            "NANOFILE_AUTH_LOCKOUT_DURATION_SECS",
            self.auth.lockout_duration_secs
        );
        env_parse!(
            "NANOFILE_AUTH_ENABLE_INVITATIONS",
            self.auth.enable_invitations
        );
        env_parse!(
            "NANOFILE_AUTH_ENABLE_PASSWORD_RESET",
            self.auth.enable_password_reset
        );
        env_parse!(
            "NANOFILE_AUTH_PASSWORD_MIN_LENGTH",
            self.auth.password_min_length
        );
        env_parse!(
            "NANOFILE_AUTH_REQUIRE_STRONG_PASSWORD",
            self.auth.require_strong_password
        );
        env_str!("NANOFILE_LOG_LEVEL", self.logging.level);
        env_parse!("NANOFILE_GC_ENABLED", self.gc.enabled);
        env_parse!("NANOFILE_GC_INTERVAL_HOURS", self.gc.interval_hours);
        env_parse!("NANOFILE_NOTIFICATION_ENABLED", self.notification.enabled);
        env_str!(
            "NANOFILE_NOTIFICATION_PRIVATE_KEY",
            self.notification.private_key
        );
        env_parse!(
            "NANOFILE_NOTIFICATION_PING_INTERVAL",
            self.notification.ping_interval
        );
        env_parse!(
            "NANOFILE_NOTIFICATION_CLIENT_TIMEOUT",
            self.notification.client_timeout
        );
        env_parse!("NANOFILE_INDEX_ENABLED", self.index.enabled);
        env_path!("NANOFILE_INDEX_INDEX_DIR", self.index.index_dir);
        env_parse!("NANOFILE_CORS_MAX_AGE_SECS", self.server.cors_max_age_secs);
        env_parse!("NANOFILE_SERVER_WEBDAV_ENABLED", self.server.webdav_enabled);
        env_parse!("NANOFILE_SERVER_SSO_ENABLED", self.server.sso_enabled);
        env_parse!(
            "NANOFILE_SERVER_FILE_SEARCH_ENABLED",
            self.server.file_search_enabled
        );
        // Optional server-info fields: an empty env var keeps the field absent.
        if let Ok(v) = std::env::var("NANOFILE_SERVER_DESKTOP_CUSTOM_BRAND") {
            self.server.desktop_custom_brand = Some(v);
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_DESKTOP_CUSTOM_LOGO") {
            self.server.desktop_custom_logo = Some(v);
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_ALGO") {
            self.server.encrypted_library_pwd_hash_algo = Some(v);
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_ENCRYPTED_LIBRARY_PWD_HASH_PARAMS") {
            self.server.encrypted_library_pwd_hash_params = Some(v);
        }
        env_parse!("NANOFILE_EMAIL_ENABLED", self.email.enabled);
        env_str!("NANOFILE_UI_DEFAULT_LANGUAGE", self.ui.default_language);

        // Admin init env vars
        if let Ok(v) = std::env::var("NANOFILE_ADMIN_INIT_EMAIL") {
            self.admin_init.email = Some(v);
        }
        if let Ok(v) = std::env::var("NANOFILE_ADMIN_INIT_PASSWORD") {
            tracing::warn!(
                "NANOFILE_ADMIN_INIT_PASSWORD is set via environment variable. \
                 Consider using NANOFILE_ADMIN_INIT_PASSWORD_FILE instead, \
                 which is less likely to leak via process listings or logs."
            );
            self.admin_init.password = Some(v);
        }
        if let Ok(filepath) = std::env::var("NANOFILE_ADMIN_INIT_PASSWORD_FILE") {
            match std::fs::read_to_string(&filepath) {
                Ok(password) => {
                    self.admin_init.password = Some(password.trim().to_string());
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to read NANOFILE_ADMIN_INIT_PASSWORD_FILE from {}: {}",
                        filepath,
                        e
                    );
                }
            }
        }

        // Comma-separated list
        if let Ok(v) = std::env::var("NANOFILE_CORS_ALLOWED_ORIGINS") {
            self.server.cors_allowed_origins = v.split(',').map(|s| s.trim().to_string()).collect();
        }

        // Comma-separated trusted proxy IP list.
        if let Ok(v) = std::env::var("NANOFILE_SERVER_TRUSTED_PROXIES") {
            self.server.trusted_proxies = v.split(',').map(|s| s.trim().to_string()).collect();
        }
    }
}

/// Recursively insert keys from `src` that are missing in `dst`, leaving
/// existing keys (and their values/comments) untouched. Returns whether any key
/// was inserted.
fn fill_missing(dst: &mut toml_edit::Table, src: &toml_edit::Table) -> bool {
    let mut changed = false;
    for (key, value) in src.iter() {
        match dst.get_mut(key) {
            None => {
                dst.insert(key, value.clone());
                changed = true;
            }
            Some(toml_edit::Item::Table(d)) => {
                if let toml_edit::Item::Table(s) = value {
                    changed |= fill_missing(d, s);
                }
            }
            Some(_) => {}
        }
    }
    changed
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

/// Atomically replace `path` with `filled`, keeping the previous content as
/// `path.bak`. Fails without touching `path` when the backup or the write
/// fails. The backup is taken with `fs::copy` so file permissions carry over.
fn persist_with_backup(path: &Path, filled: &str) -> anyhow::Result<()> {
    std::fs::copy(path, backup_path(path)).with_context(|| {
        format!(
            "failed to back up {} to {}",
            path.display(),
            backup_path(path).display()
        )
    })?;
    atomic_write(path, filled)
}

/// Write `content` to `path` via a same-directory temp file + rename. Keeps
/// the original file's permissions and fsyncs before renaming so the replaced
/// file is never observed half-written. The temp file is cleaned up on failure.
fn atomic_write(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp = tmp_path(path);
    let result = (|| -> anyhow::Result<()> {
        let perms = std::fs::metadata(path)?.permissions();
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            f.set_permissions(perms)?;
        }
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
        // Direct LAN access: Host carries the port, keep it verbatim.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(
            c.download_url_base(Some("192.168.1.100:8082")),
            "http://192.168.1.100:8082"
        );
        // Domain with explicit port.
        assert_eq!(
            c.download_url_base(Some("seafile.example.com:8082")),
            "http://seafile.example.com:8082"
        );
    }

    #[test]
    fn default_site_url_falls_back_to_host_without_port() {
        // Reverse proxy without a port in Host (80/443): never append the
        // internal listen port — the old behavior produced unreachable URLs.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(
            c.download_url_base(Some("seafile.example.com")),
            "http://seafile.example.com"
        );
        assert_eq!(
            c.download_url_base(Some("192.168.1.100")),
            "http://192.168.1.100"
        );
    }

    #[test]
    fn default_site_url_ipv6_host() {
        // IPv6 literals (with or without port) pass through verbatim — the
        // old split_once(':') logic produced malformed URLs for these.
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

    #[test]
    fn default_site_url_without_host_header() {
        // No Host header at all: fall back to site_url itself.
        let c = cfg("http://127.0.0.1:8082");
        assert_eq!(c.download_url_base(None), "http://127.0.0.1:8082");
    }

    #[test]
    fn fill_missing_inserts_missing_sections_and_fields() {
        let mut dst: toml_edit::DocumentMut = "[server]\naddr = \"1.2.3.4\"\n".parse().unwrap();
        let src: toml_edit::DocumentMut = toml::to_string_pretty(&Config::default())
            .unwrap()
            .parse()
            .unwrap();

        let changed = fill_missing(dst.as_table_mut(), src.as_table());
        assert!(changed);

        let out = dst.to_string();
        assert!(out.contains("addr = \"1.2.3.4\"")); // 原字段保持
        assert!(out.contains("port = 8082")); // 缺失字段补默认
        assert!(out.contains("[database]")); // 缺失 section 补默认
    }

    #[test]
    fn fill_missing_keeps_existing_values() {
        let mut dst: toml_edit::DocumentMut = "[server]\nport = 1234\n".parse().unwrap();
        let src: toml_edit::DocumentMut = toml::to_string_pretty(&Config::default())
            .unwrap()
            .parse()
            .unwrap();

        let changed = fill_missing(dst.as_table_mut(), src.as_table());
        assert!(changed);

        let out = dst.to_string();
        assert!(out.contains("port = 1234")); // 已有字段不被默认值覆盖
    }

    #[test]
    fn load_complete_config_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let complete = toml::to_string_pretty(&Config::default()).unwrap();
        std::fs::write(&path, &complete).unwrap();

        let config = Config::load_from(&path).expect("load complete config");
        assert_eq!(config.server.port, 8082);

        // 完整配置无缺失字段:文件不变、无备份。
        assert_eq!(std::fs::read_to_string(&path).unwrap(), complete);
        assert!(!std::path::Path::new(&format!("{}.bak", path.display())).exists());
    }

    #[test]
    fn load_incomplete_config_gets_filled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\naddr = \"1.2.3.4\"\n").unwrap();

        let config = Config::load_from(&path).expect("load incomplete config");
        assert_eq!(config.server.addr, "1.2.3.4");
        assert_eq!(config.server.port, 8082); // 默认补齐

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("port = 8082")); // 缺失字段写回
        assert!(written.contains("[database]")); // 缺失 section 写回
        assert!(std::path::Path::new(&format!("{}.bak", path.display())).exists());
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
    }
}
