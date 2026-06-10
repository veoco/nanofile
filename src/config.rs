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
    pub max_upload_size_mb: u64,
    pub request_timeout_secs: u64,
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
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct AdminInitConfig {
    pub email: Option<String>,
    pub password: Option<String>,
}

fn default_true() -> bool {
    true
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
            self.admin_init.password = Some(v);
        }
    }
}
