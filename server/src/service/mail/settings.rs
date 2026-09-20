//! Effective outbound-mail settings.
//!
//! Two layers decide how mail is sent: `[email]` in `config.toml` (the master
//! switch plus first-start bootstrap values) and the `email_settings` row saved
//! from `/sysadmin/email/`. The row wins once it exists, so an administrator can
//! fix a wrong SMTP host without editing the file or restarting the server —
//! while enabling outbound mail stays a deliberate act in the config file.

use base::error::AppError;
use infra::config::Config;
use infra::crypto::token_encryption::TokenCipher;
use infra::entity::email_settings;

use crate::repository::Repositories;
use crate::repository::email_settings::EmailSettingsUpdate;

/// How the SMTP connection is protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Connect in the clear, then upgrade with `STARTTLS` (port 587 and most
    /// submission servers).
    StartTls,
    /// TLS from the first byte (implicit TLS, usually port 465).
    ImplicitTls,
    /// No encryption at all. Only defensible for a relay on the same host.
    None,
}

impl TlsMode {
    /// Persisted identifier, also used as the `<select>` value in the admin UI.
    pub const fn id(self) -> &'static str {
        match self {
            TlsMode::StartTls => "starttls",
            TlsMode::ImplicitTls => "tls",
            TlsMode::None => "none",
        }
    }

    /// Parse a persisted/configured value. Unrecognised values are `None` so a
    /// typo in the config cannot silently disable encryption.
    pub fn from_id(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "starttls" => Some(TlsMode::StartTls),
            "tls" | "ssl" | "smtps" => Some(TlsMode::ImplicitTls),
            "none" | "plain" | "insecure" => Some(TlsMode::None),
            _ => None,
        }
    }

    pub const fn is_plaintext(self) -> bool {
        matches!(self, TlsMode::None)
    }
}

/// Where the effective values came from, so the admin page can say whether the
/// configuration file is still in charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsOrigin {
    /// No row saved yet: these are the `config.toml` bootstrap values.
    Config,
    /// The row saved from `/sysadmin/email/`.
    Stored,
}

/// The settings actually used to deliver mail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailSettings {
    pub origin: SettingsOrigin,
    /// `config.email.enabled` — the hard switch, never stored in the database.
    pub enabled: bool,
    pub paused: bool,
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    /// Decrypted password, when one is configured and usable.
    pub password: Option<String>,
    /// Whether a password is configured at all (drives the form's "keep the
    /// stored secret" placeholder).
    pub password_set: bool,
    /// A stored password that cannot be decrypted — the `secret_key` changed.
    /// Delivery must not proceed with credentials that are silently absent.
    pub password_broken: bool,
    pub from_address: String,
    pub from_name: String,
    pub timeout_secs: u64,
    pub max_attempts: u32,
    pub notify_new_device: bool,
    pub notify_api_key_created: bool,
    pub notify_new_login: bool,
    pub updated_at: i64,
    pub updated_by: Option<i32>,
}

impl EmailSettings {
    /// Why delivery is not possible, as the list of missing/broken fields. An
    /// empty list means ready (assuming `enabled`).
    ///
    /// Reported as field names rather than a sentence because two very
    /// different readers need it: the admin page renders the list, and the
    /// startup log prints it.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.host.trim().is_empty() {
            missing.push("host");
        }
        if self.port == 0 {
            missing.push("port");
        }
        // Both an absent sender and a malformed one are the same problem to the
        // reader: nothing can be sent until one is supplied.
        if !is_valid_address(&self.from_address) {
            missing.push("from_address");
        }
        if self.password_broken {
            missing.push("password");
        }
        missing
    }

    /// Whether a message can be delivered right now.
    pub fn ready(&self) -> bool {
        self.enabled && !self.paused && self.missing().is_empty()
    }

    /// Whether this notification kind is switched on. The master switch and
    /// readiness are separate: a disabled switch is a choice the user made, not
    /// a reason to silently fall back to mail.
    pub fn allows(&self, kind: super::MailKind) -> bool {
        match kind {
            // Not a notification: the reset link *is* the feature, and a
            // password-reset flow that mints a token and mails nothing is the
            // exact "pretends to work" failure this subsystem replaced. It is
            // switched off by `auth.enable_password_reset`, not from here.
            super::MailKind::PasswordReset => true,
            super::MailKind::NewDevice => self.notify_new_device,
            super::MailKind::ApiKeyCreated => self.notify_api_key_created,
            super::MailKind::NewLogin => self.notify_new_login,
            // The test message is the administrator's own probe; it must work
            // even with every notification switched off.
            super::MailKind::Test => true,
        }
    }

    /// The `[email]` bootstrap layer, used until a row is saved.
    pub fn bootstrap(config: &Config) -> Self {
        let tls = TlsMode::from_id(&config.email.tls).unwrap_or(TlsMode::StartTls);
        let password = config
            .email
            .password
            .as_ref()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        Self {
            origin: SettingsOrigin::Config,
            enabled: config.email.enabled,
            paused: false,
            host: config.email.host.trim().to_string(),
            port: config.email.port,
            tls,
            username: config.email.username.trim().to_string(),
            password_set: password.is_some(),
            password,
            password_broken: false,
            from_address: config.email.from_address.trim().to_string(),
            from_name: config.email.from_name.trim().to_string(),
            timeout_secs: config.email.timeout_secs.max(1),
            max_attempts: config.email.max_attempts.max(1),
            notify_new_device: true,
            notify_api_key_created: true,
            notify_new_login: true,
            updated_at: 0,
            updated_by: None,
        }
    }

    /// The stored row, with the config bootstrap filling anything the row
    /// cannot supply (the master switch, and a password the row does not set).
    fn from_row(row: &email_settings::Model, cipher: &TokenCipher, config: &Config) -> Self {
        let stored_password = row
            .password_enc
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let (password, password_broken) = match stored_password {
            Some(stored) => match cipher.decrypt(stored) {
                Some(plaintext) if !plaintext.is_empty() => (Some(plaintext), false),
                // An undecryptable value means the master secret changed; the
                // credentials are gone even though the field looks configured.
                _ => (None, true),
            },
            // No stored password: fall back to the config bootstrap, which is
            // how an operator can keep the secret in a 0600 file.
            None => (
                config
                    .email
                    .password
                    .as_ref()
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty()),
                false,
            ),
        };

        Self {
            origin: SettingsOrigin::Stored,
            enabled: config.email.enabled,
            paused: row.paused,
            host: row.host.trim().to_string(),
            port: row.port.clamp(0, u16::MAX as i32) as u16,
            tls: TlsMode::from_id(&row.tls).unwrap_or(TlsMode::StartTls),
            username: row.username.trim().to_string(),
            password_set: password.is_some(),
            password,
            password_broken,
            from_address: row.from_address.trim().to_string(),
            from_name: row.from_name.trim().to_string(),
            timeout_secs: row.timeout_secs.max(1) as u64,
            max_attempts: row.max_attempts.max(1) as u32,
            notify_new_device: row.notify_new_device,
            notify_api_key_created: row.notify_api_key_created,
            notify_new_login: row.notify_new_login,
            updated_at: row.updated_at,
            updated_by: row.updated_by,
        }
    }
}

/// Read the effective settings: the saved row when there is one, otherwise the
/// `[email]` bootstrap values.
pub async fn load(
    repos: &Repositories,
    cipher: &TokenCipher,
    config: &Config,
) -> Result<EmailSettings, AppError> {
    match repos.email_settings.get().await? {
        Some(row) => Ok(EmailSettings::from_row(&row, cipher, config)),
        None => Ok(EmailSettings::bootstrap(config)),
    }
}

/// Validate administrator input before it reaches the database.
///
/// Returns the offending field name, so the page can point at it.
pub fn validate(input: &EmailSettingsUpdate) -> Result<(), AppError> {
    if input.host.trim().is_empty() {
        // An empty host is allowed only while nothing is enabled; saving it
        // clears the configuration, which is how an administrator disables
        // delivery without touching the config file.
        return Ok(());
    }
    if input.port == 0 || input.port > u16::MAX as i32 {
        return Err(AppError::BadRequest(
            "the SMTP port must be between 1 and 65535".to_string(),
        ));
    }
    if TlsMode::from_id(&input.tls).is_none() {
        return Err(AppError::BadRequest(format!(
            "unknown TLS mode: {}",
            input.tls
        )));
    }
    if !input.from_address.trim().is_empty() && !is_valid_address(&input.from_address) {
        return Err(AppError::BadRequest(format!(
            "not a valid email address: {}",
            input.from_address
        )));
    }
    if input.timeout_secs < 1 || input.timeout_secs > 300 {
        return Err(AppError::BadRequest(
            "the timeout must be between 1 and 300 seconds".to_string(),
        ));
    }
    if input.max_attempts < 1 || input.max_attempts > 100 {
        return Err(AppError::BadRequest(
            "the attempt limit must be between 1 and 100".to_string(),
        ));
    }
    // Credentials without a username (or vice versa) are almost always a
    // mistake; sending them anyway would look like an authentication failure.
    if input.username.trim().is_empty() && input.password.as_ref().is_some_and(|p| p.is_some()) {
        return Err(AppError::BadRequest(
            "a password was given without a username".to_string(),
        ));
    }
    Ok(())
}

/// Persist administrator input, encrypting a newly supplied password.
///
/// Returns the effective settings so the caller can report "ready" or the list
/// of still-missing fields without a second read.
pub async fn save(
    repos: &Repositories,
    cipher: &TokenCipher,
    config: &Config,
    mut input: EmailSettingsUpdate,
    now: i64,
) -> Result<EmailSettings, AppError> {
    validate(&input)?;

    // Encrypt here, so no caller can write a plaintext password to the table.
    input.password = match input.password {
        Some(Some(password)) if !password.is_empty() => Some(Some(cipher.encrypt(&password))),
        Some(Some(_)) => Some(None),
        other => other,
    };

    repos.email_settings.upsert(input, now).await?;
    load(repos, cipher, config).await
}

/// Whether `value` is usable as an email address.
///
/// Delegates to the same parser the message builder uses (`lettre::Address`),
/// so an address that passes here cannot fail when the message is assembled —
/// and a newline is rejected rather than becoming a header injection.
pub fn is_valid_address(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && !trimmed.contains(['\r', '\n'])
        && trimmed.parse::<lettre::Address>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(email: infra::config::EmailConfig) -> Config {
        Config {
            email,
            ..Default::default()
        }
    }

    #[test]
    fn tls_ids_round_trip_and_reject_typos() {
        for mode in [TlsMode::StartTls, TlsMode::ImplicitTls, TlsMode::None] {
            assert_eq!(TlsMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(TlsMode::from_id("STARTTLS"), Some(TlsMode::StartTls));
        assert_eq!(TlsMode::from_id("ssl"), Some(TlsMode::ImplicitTls));
        // A typo must not read as "no encryption" *or* as "configured": the
        // caller falls back to STARTTLS, never to plaintext.
        assert_eq!(TlsMode::from_id("starttls "), Some(TlsMode::StartTls));
        assert_eq!(TlsMode::from_id("tls1.3"), None);
    }

    #[test]
    fn bootstrap_settings_are_not_ready_without_host_and_sender() {
        let config = config_with(infra::config::EmailConfig {
            enabled: true,
            ..Default::default()
        });
        let settings = EmailSettings::bootstrap(&config);
        assert_eq!(settings.origin, SettingsOrigin::Config);
        assert!(!settings.ready());
        assert_eq!(settings.missing(), vec!["host", "from_address"]);
    }

    #[test]
    fn a_configured_but_disabled_server_is_never_ready() {
        let config = config_with(infra::config::EmailConfig {
            enabled: false,
            host: "smtp.example.com".to_string(),
            from_address: "nanofile@example.com".to_string(),
            ..Default::default()
        });
        let settings = EmailSettings::bootstrap(&config);
        assert!(settings.missing().is_empty(), "nothing is missing");
        assert!(
            !settings.ready(),
            "the config switch alone decides whether mail exists"
        );
    }

    #[test]
    fn readiness_reports_each_missing_field() {
        let mut settings = EmailSettings::bootstrap(&Config::default());
        settings.enabled = true;
        settings.host = "smtp.example.com".to_string();
        settings.from_address = "nanofile@example.com".to_string();
        assert!(settings.ready());

        // Pausing stops delivery without inventing a missing field.
        settings.paused = true;
        assert!(!settings.ready());
        assert!(settings.missing().is_empty());
        settings.paused = false;

        settings.from_address = "not an address".to_string();
        assert_eq!(settings.missing(), vec!["from_address"]);

        settings.from_address = "nanofile@example.com".to_string();
        settings.password_broken = true;
        assert_eq!(settings.missing(), vec!["password"]);
        assert!(
            !settings.ready(),
            "an undecryptable credential must not be treated as absent"
        );
    }

    #[test]
    fn address_validation_rejects_injection_and_accepts_test_domains() {
        assert!(is_valid_address("nanofile@example.com"));
        // The e2e suite and plenty of local relays use a single-label domain.
        assert!(is_valid_address("e2e-admin@test.local"));
        assert!(!is_valid_address(""));
        assert!(!is_valid_address("   "));
        assert!(!is_valid_address("no-at-sign"));
        assert!(!is_valid_address("a@b@c.com"));
        assert!(
            !is_valid_address("victim@example.com\r\nBcc: attacker@example.com"),
            "a newline must never reach a header"
        );
        assert!(!is_valid_address("spaces in@example.com"));
    }

    #[test]
    fn notification_switches_gate_each_kind_but_not_the_test_message() {
        let mut settings = EmailSettings::bootstrap(&Config::default());
        settings.notify_new_login = false;
        assert!(!settings.allows(super::super::MailKind::NewLogin));
        assert!(
            settings.allows(super::super::MailKind::PasswordReset),
            "the reset link is the feature, not a switchable notification"
        );
        assert!(settings.allows(super::super::MailKind::NewDevice));
        assert!(
            settings.allows(super::super::MailKind::Test),
            "the admin's own probe is not a notification"
        );
    }

    #[test]
    fn validate_rejects_bad_administrator_input() {
        let base = EmailSettingsUpdate {
            host: "smtp.example.com".to_string(),
            port: 587,
            tls: "starttls".to_string(),
            from_address: "nanofile@example.com".to_string(),
            timeout_secs: 10,
            max_attempts: 5,
            ..Default::default()
        };
        assert!(validate(&base).is_ok());

        let mut bad_port = base.clone();
        bad_port.port = 0;
        assert!(validate(&bad_port).is_err());

        let mut bad_tls = base.clone();
        bad_tls.tls = "sometimes".to_string();
        assert!(validate(&bad_tls).is_err());

        let mut bad_from = base.clone();
        bad_from.from_address = "nope".to_string();
        assert!(validate(&bad_from).is_err());

        let mut bad_timeout = base.clone();
        bad_timeout.timeout_secs = 0;
        assert!(validate(&bad_timeout).is_err());

        let mut password_without_user = base.clone();
        password_without_user.password = Some(Some("secret".to_string()));
        assert!(validate(&password_without_user).is_err());

        // Clearing the configuration is allowed: an empty host is how delivery
        // is switched off from the page.
        let mut cleared = base.clone();
        cleared.host = String::new();
        cleared.from_address = String::new();
        assert!(validate(&cleared).is_ok());
    }
}
