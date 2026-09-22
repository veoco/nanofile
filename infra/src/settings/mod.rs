//! Runtime-managed settings: a catalog of every `Config` field, the layer
//! policy that decides which source wins, and the origin of each effective
//! value.
//!
//! # Layers, highest first
//!
//! 1. **Environment** (`NANOFILE_*`) — always wins. The admin UI shows these
//!    read-only together with the variable name, because a save that cannot take
//!    effect would be a lie.
//! 2. **Config file** — only for the keys the `[settings]` policy lets override
//!    the database (`config_policy = "override"`, or a key listed in
//!    `config_override_keys`).
//! 3. **Database** — the `settings` row saved from `/sysadmin/settings/`.
//! 4. **Built-in default**.
//!
//! The config file is otherwise *bootstrap*: a key with no stored row takes its
//! value from the file, and the file is ignored for that key once a row exists.
//! That is why the origin of a value whose config-file entry merely repeats the
//! built-in default is reported as [`Origin::Default`] — the two are
//! indistinguishable and mean the same thing.
//!
//! # Why a catalog
//!
//! [`CATALOG`] is the single source of truth for which fields exist, what they
//! are called on the wire (`NANOFILE_*`), how they are parsed, and whether a
//! change can take effect without a restart. The environment layer is applied
//! *from* the catalog, so a setting cannot be added without also wiring its
//! variable — the two can no longer drift apart.

pub mod catalog;

use std::collections::{BTreeMap, BTreeSet};

use crate::config::Config;

pub use catalog::CATALOG;

/// The admin page a setting is shown on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    General,
    Security,
    Storage,
    Email,
    Advanced,
}

impl Section {
    /// Stable identifier used in the URL (`/sysadmin/settings/<id>/`).
    pub const fn id(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Security => "security",
            Section::Storage => "storage",
            Section::Email => "email",
            Section::Advanced => "advanced",
        }
    }

    /// Parse a URL segment.
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "general" => Some(Section::General),
            "security" => Some(Section::Security),
            "storage" => Some(Section::Storage),
            "email" => Some(Section::Email),
            "advanced" => Some(Section::Advanced),
            _ => None,
        }
    }

    /// Every section, in page order.
    pub const ALL: [Section; 5] = [
        Section::General,
        Section::Security,
        Section::Storage,
        Section::Email,
        Section::Advanced,
    ];
}

/// The value shape, which decides both the form control and the parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    U16,
    U32,
    U64,
    I32,
    Usize,
    /// Free text.
    Text,
    /// `Option<String>`: an empty value is `None`.
    OptText,
    /// A write-only value: never rendered back into HTML.
    Secret,
    /// A filesystem path.
    Path,
    /// `Option<bool>`: an empty value is `None`.
    OptBool,
    /// A string restricted to a fixed set.
    Enum(&'static [&'static str]),
    /// `Vec<String>`, comma-separated.
    TextList,
    /// `Vec<u64>`, comma-separated.
    NumList,
}

/// A long-lived object a live change has to be pushed into.
///
/// A setting whose value is read from the config snapshot on every request needs
/// no hook ([`Apply::Live`]); one that was copied into an object at startup does,
/// or it would be [`Apply::Restart`] instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hook {
    /// `AuthRateLimiters`: the per-endpoint attempt budgets.
    RateLimits,
    /// `TaskManager`: the concurrent copy/move cap.
    TaskManager,
    /// `NotificationManager`: the WebSocket connection caps.
    NotificationManager,
    /// The process-wide sync-protocol statics (`traversal`, FS-object check).
    SyncStatics,
    /// The outbound-mail drainer: its scheduler task is registered
    /// unconditionally and checks the live switch itself.
    MailDrain,
}

/// When a saved change takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    /// Effective for the next request.
    Live,
    /// Effective for the next request, after a hook pushes it into the object
    /// that captured it at startup.
    LiveWithHook(Hook),
    /// Saved now, applied at the next start.
    Restart,
    /// Never stored: the value can only come from the environment or the config
    /// file. Editing it would be either meaningless (the database cannot govern
    /// the database) or unrecoverable (a new `secret_key` makes existing
    /// ciphertext permanently unreadable).
    ReadOnly,
}

impl Hook {
    /// Every hook, for callers that want to re-apply all of them.
    pub const ALL: [Hook; 5] = [
        Hook::RateLimits,
        Hook::TaskManager,
        Hook::NotificationManager,
        Hook::SyncStatics,
        Hook::MailDrain,
    ];
}

impl Apply {
    pub const fn is_live(self) -> bool {
        matches!(self, Apply::Live | Apply::LiveWithHook(_))
    }

    pub const fn is_stored(self) -> bool {
        !matches!(self, Apply::ReadOnly)
    }
}

/// Where the effective value of one setting came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Environment {
        var: &'static str,
    },
    ConfigFile {
        key: &'static str,
    },
    Database {
        updated_at: i64,
        updated_by: Option<i32>,
    },
    Default,
}

impl Origin {
    /// Stable identifier for the template's origin badge.
    pub const fn id(self) -> &'static str {
        match self {
            Origin::Environment { .. } => "environment",
            Origin::ConfigFile { .. } => "config",
            Origin::Database { .. } => "database",
            Origin::Default => "default",
        }
    }
}

/// One stored value from the `settings` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingRow {
    pub value: String,
    pub updated_at: i64,
    pub updated_by: Option<i32>,
}

/// One catalog entry.
///
/// `get`/`set` are plain function pointers (non-capturing closures) so the
/// catalog stays a `static` and both directions are total for every field.
#[derive(Clone, Copy)]
pub struct SettingDef {
    /// `"auth.max_login_attempts"`, also the `settings.key` value.
    pub key: &'static str,
    pub section: Section,
    /// The `NANOFILE_*` variable that overrides this field.
    pub env: Option<&'static str>,
    /// An additional `*_FILE` variable holding the secret (preferred over `env`
    /// so the value does not reach a process listing).
    pub env_file: Option<&'static str>,
    pub kind: Kind,
    pub apply: Apply,
    pub get: fn(&Config) -> String,
    pub set: fn(&mut Config, &str) -> Result<(), String>,
}

impl SettingDef {
    /// A catalog entry is identified by its key and how it is applied; the
    /// function pointers are the same for every copy and add nothing.
    pub fn debug_label(&self) -> String {
        format!("{} ({:?})", self.key, self.apply)
    }

    /// The locale key of this setting's label.
    pub fn label_key(&self) -> String {
        label_key(self.key)
    }

    /// The locale key of this setting's help text.
    pub fn help_key(&self) -> String {
        format!("{}_help", label_key(self.key))
    }

    /// Whether a saved value for this key can be stored at all.
    pub const fn is_stored(&self) -> bool {
        self.apply.is_stored()
    }
}

/// `"auth.max_login_attempts"` → `"setting.auth_max_login_attempts"`.
pub fn label_key(key: &str) -> String {
    format!("setting.{}", key.replace('.', "_"))
}

impl std::fmt::Debug for SettingDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the accessors: a `{:?}` on a catalog entry is about which
        // setting it is, not about how it is read.
        f.debug_struct("SettingDef")
            .field("key", &self.key)
            .field("section", &self.section)
            .field("env", &self.env)
            .field("kind", &self.kind)
            .field("apply", &self.apply)
            .finish_non_exhaustive()
    }
}

/// Look up one entry by key.
pub fn find(key: &str) -> Option<&'static SettingDef> {
    CATALOG.iter().find(|def| def.key == key)
}

/// Every entry of a section, in declaration order.
pub fn section(section: Section) -> impl Iterator<Item = &'static SettingDef> {
    CATALOG.iter().filter(move |def| def.section == section)
}

/// How the config file participates in the layering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfigPolicy {
    /// The file seeds the database; a stored row wins (default).
    #[default]
    Bootstrap,
    /// The file wins over the database for every key.
    Override,
}

/// The `[settings]` section plus its environment overrides, resolved.
#[derive(Clone, Debug)]
pub struct SettingsPolicy {
    pub config_policy: ConfigPolicy,
    /// Keys the config file wins over the database for, even under
    /// [`ConfigPolicy::Bootstrap`].
    pub config_override_keys: BTreeSet<String>,
    /// Seconds between re-reads of the `settings` table (0 = only at startup and
    /// on this instance's own saves).
    pub refresh_interval_secs: u64,
}

impl Default for SettingsPolicy {
    fn default() -> Self {
        Self {
            config_policy: ConfigPolicy::Bootstrap,
            config_override_keys: BTreeSet::new(),
            refresh_interval_secs: 30,
        }
    }
}

impl SettingsPolicy {
    /// Resolve the policy from the loaded config file/env.
    ///
    /// An unknown `config_policy` falls back to `bootstrap` (never silently to
    /// `override`, which would make a database save do nothing) with a warning.
    pub fn from_config(config: &Config) -> Self {
        let raw = config.settings.config_policy.trim();
        let config_policy = match raw.to_ascii_lowercase().as_str() {
            "" | "bootstrap" => ConfigPolicy::Bootstrap,
            "override" => ConfigPolicy::Override,
            other => {
                tracing::warn!(
                    "settings.config_policy = '{other}' is not a policy; using 'bootstrap' \
                     (valid values: bootstrap, override)"
                );
                ConfigPolicy::Bootstrap
            }
        };
        let config_override_keys = config
            .settings
            .config_override_keys
            .iter()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect();
        Self {
            config_policy,
            config_override_keys,
            refresh_interval_secs: config.settings.refresh_interval_secs,
        }
    }

    /// Whether the config-file value beats a stored row for this key.
    pub fn config_wins(&self, def: &SettingDef) -> bool {
        matches!(self.config_policy, ConfigPolicy::Override)
            || self.config_override_keys.contains(def.key)
    }

    /// Whether a config-file entry that disagrees with a stored row is being
    /// ignored, which is worth telling the operator about.
    ///
    /// Under [`ConfigPolicy::Override`] the file wins, so there is nothing to
    /// report — that is the point of the setting.
    pub fn should_report_drift(&self) -> bool {
        matches!(self.config_policy, ConfigPolicy::Bootstrap)
    }
}

/// One setting with its effective value and where that value came from.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub def: &'static SettingDef,
    /// Canonical string form (for [`Kind::Secret`] this is the *stored ciphertext*
    /// when the database supplied it, and the plaintext when only the
    /// environment or the file supplied it — callers that render HTML must never
    /// print it at all).
    pub value: String,
    pub origin: Origin,
    /// A stored secret that cannot be decrypted any more (the `secret_key`
    /// changed). Delivery and login must not proceed as if it were absent.
    pub broken_secret: bool,
}

/// Decide the effective value of one setting.
///
/// `base` is the config file **with the environment already applied** and
/// `env_keys` names the catalog keys that came from the environment; `defaults`
/// is a `Config::default()` used to tell a configured value from a built-in one.
pub fn resolve(
    def: &'static SettingDef,
    base: &Config,
    defaults: &Config,
    env_keys: &BTreeSet<&'static str>,
    rows: &BTreeMap<String, SettingRow>,
    policy: &SettingsPolicy,
) -> Resolved {
    let base_value = (def.get)(base);
    let default_value = (def.get)(defaults);

    if let Some(var) = def.env
        && env_keys.contains(def.key)
    {
        return Resolved {
            def,
            value: base_value,
            origin: Origin::Environment { var },
            broken_secret: false,
        };
    }

    if def.is_stored()
        && !policy.config_wins(def)
        && let Some(row) = rows.get(def.key)
    {
        return Resolved {
            def,
            value: row.value.clone(),
            origin: Origin::Database {
                updated_at: row.updated_at,
                updated_by: row.updated_by,
            },
            broken_secret: false,
        };
    }

    let origin = if base_value != default_value {
        Origin::ConfigFile { key: def.key }
    } else if policy.config_wins(def) {
        // The file's value happens to equal the built-in default, but the file
        // is what is in charge for this key — say so, because that is why a
        // stored row (if any) is being ignored.
        Origin::ConfigFile { key: def.key }
    } else {
        Origin::Default
    };
    Resolved {
        def,
        value: base_value,
        origin,
        broken_secret: false,
    }
}

/// Decide the effective value of every catalog entry.
pub fn resolve_all(
    base: &Config,
    defaults: &Config,
    env_keys: &BTreeSet<&'static str>,
    rows: &BTreeMap<String, SettingRow>,
    policy: &SettingsPolicy,
) -> Vec<Resolved> {
    CATALOG
        .iter()
        .map(|def| resolve(def, base, defaults, env_keys, rows, policy))
        .collect()
}

/// Write resolved values into `config`.
///
/// `live_only` skips [`Apply::Restart`] and [`Apply::ReadOnly`] entries, which is
/// how the running snapshot keeps the startup value of a restart-only setting
/// while the database row waits for the next start.
///
/// Returns a description of every value that could not be applied. That is
/// reported rather than fatal: a row written by a newer build (or edited by
/// hand) must not stop the process from starting, and the admin page has to stay
/// reachable to fix it.
pub fn apply_resolved(config: &mut Config, resolved: &[Resolved], live_only: bool) -> Vec<String> {
    let mut failures = Vec::new();
    for entry in resolved {
        if live_only && !entry.def.apply.is_live() {
            continue;
        }
        if let Err(e) = (entry.def.set)(config, &entry.value) {
            failures.push(format!("{} = {:?}: {e}", entry.def.key, entry.value));
        }
    }
    failures
}

// ─── Value parsing / formatting ─────────────────────────────────────────────

/// Canonical string form of a boolean: `"true"` / `"false"`.
pub fn fmt_bool(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// Canonical string form of an optional boolean (empty = unset).
pub fn fmt_opt_bool(value: Option<bool>) -> String {
    value.map(fmt_bool).unwrap_or_default()
}

/// Parse a boolean, accepting the spellings a config file or environment
/// variable plausibly uses. Anything else is an error rather than a silent
/// `false`: a typo must not quietly switch a protection off.
pub fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(format!("not a boolean: {other:?} (use true or false)")),
    }
}

/// Parse an optional boolean; an empty value (or one of `unset`/`auto`/`none`)
/// means "not configured".
pub fn parse_opt_bool(value: &str) -> Result<Option<bool>, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "unset" | "auto" | "none" | "default" => Ok(None),
        _ => parse_bool(value).map(Some),
    }
}

/// Parse an unsigned integer.
pub fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("not a whole number: {:?}", value.trim()))
}

/// Parse an unsigned 32-bit integer.
pub fn parse_u32(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("not a whole number: {:?}", value.trim()))
}

/// Parse an unsigned 16-bit integer.
pub fn parse_u16(value: &str) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("not a port number: {:?}", value.trim()))
}

/// Parse a signed 32-bit integer.
pub fn parse_i32(value: &str) -> Result<i32, String> {
    value
        .trim()
        .parse::<i32>()
        .map_err(|_| format!("not a whole number: {:?}", value.trim()))
}

/// Parse a `usize`.
pub fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("not a whole number: {:?}", value.trim()))
}

/// Canonical form of an optional string: the trimmed value, or `None` for empty.
pub fn opt_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Split a comma-separated text list, trimming entries and dropping empties.
pub fn parse_text_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// Canonical form of a text list.
pub fn fmt_text_list(values: &[String]) -> String {
    values.join(",")
}

/// Parse a comma-separated list of whole numbers. An unparsable entry is
/// dropped rather than failing the whole list, matching how the environment
/// layer has always treated these presets.
pub fn parse_num_list(value: &str) -> Vec<u64> {
    value
        .split(',')
        .filter_map(|entry| entry.trim().parse::<u64>().ok())
        .collect()
}

/// Canonical form of a numeric list.
pub fn fmt_num_list(values: &[u64]) -> String {
    values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse one of a fixed set of identifiers, case-insensitively.
pub fn parse_enum(value: &str, allowed: &[&str]) -> Result<String, String> {
    let trimmed = value.trim();
    allowed
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(trimmed))
        .map(|candidate| (*candidate).to_string())
        .ok_or_else(|| {
            format!(
                "unknown value {trimmed:?}; use one of {}",
                allowed.join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_with_env(key: &'static str) -> (Config, BTreeSet<&'static str>) {
        let mut config = Config::default();
        let def = find(key).expect("catalog entry");
        (def.set)(&mut config, "false").ok();
        let mut env_keys = BTreeSet::new();
        env_keys.insert(key);
        (config, env_keys)
    }

    #[test]
    fn every_key_is_unique_and_carries_an_env_name() {
        let mut seen = BTreeSet::new();
        for def in CATALOG {
            assert!(seen.insert(def.key), "duplicate catalog key {}", def.key);
            let env = def.env.expect("every setting has an environment variable");
            assert!(env.starts_with("NANOFILE_"), "{}: bad env {env}", def.key);
            assert!(
                env.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{env} must be upper-case",
            );
        }
    }

    #[test]
    fn every_declared_config_field_is_in_the_catalog() {
        // The list is written out rather than derived so a field added to
        // `Config` without a catalog entry makes this test fail instead of
        // silently producing a settings page that cannot show it.
        let expected: BTreeSet<&str> = [
            "server.addr",
            "server.port",
            "server.version",
            "server.site_url",
            "server.max_upload_size_mb",
            "server.max_json_body_mb",
            "server.max_chunk_size_mb",
            "server.request_timeout_secs",
            "server.header_read_timeout_secs",
            "server.body_timeout_secs",
            "server.cors_allowed_origins",
            "server.secret_key",
            "server.cors_max_age_secs",
            "server.webdav_enabled",
            "server.sso_enabled",
            "server.desktop_custom_brand",
            "server.desktop_custom_logo",
            "server.encrypted_library_version",
            "server.encrypted_library_pwd_hash_algo",
            "server.encrypted_library_pwd_hash_params",
            "server.file_search_enabled",
            "server.share_link_enabled",
            "server.trusted_proxies",
            "server.allowed_hosts",
            "server.trust_request_host",
            "server.max_propfind_entries",
            "server.hsts_include_subdomains",
            "server.tray",
            "database.url",
            "database.max_connections",
            "storage.block_dir",
            "storage.temp_dir",
            "storage.max_storage_bytes",
            "storage.ffmpeg_path",
            "storage.max_temp_uploads",
            "storage.max_temp_upload_bytes",
            "storage.temp_upload_ttl_hours",
            "storage.max_zip_entries",
            "storage.max_zip_bytes",
            "storage.thumbnail_dir",
            "storage.avatar_dir",
            "storage.block_encryption_mode",
            "storage.encryption_key",
            "auth.password_hash_iterations",
            "auth.api_token_ttl_days",
            "auth.sync_token_ttl_days",
            "auth.max_login_attempts",
            "auth.lockout_duration_secs",
            "auth.max_distinct_usernames_per_ip",
            "auth.sso_link_max_per_hour",
            "auth.enable_invitations",
            "auth.enable_password_reset",
            "auth.password_min_length",
            "auth.require_strong_password",
            "auth.password_reset_max_per_hour",
            "auth.registration_max_per_hour",
            "auth.totp_max_attempts",
            "auth.link_password_max_per_hour",
            "auth.repo_password_max_per_hour",
            "auth.share_download_max_per_minute",
            "auth.webdav_max_failures_per_5min",
            "auth.reindex_max_per_hour",
            "auth.search_max_per_minute",
            "auth.api_key_ttl_presets_days",
            "auth.api_key_max_ttl_days",
            "logging.level",
            "logging.file_enabled",
            "logging.file",
            "logging.max_file_size_mb",
            "logging.max_backups",
            "gc.enabled",
            "gc.interval_hours",
            "index.enabled",
            "index.index_dir",
            "notification.enabled",
            "notification.private_key",
            "notification.ping_interval",
            "notification.client_timeout",
            "notification.max_connections",
            "notification.max_connections_per_ip",
            "notification.subscribe_timeout_secs",
            "notification.accept_legacy_event_tokens",
            "tasks.max_active_tasks",
            "admin_init.email",
            "admin_init.password",
            "email.enabled",
            "email.host",
            "email.port",
            "email.tls",
            "email.username",
            "email.password",
            "email.from_address",
            "email.from_name",
            "email.timeout_secs",
            "email.max_attempts",
            "email.paused",
            "email.notify_new_device",
            "email.notify_api_key_created",
            "email.notify_new_login",
            "ui.default_language",
            "ui.tray_language",
            "sync.verify_fs_objects",
            "sync.max_tree_depth",
            "sync.max_tree_visits",
        ]
        .into_iter()
        .collect();
        let actual: BTreeSet<&str> = CATALOG.iter().map(|def| def.key).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn every_kind_round_trips_through_get_and_set() {
        // A sample per kind; the point is that `set` accepts what `get` produces
        // and that the round trip is stable for every catalog entry, so the
        // admin form can submit exactly what the page rendered.
        for def in CATALOG {
            let mut config = Config::default();
            let sample: &str = match def.kind {
                Kind::Bool => "true",
                Kind::U16 => "8082",
                Kind::U32 => "7",
                Kind::U64 => "7",
                Kind::I32 => "4",
                Kind::Usize => "7",
                Kind::Text | Kind::OptText | Kind::Secret | Kind::Path => "sample",
                Kind::OptBool => "false",
                Kind::Enum(allowed) => allowed[0],
                Kind::TextList => "a,b",
                Kind::NumList => "7,30",
            };
            (def.set)(&mut config, sample).unwrap_or_else(|e| {
                panic!("{}: set({sample:?}) failed: {e}", def.key);
            });
            let rendered = (def.get)(&config);
            let mut again = Config::default();
            (def.set)(&mut again, &rendered).unwrap_or_else(|e| {
                panic!("{}: re-set({rendered:?}) failed: {e}", def.key);
            });
            assert_eq!(
                (def.get)(&again),
                rendered,
                "{}: get/set is not stable",
                def.key
            );
        }
    }

    #[test]
    fn read_only_entries_are_never_stored() {
        for key in [
            "server.secret_key",
            "storage.encryption_key",
            "storage.block_encryption_mode",
            "database.url",
            "database.max_connections",
            "admin_init.email",
            "admin_init.password",
            "logging.file",
            "logging.file_enabled",
        ] {
            let def = find(key).expect(key);
            assert_eq!(def.apply, Apply::ReadOnly, "{key} must be read-only");
        }
    }

    #[test]
    fn live_entries_are_the_majority() {
        let live = CATALOG.iter().filter(|def| def.apply.is_live()).count();
        assert!(
            live * 2 > CATALOG.len(),
            "most settings must take effect without a restart: {live}/{}",
            CATALOG.len()
        );
    }

    #[test]
    fn booleans_accept_the_usual_spellings_and_reject_typos() {
        for (value, expected) in [
            ("true", true),
            ("TRUE", true),
            ("1", true),
            ("on", true),
            ("yes", true),
            ("false", false),
            ("0", false),
            ("off", false),
            ("no", false),
        ] {
            assert_eq!(parse_bool(value), Ok(expected), "{value}");
        }
        assert!(parse_bool("maybe").is_err());
        assert!(parse_bool("").is_err());
        assert_eq!(parse_opt_bool(""), Ok(None));
        assert_eq!(parse_opt_bool("auto"), Ok(None));
        assert_eq!(parse_opt_bool("on"), Ok(Some(true)));
        assert!(parse_opt_bool("sometimes").is_err());
    }

    #[test]
    fn lists_drop_empty_entries_and_unparsable_numbers() {
        assert_eq!(parse_text_list(" a , b ,, c "), vec!["a", "b", "c"]);
        assert_eq!(fmt_text_list(&["a".into(), "b".into()]), "a,b");
        assert_eq!(parse_num_list("7, 30 ,x,365"), vec![7, 30, 365]);
        assert_eq!(fmt_num_list(&[7, 30]), "7,30");
    }

    #[test]
    fn enum_parsing_is_case_insensitive_and_canonicalising() {
        let allowed = ["starttls", "tls", "none"];
        assert_eq!(parse_enum("STARTTLS", &allowed), Ok("starttls".to_string()));
        assert!(parse_enum("sometimes", &allowed).is_err());
    }

    #[test]
    fn policy_defaults_to_bootstrap_and_can_be_overridden() {
        let policy = SettingsPolicy::from_config(&Config::default());
        assert_eq!(policy.config_policy, ConfigPolicy::Bootstrap);
        assert!(policy.config_override_keys.is_empty());

        let mut config = Config::default();
        config.settings.config_policy = "override".to_string();
        config.settings.config_override_keys =
            vec!["server.site_url".to_string(), "  ".to_string()];
        let policy = SettingsPolicy::from_config(&config);
        assert_eq!(policy.config_policy, ConfigPolicy::Override);
        assert!(policy.config_wins(find("server.site_url").unwrap()));
        assert!(policy.config_wins(find("server.port").unwrap()));

        // A typo must not silently make saves useless.
        let mut config = Config::default();
        config.settings.config_policy = "database".to_string();
        assert_eq!(
            SettingsPolicy::from_config(&config).config_policy,
            ConfigPolicy::Bootstrap
        );
    }

    #[test]
    fn resolution_prefers_environment_then_database_then_file_then_default() {
        let defaults = Config::default();
        let policy = SettingsPolicy::default();
        let def = find("server.share_link_enabled").unwrap();

        // Default.
        let resolved = resolve(
            def,
            &defaults,
            &defaults,
            &BTreeSet::new(),
            &BTreeMap::new(),
            &policy,
        );
        assert_eq!(resolved.origin, Origin::Default);
        assert_eq!(resolved.value, "true");

        // Config file value (no row).
        let mut file = Config::default();
        file.server.share_link_enabled = false;
        let resolved = resolve(
            def,
            &file,
            &defaults,
            &BTreeSet::new(),
            &BTreeMap::new(),
            &policy,
        );
        assert_eq!(resolved.origin, Origin::ConfigFile { key: def.key });
        assert_eq!(resolved.value, "false");

        // Database wins over the file.
        let mut rows = BTreeMap::new();
        rows.insert(
            def.key.to_string(),
            SettingRow {
                value: "false".to_string(),
                updated_at: 7,
                updated_by: Some(3),
            },
        );
        let resolved = resolve(def, &defaults, &defaults, &BTreeSet::new(), &rows, &policy);
        assert_eq!(
            resolved.origin,
            Origin::Database {
                updated_at: 7,
                updated_by: Some(3)
            }
        );
        assert_eq!(resolved.value, "false");

        // The environment wins over everything.
        let (env_config, env_keys) = base_with_env("server.share_link_enabled");
        let resolved = resolve(def, &env_config, &defaults, &env_keys, &rows, &policy);
        assert_eq!(
            resolved.origin,
            Origin::Environment {
                var: "NANOFILE_SERVER_SHARE_LINK_ENABLED"
            }
        );
        assert_eq!(resolved.value, "false");

        // ...unless the key is simply not set in the environment.
        let resolved = resolve(def, &defaults, &defaults, &BTreeSet::new(), &rows, &policy);
        assert_eq!(resolved.origin.id(), "database");
    }

    #[test]
    fn a_config_override_key_beats_the_database() {
        let def = find("server.share_link_enabled").unwrap();
        let defaults = Config::default();
        let mut file = Config::default();
        file.server.share_link_enabled = true;
        let mut rows = BTreeMap::new();
        rows.insert(
            def.key.to_string(),
            SettingRow {
                value: "false".to_string(),
                updated_at: 7,
                updated_by: None,
            },
        );

        let mut policy = SettingsPolicy::default();
        policy
            .config_override_keys
            .insert("server.share_link_enabled".to_string());
        let resolved = resolve(def, &file, &defaults, &BTreeSet::new(), &rows, &policy);
        assert_eq!(resolved.origin.id(), "config");
        assert_eq!(resolved.value, "true");
    }

    #[test]
    fn a_read_only_entry_never_uses_a_stored_row() {
        let def = find("server.secret_key").unwrap();
        let defaults = Config::default();
        let mut rows = BTreeMap::new();
        rows.insert(
            def.key.to_string(),
            SettingRow {
                value: "from-the-database".to_string(),
                updated_at: 1,
                updated_by: None,
            },
        );
        let resolved = resolve(
            def,
            &defaults,
            &defaults,
            &BTreeSet::new(),
            &rows,
            &SettingsPolicy::default(),
        );
        assert_eq!(resolved.origin, Origin::Default);
    }

    #[test]
    fn applying_live_only_keeps_restart_values_at_their_startup_state() {
        let defaults = Config::default();
        let policy = SettingsPolicy::default();
        let mut rows = BTreeMap::new();
        // A live key and a restart key, both stored.
        rows.insert(
            "server.share_link_enabled".to_string(),
            SettingRow {
                value: "false".to_string(),
                updated_at: 1,
                updated_by: None,
            },
        );
        rows.insert(
            "server.max_json_body_mb".to_string(),
            SettingRow {
                value: "8".to_string(),
                updated_at: 1,
                updated_by: None,
            },
        );

        let mut config = defaults.clone();
        let resolved = resolve_all(&defaults, &defaults, &BTreeSet::new(), &rows, &policy);
        assert!(apply_resolved(&mut config, &resolved, true).is_empty());
        assert!(!config.server.share_link_enabled, "live key applied");
        assert_eq!(
            config.server.max_json_body_mb, 64,
            "restart key keeps its startup value"
        );
    }

    #[test]
    fn label_keys_are_flat_and_section_ids_round_trip() {
        assert_eq!(
            label_key("auth.max_login_attempts"),
            "setting.auth_max_login_attempts"
        );
        for section in Section::ALL {
            assert_eq!(Section::from_id(section.id()), Some(section));
        }
        assert_eq!(Section::from_id("nope"), None);
    }
}
