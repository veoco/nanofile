use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub auth: AuthConfig,
    pub logging: LoggingConfig,
    pub gc: GcConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub notification: NotificationConfig,
    #[serde(default)]
    pub admin_init: AdminInitConfig,
}

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub addr: String,
    pub port: u16,
    /// External URL for this server, e.g. "http://127.0.0.1:8082".
    /// Used to construct download/upload/share URLs and as the default CORS origin.
    /// If empty at startup, derived from addr:port as http://{addr}:{port}.
    #[serde(default = "default_site_url")]
    pub site_url: String,
    pub max_upload_size_mb: u64,
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
}

fn default_site_url() -> String {
    "http://127.0.0.1:8082".to_string()
}
fn default_cors_max_age() -> u64 {
    86400
}

impl ServerConfig {
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

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    pub block_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub max_storage_bytes: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    pub password_hash_iterations: u32,
    pub api_token_ttl_days: u64,
    pub sync_token_ttl_days: u64,
    pub max_login_attempts: u32,
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

#[derive(Debug, Deserialize, Clone, Default)]
pub struct AdminInitConfig {
    pub email: Option<String>,
    pub password: Option<String>,
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

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub level: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GcConfig {
    pub enabled: bool,
    pub interval_hours: u64,
    pub keep_commits: u64,
}

#[derive(Debug, Deserialize, Clone)]
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
        let config_str = std::fs::read_to_string("config.toml")?;
        let mut config: Config = toml::from_str(&config_str)?;
        config.apply_env_overrides();
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("NANOFILE_SERVER_ADDR") {
            self.server.addr = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_PORT")
            && let Ok(p) = v.parse()
        {
            self.server.port = p;
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_MAX_UPLOAD_SIZE_MB")
            && let Ok(n) = v.parse()
        {
            self.server.max_upload_size_mb = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_SITE_URL") {
            self.server.site_url = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_SECRET_KEY") {
            self.server.secret_key = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_SERVER_REQUEST_TIMEOUT_SECS")
            && let Ok(n) = v.parse()
        {
            self.server.request_timeout_secs = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_DATABASE_URL") {
            self.database.url = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_STORAGE_BLOCK_DIR") {
            self.storage.block_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("NANOFILE_STORAGE_TEMP_DIR") {
            self.storage.temp_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("NANOFILE_STORAGE_MAX_STORAGE_BYTES")
            && let Ok(n) = v.parse()
        {
            self.storage.max_storage_bytes = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_PASSWORD_HASH_ITERATIONS")
            && let Ok(n) = v.parse()
        {
            self.auth.password_hash_iterations = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_API_TOKEN_TTL_DAYS")
            && let Ok(n) = v.parse()
        {
            self.auth.api_token_ttl_days = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_SYNC_TOKEN_TTL_DAYS")
            && let Ok(n) = v.parse()
        {
            self.auth.sync_token_ttl_days = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_MAX_LOGIN_ATTEMPTS")
            && let Ok(n) = v.parse()
        {
            self.auth.max_login_attempts = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_LOCKOUT_DURATION_SECS")
            && let Ok(n) = v.parse()
        {
            self.auth.lockout_duration_secs = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_ENABLE_INVITATIONS")
            && let Ok(b) = v.parse()
        {
            self.auth.enable_invitations = b;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_ENABLE_PASSWORD_RESET")
            && let Ok(b) = v.parse()
        {
            self.auth.enable_password_reset = b;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_PASSWORD_MIN_LENGTH")
            && let Ok(n) = v.parse()
        {
            self.auth.password_min_length = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_AUTH_REQUIRE_STRONG_PASSWORD")
            && let Ok(b) = v.parse()
        {
            self.auth.require_strong_password = b;
        }
        if let Ok(v) = std::env::var("NANOFILE_LOG_LEVEL") {
            self.logging.level = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_GC_ENABLED")
            && let Ok(b) = v.parse()
        {
            self.gc.enabled = b;
        }
        if let Ok(v) = std::env::var("NANOFILE_GC_INTERVAL_HOURS")
            && let Ok(n) = v.parse()
        {
            self.gc.interval_hours = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_GC_KEEP_COMMITS")
            && let Ok(n) = v.parse()
        {
            self.gc.keep_commits = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_NOTIFICATION_ENABLED")
            && let Ok(b) = v.parse()
        {
            self.notification.enabled = b;
        }
        if let Ok(v) = std::env::var("NANOFILE_NOTIFICATION_PRIVATE_KEY") {
            self.notification.private_key = v;
        }
        if let Ok(v) = std::env::var("NANOFILE_NOTIFICATION_PING_INTERVAL")
            && let Ok(n) = v.parse()
        {
            self.notification.ping_interval = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_NOTIFICATION_CLIENT_TIMEOUT")
            && let Ok(n) = v.parse()
        {
            self.notification.client_timeout = n;
        }
        if let Ok(v) = std::env::var("NANOFILE_INDEX_ENABLED")
            && let Ok(b) = v.parse()
        {
            self.index.enabled = b;
        }
        if let Ok(v) = std::env::var("NANOFILE_INDEX_INDEX_DIR") {
            self.index.index_dir = PathBuf::from(v);
        }
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
        if let Ok(v) = std::env::var("NANOFILE_CORS_ALLOWED_ORIGINS") {
            self.server.cors_allowed_origins = v.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(v) = std::env::var("NANOFILE_CORS_MAX_AGE_SECS")
            && let Ok(n) = v.parse()
        {
            self.server.cors_max_age_secs = n;
        }
    }
}
