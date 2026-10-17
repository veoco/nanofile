//! What a first run asks for, and what the answers mean, free of HTTP and of the
//! database: plain values, unit-testable, and the page only renders them.

use std::path::{Path, PathBuf};

use infra::config::{Config, ConfigSection, ConfigValue, USER_CONFIG_DIR_NAME};

use crate::service::auth::password::validate_password;

/// Document text above the sections a completed setup writes, as in `config.toml.example`:
/// only what the web interface cannot store, plus values decided before it is reachable.
pub const CONFIG_HEADER: &str = "\
Nanofile configuration.

Written by the first-run setup. It holds only the keys that have to be decided
before the web interface is reachable: the listener, the external URL, the
master secret, where state lives, the at-rest encryption mode and the log file.
Every other setting is managed at \"System Settings\", and any NANOFILE_*
environment variable overrides this file and is never written back into it.

Keep this file private: it holds the master secret. Delete it to run the setup
again.";

/// What the setup page is prefilled with, and what it validates against.
#[derive(Debug, Clone)]
pub struct SetupDefaults {
    pub addr: String,
    pub port: u16,
    pub site_url: String,
    pub data_dir: String,
    pub encryption: String,
    pub password_min_length: u32,
    pub require_strong_password: bool,
    pub language: String,
    /// The configured address could not be bound: the page is served on a random loopback port.
    pub port_in_use: bool,
    /// The address the setup listener holds: available to the submit though a bind fails.
    pub held_address: Option<String>,
    pub existing_accounts: Option<u64>,
}

impl SetupDefaults {
    /// The defaults a zero-config start would use: `config` holds the built-in defaults.
    pub fn detect(config: &Config) -> Self {
        let data_dir = default_data_dir(
            infra::common::util::exe_dir().as_deref(),
            infra::common::util::home_dir().as_deref(),
        );
        Self {
            addr: config.server.addr.clone(),
            port: config.server.port,
            site_url: format!("http://127.0.0.1:{}", config.server.port),
            data_dir: data_dir.to_string_lossy().to_string(),
            encryption: config.storage.block_encryption_mode.clone(),
            password_min_length: config.auth.password_min_length,
            require_strong_password: config.auth.require_strong_password,
            language: config.ui.default_language.clone(),
            port_in_use: false,
            held_address: None,
            existing_accounts: None,
        }
    }
}

/// The data directory a first run suggests: **never move existing state**, since a
/// `data/` next to the binary is where earlier deployments kept their blocks, and a
/// deleted configuration file must not make an installation look gone.
pub fn default_data_dir(exe_dir: Option<&Path>, home: Option<&Path>) -> PathBuf {
    if let Some(exe_dir) = exe_dir {
        let legacy = exe_dir.join("data");
        if legacy.is_dir() {
            return legacy;
        }
    }
    if let Some(home) = home {
        return home.join(USER_CONFIG_DIR_NAME).join("data");
    }
    exe_dir
        .map(|dir| dir.join("data"))
        .unwrap_or_else(|| PathBuf::from("data"))
}

/// The submitted form, as the browser sent it: `port` is text on purpose, so a
/// non-numeric value is a field error on the page, not an extractor rejection.
#[derive(Clone, Default, serde::Deserialize)]
pub struct SetupForm {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub password_confirm: String,
    #[serde(default)]
    pub addr: String,
    #[serde(default)]
    pub port: String,
    #[serde(default)]
    pub site_url: String,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub encryption: String,
}

/// A rejected form: which field, and which locale key says why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupError {
    pub field: &'static str,
    pub key: &'static str,
}

impl SetupError {
    fn on(field: &'static str, key: &'static str) -> Self {
        Self { field, key }
    }
}

#[derive(Clone)]
pub struct SetupPlan {
    pub email: String,
    /// Only ever held in memory: the account is created directly, never written down.
    pub password: String,
    pub secret_key: String,
    pub addr: String,
    pub port: u16,
    pub site_url: String,
    pub data_dir: PathBuf,
    pub encryption: String,
}

/// `password` and `secret_key` print as placeholders, so a `{:?}` cannot leak either.
impl std::fmt::Debug for SetupPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupPlan")
            .field("email", &self.email)
            .field("password", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("addr", &self.addr)
            .field("port", &self.port)
            .field("site_url", &self.site_url)
            .field("data_dir", &self.data_dir)
            .field("encryption", &self.encryption)
            .finish()
    }
}

impl SetupForm {
    /// Validate the answers into a plan, or report the first field that is wrong. The
    /// data directory is created here, so an unwritable one fails while somebody watches.
    pub fn into_plan(&self, defaults: &SetupDefaults) -> Result<SetupPlan, SetupError> {
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.len() > 254 {
            return Err(SetupError::on("email", "setup.err_email"));
        }

        if self.password != self.password_confirm {
            return Err(SetupError::on(
                "password_confirm",
                "setup.err_password_mismatch",
            ));
        }
        validate_password(
            &self.password,
            defaults.password_min_length,
            defaults.require_strong_password,
        )
        .map_err(|_| SetupError::on("password", "setup.err_password"))?;

        let addr = self.addr.trim();
        if addr.is_empty() || addr.contains(char::is_whitespace) {
            return Err(SetupError::on("addr", "setup.err_addr"));
        }
        let port: u16 = self
            .port
            .trim()
            .parse()
            .ok()
            .filter(|port| *port != 0)
            .ok_or(SetupError::on("port", "setup.err_port"))?;

        // The address this process already serves the page on is available by
        // definition, and so is another interface on that same *port*: the listener is
        // closed (or handed over) before the server binds; anything else must bind *now*.
        let address = format!("{addr}:{port}");
        let same_address = defaults.held_address.as_deref() == Some(address.as_str());
        let on_the_held_port = defaults
            .held_address
            .as_deref()
            .and_then(|held| held.rsplit_once(':'))
            .is_some_and(|(_, held_port)| held_port.parse::<u16>().ok() == Some(port));
        if !same_address && !on_the_held_port && std::net::TcpListener::bind(&address).is_err() {
            return Err(SetupError::on("port", "setup.err_port_in_use"));
        }

        let site_url = self.site_url.trim();
        let site_url = if site_url.is_empty() {
            format!("http://127.0.0.1:{port}")
        } else if site_url.contains(char::is_whitespace) {
            return Err(SetupError::on("site_url", "setup.err_site_url"));
        } else {
            site_url.to_string()
        };

        let raw_data_dir = self.data_dir.trim();
        if raw_data_dir.is_empty() {
            return Err(SetupError::on("data_dir", "setup.err_data_dir"));
        }
        let data_dir = PathBuf::from(raw_data_dir);
        let data_dir = if data_dir.is_absolute() {
            data_dir
        } else {
            infra::config::state_path_base().join(data_dir)
        };
        infra::common::util::ensure_private_dir(&data_dir)
            .map_err(|_| SetupError::on("data_dir", "setup.err_data_dir"))?;

        let encryption = match self.encryption.trim().to_ascii_lowercase().as_str() {
            "" | "off" => "off".to_string(),
            "on" => "on".to_string(),
            "lazy" => "lazy".to_string(),
            _ => return Err(SetupError::on("encryption", "setup.err_encryption")),
        };

        Ok(SetupPlan {
            email: email.to_string(),
            password: self.password.clone(),
            secret_key: generate_secret_key(),
            addr: addr.to_string(),
            port,
            site_url,
            data_dir,
            encryption,
        })
    }
}

impl SetupPlan {
    pub fn listener_address(&self) -> String {
        format!("{}:{}", self.addr, self.port)
    }

    pub fn database_url(&self) -> String {
        format!("sqlite:{}?mode=rwc", self.database_file().display())
    }

    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("nanofile.db")
    }

    pub fn log_path(&self) -> PathBuf {
        self.data_dir.join("nanofile.log")
    }

    /// Every directory the plan needs, in creation order, named by its setting.
    pub fn directories(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("data directory", self.data_dir.clone()),
            ("storage.block_dir", self.data_dir.join("blocks")),
            ("storage.temp_dir", self.data_dir.join("temp")),
            ("storage.thumbnail_dir", self.data_dir.join("thumbnails")),
            ("storage.avatar_dir", self.data_dir.join("avatars")),
            ("index.index_dir", self.data_dir.join("index")),
        ]
    }

    /// Apply the plan to the in-memory configuration used before anything is written.
    pub fn apply_to(&self, config: &mut Config) {
        config.server.addr = self.addr.clone();
        config.server.port = self.port;
        config.server.site_url = self.site_url.clone();
        config.server.secret_key = self.secret_key.clone();
        config.database.url = self.database_url();
        config.storage.block_dir = self.data_dir.join("blocks");
        config.storage.temp_dir = self.data_dir.join("temp");
        config.storage.thumbnail_dir = self.data_dir.join("thumbnails");
        config.storage.avatar_dir = self.data_dir.join("avatars");
        config.storage.block_encryption_mode = self.encryption.clone();
        config.index.index_dir = self.data_dir.join("index");
        config.logging.file = Some(self.log_path());
    }

    /// The sections of the configuration file this plan writes, with absolute paths
    /// throughout: a relative one would resolve against the binary's directory, not
    /// against the file's location.
    pub fn sections(&self) -> Vec<ConfigSection<'static>> {
        let text = |value: String| ConfigValue::Text(std::borrow::Cow::Owned(value));
        let path = |value: PathBuf| text(value.to_string_lossy().to_string());

        let mut storage = vec![
            ("block_dir", path(self.data_dir.join("blocks"))),
            ("temp_dir", path(self.data_dir.join("temp"))),
            ("thumbnail_dir", path(self.data_dir.join("thumbnails"))),
            ("avatar_dir", path(self.data_dir.join("avatars"))),
        ];
        // Off is the default and is not written: only a mode that changes how bytes are read.
        if self.encryption != "off" {
            storage.push(("block_encryption_mode", text(self.encryption.clone())));
        }

        vec![
            ConfigSection {
                name: "server",
                comment: "Listener, external URL and master secret.",
                entries: vec![
                    ("addr", text(self.addr.clone())),
                    ("port", ConfigValue::Integer(u64::from(self.port))),
                    ("site_url", text(self.site_url.clone())),
                    ("secret_key", text(self.secret_key.clone())),
                ],
            },
            ConfigSection {
                name: "database",
                comment: "SQLite database. Absolute, so every start uses this one.",
                entries: vec![("url", text(self.database_url()))],
            },
            ConfigSection {
                name: "storage",
                comment: "State directories, decided before the first start.",
                entries: storage,
            },
            ConfigSection {
                name: "index",
                // `enabled` has no serde default, so a `[index]` table without it is rejected.
                comment: "Full-text search index.",
                entries: vec![
                    ("enabled", ConfigValue::Boolean(true)),
                    ("index_dir", path(self.data_dir.join("index"))),
                ],
            },
            ConfigSection {
                name: "logging",
                // Written anyway: a desktop run logs beside the state, not where it started.
                comment: "Only used when logs go to a file (desktop runs).",
                entries: vec![("file", path(self.log_path()))],
            },
        ]
    }
}

/// A 256-bit master key, hex-encoded, generated once per completed setup.
pub fn generate_secret_key() -> String {
    use rand::Rng;

    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);
    hex::encode(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> SetupDefaults {
        SetupDefaults {
            addr: "0.0.0.0".into(),
            port: 8082,
            site_url: "http://127.0.0.1:8082".into(),
            data_dir: "/tmp/nanofile-default".into(),
            encryption: "off".into(),
            password_min_length: 8,
            require_strong_password: false,
            language: "en".into(),
            port_in_use: false,
            held_address: Some("0.0.0.0:8082".into()),
            existing_accounts: None,
        }
    }

    fn valid_form(dir: &Path) -> SetupForm {
        SetupForm {
            email: "admin@example.com".into(),
            password: "secret123".into(),
            password_confirm: "secret123".into(),
            addr: "0.0.0.0".into(),
            port: "8082".into(),
            site_url: String::new(),
            data_dir: dir.to_string_lossy().to_string(),
            encryption: "off".into(),
        }
    }

    #[test]
    fn a_filled_form_becomes_a_plan() {
        let dir = tempfile::tempdir().unwrap();
        let defaults = defaults();
        let plan = valid_form(dir.path())
            .into_plan(&defaults)
            .expect("valid form");

        assert_eq!(plan.email, "admin@example.com");
        assert_eq!(plan.addr, "0.0.0.0");
        assert_eq!(plan.port, 8082);
        assert_eq!(
            plan.site_url, "http://127.0.0.1:8082",
            "an empty site URL follows the port"
        );
        assert_eq!(plan.data_dir, dir.path());
        assert_eq!(plan.encryption, "off");
        // A 64-hex key, which is what the release check demands of a persisted secret.
        assert_eq!(plan.secret_key.len(), 64);
        assert!(plan.secret_key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn every_field_reports_its_own_rejection() {
        let dir = tempfile::tempdir().unwrap();

        let mut bad = valid_form(dir.path());
        bad.email = "not-an-address".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("email", "setup.err_email")
        );

        let mut bad = valid_form(dir.path());
        bad.password_confirm = "different1".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("password_confirm", "setup.err_password_mismatch")
        );

        let mut bad = valid_form(dir.path());
        bad.password = "short".into();
        bad.password_confirm = "short".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("password", "setup.err_password")
        );

        let mut strict = defaults();
        strict.password_min_length = 32;
        let mut ok = valid_form(dir.path());
        ok.password = "secret123".into();
        ok.password_confirm = "secret123".into();
        assert_eq!(
            ok.into_plan(&strict).unwrap_err(),
            SetupError::on("password", "setup.err_password")
        );

        for port in ["0", "70000", "", "   ", "eighty"] {
            let mut bad = valid_form(dir.path());
            bad.port = port.into();
            assert_eq!(
                bad.into_plan(&defaults()).unwrap_err(),
                SetupError::on("port", "setup.err_port"),
                "port={port:?}"
            );
        }

        let mut bad = valid_form(dir.path());
        bad.addr = "  ".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("addr", "setup.err_addr")
        );

        let mut bad = valid_form(dir.path());
        bad.site_url = "http://two words/".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("site_url", "setup.err_site_url")
        );

        let mut bad = valid_form(dir.path());
        bad.data_dir = "".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("data_dir", "setup.err_data_dir")
        );

        let mut bad = valid_form(dir.path());
        bad.encryption = "maybe".into();
        assert_eq!(
            bad.into_plan(&defaults()).unwrap_err(),
            SetupError::on("encryption", "setup.err_encryption")
        );
    }

    #[test]
    fn the_address_the_page_is_served_on_counts_as_available() {
        let dir = tempfile::tempdir().unwrap();

        // A fresh bind of this address fails, so rejecting it would refuse the page's defaults.
        let mut defaults = defaults();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let held = listener.local_addr().unwrap();
        defaults.held_address = Some(held.to_string());
        let mut form = valid_form(dir.path());
        form.addr = held.ip().to_string();
        form.port = held.port().to_string();
        assert!(form.into_plan(&defaults).is_ok());

        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = taken.local_addr().unwrap();
        let mut form = valid_form(dir.path());
        form.addr = taken.ip().to_string();
        form.port = taken.port().to_string();
        assert_eq!(
            form.into_plan(&defaults).unwrap_err(),
            SetupError::on("port", "setup.err_port_in_use")
        );
    }

    #[test]
    fn a_relative_data_directory_resolves_against_the_binary_directory() {
        let mut form = valid_form(Path::new("."));
        form.data_dir = "nanofile-setup-test-data".into();
        let plan = form.into_plan(&defaults()).unwrap();
        assert!(plan.data_dir.is_absolute(), "{:?}", plan.data_dir);
        assert!(plan.data_dir.ends_with("nanofile-setup-test-data"));
        let _ = std::fs::remove_dir(&plan.data_dir);
    }

    #[test]
    fn the_document_carries_every_value_the_page_collected() {
        let dir = tempfile::tempdir().unwrap();
        let mut form = valid_form(dir.path());
        form.port = "9090".into();
        form.site_url = "https://nanofile.example.com".into();
        form.encryption = "on".into();
        let plan = form.into_plan(&defaults()).unwrap();

        let sections = plan.sections();
        let path = dir.path().join("config.toml");
        infra::config::write_new_config(&path, CONFIG_HEADER, &sections).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: Config = toml::from_str(&text).expect("the document parses");
        assert_eq!(parsed.server.port, 9090);
        assert_eq!(parsed.server.addr, "0.0.0.0");
        assert_eq!(parsed.server.site_url, "https://nanofile.example.com");
        assert_eq!(parsed.server.secret_key, plan.secret_key);
        assert_eq!(parsed.database.url, plan.database_url());
        assert_eq!(parsed.storage.block_dir, dir.path().join("blocks"));
        assert_eq!(parsed.storage.temp_dir, dir.path().join("temp"));
        assert_eq!(parsed.storage.thumbnail_dir, dir.path().join("thumbnails"));
        assert_eq!(parsed.storage.avatar_dir, dir.path().join("avatars"));
        assert_eq!(parsed.storage.block_encryption_mode, "on");
        assert_eq!(parsed.index.index_dir, dir.path().join("index"));
        assert_eq!(parsed.logging.file, Some(plan.log_path()));

        // The plan's in-memory configuration has to agree with the document: the
        // account is created through one and every later start reads the other.
        let mut applied = Config::default();
        plan.apply_to(&mut applied);
        assert_eq!(
            toml::to_string_pretty(&applied).unwrap(),
            toml::to_string_pretty(&parsed).unwrap()
        );
    }

    #[test]
    fn the_default_data_directory_never_moves_existing_state() {
        let dir = tempfile::tempdir().unwrap();
        let exe_dir = dir.path().join("install");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        // No state yet: everything goes under the per-user directory holding the config.
        assert_eq!(
            default_data_dir(Some(&exe_dir), Some(&home)),
            home.join(USER_CONFIG_DIR_NAME).join("data")
        );

        // The legacy location exists and is adopted, so a deleted config cannot hide the data.
        std::fs::create_dir_all(exe_dir.join("data")).unwrap();
        assert_eq!(
            default_data_dir(Some(&exe_dir), Some(&home)),
            exe_dir.join("data")
        );

        // Without a home directory there is nowhere per-user to go.
        assert_eq!(default_data_dir(Some(&exe_dir), None), exe_dir.join("data"));
    }
}
