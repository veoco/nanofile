//! Effective outbound-mail settings.
//!
//! There is one source of truth now: the layered configuration. `[email]` in the
//! config file seeds these values, a `NANOFILE_EMAIL_*` variable overrides them,
//! and a row saved at `/sysadmin/settings/email/` wins over the file unless the
//! `[settings]` policy says otherwise. The mailer reads a snapshot and nothing
//! else, so there is no second copy to reconcile and no cache to invalidate.
//!
//! The one thing that is *not* here is whether a stored secret can still be
//! decrypted: the settings service reports that per key, so the page can say
//! "unreadable, enter it again" while delivery simply fails at SMTP with the
//! recorded reason.

use infra::config::Config;

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

/// The settings actually used to deliver mail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailSettings {
    /// The master switch, `[email] enabled`.
    pub enabled: bool,
    pub paused: bool,
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    /// The password, when one is configured and usable.
    pub password: Option<String>,
    /// Whether a password is configured at all (drives the form's "keep the
    /// stored secret" placeholder).
    pub password_set: bool,
    pub from_address: String,
    pub from_name: String,
    pub timeout_secs: u64,
    pub max_attempts: u32,
    pub notify_new_device: bool,
    pub notify_api_key_created: bool,
    pub notify_new_login: bool,
}

impl EmailSettings {
    /// The settings in force for this configuration snapshot.
    pub fn from_config(config: &Config) -> Self {
        let email = &config.email;
        let password = email
            .password
            .as_ref()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        Self {
            enabled: email.enabled,
            paused: email.paused,
            host: email.host.trim().to_string(),
            port: email.port,
            tls: TlsMode::from_id(&email.tls).unwrap_or(TlsMode::StartTls),
            username: email.username.trim().to_string(),
            password_set: password.is_some(),
            password,
            from_address: email.from_address.trim().to_string(),
            from_name: email.from_name.trim().to_string(),
            timeout_secs: email.timeout_secs.max(1),
            max_attempts: email.max_attempts.max(1),
            notify_new_device: email.notify_new_device,
            notify_api_key_created: email.notify_api_key_created,
            notify_new_login: email.notify_new_login,
        }
    }

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
    use infra::config::EmailConfig;

    fn config_with(email: EmailConfig) -> Config {
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
    fn settings_come_straight_from_the_configuration() {
        let config = config_with(EmailConfig {
            enabled: true,
            host: " smtp.example.com ".to_string(),
            port: 2525,
            tls: "tls".to_string(),
            username: "bot".to_string(),
            password: Some("secret".to_string()),
            from_address: "nanofile@example.com".to_string(),
            from_name: "Nanofile".to_string(),
            timeout_secs: 7,
            max_attempts: 3,
            paused: false,
            notify_new_device: false,
            notify_api_key_created: true,
            notify_new_login: true,
        });
        let settings = EmailSettings::from_config(&config);
        assert_eq!(settings.host, "smtp.example.com");
        assert_eq!(settings.port, 2525);
        assert_eq!(settings.tls, TlsMode::ImplicitTls);
        assert_eq!(settings.password.as_deref(), Some("secret"));
        assert!(settings.password_set);
        assert_eq!(settings.timeout_secs, 7);
        assert_eq!(settings.max_attempts, 3);
        assert!(settings.ready());
    }

    #[test]
    fn settings_are_not_ready_without_host_and_sender() {
        let config = config_with(EmailConfig {
            enabled: true,
            ..Default::default()
        });
        let settings = EmailSettings::from_config(&config);
        assert!(!settings.ready());
        assert_eq!(settings.missing(), vec!["host", "from_address"]);
    }

    #[test]
    fn a_configured_but_disabled_server_is_never_ready() {
        let config = config_with(EmailConfig {
            enabled: false,
            host: "smtp.example.com".to_string(),
            from_address: "nanofile@example.com".to_string(),
            ..Default::default()
        });
        let settings = EmailSettings::from_config(&config);
        assert!(settings.missing().is_empty(), "nothing is missing");
        assert!(
            !settings.ready(),
            "the master switch alone decides whether mail exists"
        );
    }

    #[test]
    fn readiness_reports_each_missing_field() {
        let mut settings = EmailSettings::from_config(&Config::default());
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
        let mut settings = EmailSettings::from_config(&Config::default());
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
}
