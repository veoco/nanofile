//! The layered settings service.
//!
//! It owns the saved rows, computes the effective value of every catalog key
//! (and where it came from), keeps the [`RuntimeConfig`] snapshot in step, and
//! persists an administrator's changes.
//!
//! A save parses and validates the whole candidate against the very rules the
//! server starts with *before* writing, so a rejected save leaves the database
//! exactly as it was; the live snapshot is then recomputed and the hooks that
//! have to be pushed are reported.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use base::error::AppError;
use infra::config::Config;
use infra::config::EnvKeys;
use infra::crypto::token_encryption::TokenCipher;
use infra::settings::{
    self, Apply, Hook, Kind, Origin, Resolved, Section, SettingDef, SettingRow, SettingsPolicy,
    apply_resolved,
};

use crate::repository::settings::SettingsRepository;
use crate::settings::runtime::RuntimeConfig;

pub struct SettingsLayers {
    /// The config file with the environment already applied.
    pub base: Config,
    pub env_keys: EnvKeys,
    pub rows: BTreeMap<String, SettingRow>,
    pub policy: SettingsPolicy,
}

impl SettingsLayers {
    /// The config file is the only layer (tests, CLI subcommands, callers without a database).
    pub fn plain(config: &Config) -> Self {
        Self {
            base: config.clone(),
            env_keys: EnvKeys::new(),
            rows: BTreeMap::new(),
            policy: SettingsPolicy::from_config(config),
        }
    }

    pub async fn load(
        repo: &dyn SettingsRepository,
        config: &Config,
        env_keys: EnvKeys,
    ) -> Result<Self, AppError> {
        let mut rows = repo.load_all().await?;
        normalize_paths(&mut rows);
        Ok(Self {
            base: config.clone(),
            env_keys,
            rows,
            policy: SettingsPolicy::from_config(config),
        })
    }
}

/// A submitted settings form: non-secret values as text, each secret as
/// `Some(value)` to replace, `Some("")` to erase, or `None` to keep what is
/// stored (a password field cannot tell an empty box from an absent one).
#[derive(Clone, Debug, Default)]
pub struct SettingsForm {
    pub values: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, Option<String>>,
}

/// What a save (or a reload) changed.
#[derive(Clone, Debug, Default)]
pub struct SaveOutcome {
    pub changed: Vec<String>,
    /// Changed keys that only take effect at the next start.
    pub restart_pending: Vec<String>,
    /// Long-lived objects a hook must be pushed into.
    pub hooks: BTreeSet<Hook>,
}

impl SaveOutcome {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }
}

/// Whether a write-only secret is configured, and from where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretState {
    Unset,
    FromEnvironment,
    FromConfigFile,
    Stored,
    /// A stored value that can no longer be decrypted (the `secret_key` changed): effectively unset.
    StoredUnreadable,
}

pub struct SettingsService {
    repo: Arc<dyn SettingsRepository>,
    cipher: Arc<TokenCipher>,
    runtime: RuntimeConfig,
    /// The config file with the environment applied: the bootstrap layer.
    base: Config,
    /// Built-in defaults, used to tell a configured value from an unset one.
    defaults: Config,
    /// `base` with **every** stored value applied: what this process started
    /// with, and the authority for restart-only settings.
    startup: Config,
    env_keys: EnvKeys,
    policy: SettingsPolicy,
    rows: RwLock<BTreeMap<String, SettingRow>>,
    broken_secrets: RwLock<BTreeSet<String>>,
    /// The effective value of each key as last applied, so a reload can tell what changed.
    applied: RwLock<BTreeMap<String, String>>,
}

/// The configuration a process starts with: every layer applied. Also returns
/// the unreadable secrets, so the caller can report them before the first
/// request. It is separate from [`SettingsService::new`], so the startup path can
/// read the effective values before any object is built from them.
pub fn resolve_startup(
    layers: &SettingsLayers,
    cipher: &TokenCipher,
) -> (Config, BTreeSet<String>) {
    let defaults = Config::default();
    let (resolved, broken) = resolve_effective(
        &layers.base,
        &defaults,
        &layers.env_keys,
        &layers.rows,
        &layers.policy,
        cipher,
    );

    let mut startup = layers.base.clone();
    for failure in apply_resolved(&mut startup, &resolved, false) {
        // Never fatal: a value that cannot be applied (a row written by a newer
        // build, or edited by hand) is reported and skipped, so the server still
        // starts and the page stays reachable to fix it.
        tracing::error!("saved setting could not be applied: {failure}");
    }
    (startup, broken)
}

impl SettingsService {
    pub fn new(
        repo: Arc<dyn SettingsRepository>,
        cipher: Arc<TokenCipher>,
        layers: SettingsLayers,
    ) -> Self {
        let defaults = Config::default();
        let (startup, broken) = resolve_startup(&layers, &cipher);
        let resolved = settings::resolve_all(
            &layers.base,
            &defaults,
            &layers.env_keys,
            &layers.rows,
            &layers.policy,
        );

        let runtime = RuntimeConfig::new(startup.clone());
        let applied = resolved
            .iter()
            .map(|entry| (entry.def.key.to_string(), entry.value.clone()))
            .collect();

        Self {
            repo,
            cipher,
            runtime,
            base: layers.base,
            defaults,
            startup,
            env_keys: layers.env_keys,
            policy: layers.policy,
            rows: RwLock::new(layers.rows),
            broken_secrets: RwLock::new(broken),
            applied: RwLock::new(applied),
        }
    }

    pub fn runtime(&self) -> &RuntimeConfig {
        &self.runtime
    }

    /// The configuration this process started with (all layers applied).
    pub fn startup(&self) -> &Config {
        &self.startup
    }

    pub fn base(&self) -> &Config {
        &self.base
    }

    pub fn policy(&self) -> &SettingsPolicy {
        &self.policy
    }

    pub fn refresh_interval_secs(&self) -> u64 {
        self.policy.refresh_interval_secs
    }

    pub fn row(&self, key: &str) -> Option<SettingRow> {
        self.rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    pub fn stored_keys(&self) -> BTreeSet<String> {
        self.rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// Every catalog key with its effective value and origin. Secret values stay
    /// in their stored (ciphertext) form: this output feeds HTML, and a page
    /// must never be able to print a secret back.
    pub fn resolved(&self) -> Vec<Resolved> {
        let rows = self
            .rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        settings::resolve_all(
            &self.base,
            &self.defaults,
            &self.env_keys,
            &rows,
            &self.policy,
        )
    }

    pub fn resolved_for(&self, section: Section) -> Vec<Resolved> {
        self.resolved()
            .into_iter()
            .filter(|entry| entry.def.section == section)
            .collect()
    }

    pub fn resolved_one(&self, key: &str) -> Option<Resolved> {
        self.resolved()
            .into_iter()
            .find(|entry| entry.def.key == key)
    }

    pub fn secret_state(&self, key: &str) -> Option<SecretState> {
        let entry = self.resolved_one(key)?;
        if entry.def.kind != Kind::Secret {
            return None;
        }
        if self
            .broken_secrets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(key)
        {
            return Some(SecretState::StoredUnreadable);
        }
        Some(match entry.origin {
            Origin::Database { .. } => SecretState::Stored,
            Origin::Environment { .. } => SecretState::FromEnvironment,
            Origin::ConfigFile { .. } => SecretState::FromConfigFile,
            Origin::Default => SecretState::Unset,
        })
    }

    /// Keys whose saved value only takes effect at the next start: a
    /// restart-only setting's effective (stored) value differs from the value
    /// the running process started with. The in-place "Restart server" button
    /// applies these; [`Self::pending_process_restart`] holds the rest.
    pub fn pending_restart(&self) -> BTreeSet<String> {
        let live = self.runtime.get();
        self.resolved()
            .into_iter()
            .filter(|entry| entry.def.apply.is_in_place_restart())
            .filter(|entry| entry.value != (entry.def.get)(&live))
            .map(|entry| entry.def.key.to_string())
            .collect()
    }

    /// Keys whose saved value needs the *operating-system process* restarted.
    ///
    /// Read before the server loop exists (the log subscriber, the desktop
    /// tray), so the in-app restart cannot reach them.
    pub fn pending_process_restart(&self) -> BTreeSet<String> {
        let live = self.runtime.get();
        self.resolved()
            .into_iter()
            .filter(|entry| entry.def.apply == Apply::ProcessRestart)
            .filter(|entry| entry.value != (entry.def.get)(&live))
            .map(|entry| entry.def.key.to_string())
            .collect()
    }

    /// Re-read the settings table, returning what changed, if anything. Called
    /// periodically so a change made on another instance (or a restored backup)
    /// is picked up. The environment and config file are *not* re-read:
    /// process-level inputs must not change under a running process.
    pub async fn reload(&self) -> Result<SaveOutcome, AppError> {
        let mut fresh = self.repo.load_all().await?;
        normalize_paths(&mut fresh);
        {
            let current = self
                .rows
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *current == fresh {
                return Ok(SaveOutcome::default());
            }
        }
        *self
            .rows
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = fresh;
        Ok(self.reapply())
    }

    pub async fn save(
        &self,
        section: Section,
        form: &SettingsForm,
        updated_by: Option<i32>,
    ) -> Result<SaveOutcome, AppError> {
        let now = chrono::Utc::now().timestamp();

        // Start from the fully-layered configuration, so a cross-field rule can
        // be checked against what the server would actually use.
        let mut candidate = self.layered_config();
        let mut writes: BTreeMap<String, String> = BTreeMap::new();
        let mut clears: Vec<String> = Vec::new();

        for (key, raw) in &form.values {
            let def = self.stored_def(key, section)?;
            (def.set)(&mut candidate, raw)
                .map_err(|e| AppError::BadRequest(format!("{key}: {e}")))?;
            // Store the canonical spelling, so the database holds exactly what
            // the page would render and no parse can differ later.
            writes.insert(key.clone(), (def.get)(&candidate));
        }

        for (key, choice) in &form.secrets {
            let def = self.stored_def(key, section)?;
            if def.kind != Kind::Secret {
                return Err(AppError::BadRequest(format!(
                    "{key} is not a secret; submit it as a value"
                )));
            }
            match choice {
                // The field was rendered but not filled in: keep what is stored.
                None => {}
                Some(value) if value.trim().is_empty() => clears.push(key.clone()),
                Some(value) => {
                    (def.set)(&mut candidate, value)
                        .map_err(|e| AppError::BadRequest(format!("{key}: {e}")))?;
                    writes.insert(key.clone(), self.cipher.encrypt(value));
                }
            }
        }

        if writes.is_empty() && clears.is_empty() {
            return Ok(SaveOutcome::default());
        }
        for (key, value) in writes.iter_mut() {
            if is_path_key(key) {
                *value = normalize_path_value(value);
            }
        }

        self.validate(&candidate)?;

        if !writes.is_empty() {
            self.repo.upsert_many(&writes, updated_by, now).await?;
        }
        if !clears.is_empty() {
            self.repo.delete_keys(&clears).await?;
        }

        {
            let mut rows = self
                .rows
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (key, value) in &writes {
                rows.insert(
                    key.clone(),
                    SettingRow {
                        value: value.clone(),
                        updated_at: now,
                        updated_by,
                    },
                );
            }
            for key in &clears {
                rows.remove(key);
            }
        }

        let mut outcome = self.reapply();
        // The change list is what the form submitted (in catalog order), not the
        // whole effective diff: clearing a key that had no row changed nothing.
        let mut changed: Vec<String> = writes
            .keys()
            .chain(clears.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        changed.sort();
        outcome.changed = changed;
        outcome.restart_pending = outcome
            .changed
            .iter()
            .filter(|key| settings::find(key).is_some_and(|def| def.apply.is_restart_only()))
            .cloned()
            .collect();
        Ok(outcome)
    }

    /// Drop the stored override for `keys`, so the config file (or the built-in
    /// default) supplies them again.
    pub async fn clear(&self, keys: &[String]) -> Result<SaveOutcome, AppError> {
        let mut targets = Vec::new();
        for key in keys {
            let def = settings::find(key)
                .filter(|def| def.is_stored())
                .ok_or_else(|| AppError::BadRequest(format!("{key} is not a stored setting")))?;
            targets.push(def.key.to_string());
        }
        if targets.is_empty() {
            return Ok(SaveOutcome::default());
        }
        self.repo.delete_keys(&targets).await?;
        {
            let mut rows = self
                .rows
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for key in &targets {
                rows.remove(key);
            }
        }
        let mut outcome = self.reapply();
        outcome.changed = targets;
        Ok(outcome)
    }

    /// Report the layering at startup: values the config file no longer supplies,
    /// changes still needing a restart, and stored rows this build does not know.
    pub fn log_startup_diagnostics(&self) {
        let resolved = self.resolved();
        let stored = self.stored_keys();

        // A row for a key the catalog lacks (a downgrade, or a key renamed by hand) is never applied.
        let unknown: Vec<&str> = stored
            .iter()
            .filter(|key| settings::find(key).is_none())
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            tracing::warn!(
                keys = ?unknown,
                "the settings table holds rows for settings this build does not know; they \
                 are ignored"
            );
        }

        // Under `bootstrap` a stored value wins, so a disagreeing config-file
        // entry is no longer in effect; an operator who edited the file needs
        // to be told which keys those are.
        if self.policy.should_report_drift() {
            let drifted: Vec<&str> = resolved
                .iter()
                .filter(|entry| matches!(entry.origin, Origin::Database { .. }))
                .filter(|entry| (entry.def.get)(&self.base) != (entry.def.get)(&self.defaults))
                .map(|entry| entry.def.key)
                .collect();
            if !drifted.is_empty() {
                tracing::info!(
                    keys = ?drifted,
                    "config file values for these settings are superseded by saved values; \
                     change them at /sysadmin/settings/ (or list them in \
                     [settings] config_override_keys to make the file win again)"
                );
            }
        }

        let pending = self.pending_restart();
        if !pending.is_empty() {
            tracing::info!(
                keys = ?pending.iter().map(String::as_str).collect::<Vec<_>>(),
                "saved settings will take effect at the next restart"
            );
        }

        // The values read before the server loop exists cannot be reached by
        // that restart at all, so they get their own line.
        let process_pending = self.pending_process_restart();
        if !process_pending.is_empty() {
            tracing::warn!(
                keys = ?process_pending.iter().map(String::as_str).collect::<Vec<_>>(),
                "saved settings only take effect after the process is restarted \
                 (the log subscriber and the desktop tray are set up before the server loop)"
            );
        }

        let broken = self
            .broken_secrets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !broken.is_empty() {
            tracing::warn!(
                keys = ?broken.iter().map(String::as_str).collect::<Vec<_>>(),
                "saved secrets cannot be decrypted any more (the server secret_key changed); \
                 re-enter them at /sysadmin/settings/"
            );
        }
    }

    /// The fully-layered configuration (every key, including restart-only ones).
    fn layered_config(&self) -> Config {
        let rows = self
            .rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (resolved, _) = resolve_effective(
            &self.base,
            &self.defaults,
            &self.env_keys,
            &rows,
            &self.policy,
            &self.cipher,
        );
        let mut config = self.base.clone();
        for failure in apply_resolved(&mut config, &resolved, false) {
            tracing::error!("setting could not be applied: {failure}");
        }
        config
    }

    fn reapply(&self) -> SaveOutcome {
        let rows = self
            .rows
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (resolved, broken) = resolve_effective(
            &self.base,
            &self.defaults,
            &self.env_keys,
            &rows,
            &self.policy,
            &self.cipher,
        );
        *self
            .broken_secrets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = broken;

        // Which keys actually changed value, so a hook only runs for those.
        let previous = self
            .applied
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut changed = Vec::new();
        let mut hooks = BTreeSet::new();
        for entry in &resolved {
            if previous.get(entry.def.key) != Some(&entry.value) {
                changed.push(entry.def.key.to_string());
                if let Apply::LiveWithHook(hook) = entry.def.apply {
                    hooks.insert(hook);
                }
            }
        }

        // The live snapshot starts from `startup` (so restart-only settings keep
        // the value the process started with), with every live key overwritten.
        let mut live = self.startup.clone();
        for failure in apply_resolved(&mut live, &resolved, true) {
            tracing::error!("live setting could not be applied: {failure}");
        }
        self.runtime.replace(live);

        *self
            .applied
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = resolved
            .iter()
            .map(|entry| (entry.def.key.to_string(), entry.value.clone()))
            .collect();

        SaveOutcome {
            changed,
            restart_pending: Vec::new(),
            hooks,
        }
    }

    fn stored_def(&self, key: &str, section: Section) -> Result<&'static SettingDef, AppError> {
        let def = settings::find(key)
            .ok_or_else(|| AppError::BadRequest(format!("{key} is not a setting")))?;
        if def.section != section {
            return Err(AppError::BadRequest(format!(
                "{key} does not belong to this page"
            )));
        }
        if !def.is_stored() {
            return Err(AppError::BadRequest(format!(
                "{key} can only be set in config.toml or the environment"
            )));
        }
        Ok(def)
    }

    /// The rules the server starts with, so a saved value cannot be one the next start would refuse.
    fn validate(&self, candidate: &Config) -> Result<(), AppError> {
        candidate.validate().map_err(AppError::BadRequest)?;

        // Server-only checks: the SMTP sender must be a real address, parsed the
        // same way the message builder parses it.
        if !candidate.email.from_address.trim().is_empty()
            && !crate::service::mail::settings::is_valid_address(&candidate.email.from_address)
        {
            return Err(AppError::BadRequest(format!(
                "email.from_address: not a valid email address: {}",
                candidate.email.from_address
            )));
        }
        for address in [
            &candidate.email.host,
            &candidate.email.username,
            &candidate.email.from_name,
        ] {
            if address.contains(['\r', '\n']) {
                return Err(AppError::BadRequest(
                    "an email header value must not contain a line break".to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn is_path_key(key: &str) -> bool {
    settings::find(key).is_some_and(|def| def.kind == Kind::Path)
}

/// Resolve a relative path against the installation directory.
///
/// [`Config::resolve_state_paths`] does this for the config file at startup; a
/// saved path must follow the same rule, or the setting names a different
/// directory depending on where the process starts.
fn normalize_path_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || std::path::Path::new(trimmed).is_absolute() {
        return trimmed.to_string();
    }
    infra::config::state_path_base()
        .join(trimmed)
        .display()
        .to_string()
}

fn normalize_paths(rows: &mut BTreeMap<String, SettingRow>) {
    for (key, row) in rows.iter_mut() {
        if is_path_key(key) {
            row.value = normalize_path_value(&row.value);
        }
    }
}

/// Resolve every key, decrypting stored secrets and reporting the unreadable ones.
fn resolve_effective(
    base: &Config,
    defaults: &Config,
    env_keys: &EnvKeys,
    rows: &BTreeMap<String, SettingRow>,
    policy: &SettingsPolicy,
    cipher: &TokenCipher,
) -> (Vec<Resolved>, BTreeSet<String>) {
    let mut resolved = settings::resolve_all(base, defaults, env_keys, rows, policy);
    let mut broken = BTreeSet::new();
    for entry in &mut resolved {
        if entry.def.kind != Kind::Secret {
            continue;
        }
        if !matches!(entry.origin, Origin::Database { .. }) {
            continue;
        }
        match cipher.decrypt(&entry.value) {
            Some(plaintext) if !plaintext.is_empty() => entry.value = plaintext,
            // An undecryptable value means the master secret changed. Do not
            // fall back to the ciphertext as if it were the password: report it
            // and use the layer below, so delivery fails loudly rather than
            // authenticating with a value nobody typed.
            _ => {
                entry.broken_secret = true;
                broken.insert(entry.def.key.to_string());
                entry.value = (entry.def.get)(base);
            }
        }
    }
    (resolved, broken)
}
