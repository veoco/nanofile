//! Outbound email: settings, message rendering, the retry queue and delivery.
//!
//! # Shape of the subsystem
//!
//! * [`settings`] — the effective SMTP settings, read from the layered
//!   configuration (environment, config file, saved row); see
//!   [`crate::settings`].
//! * [`message`] — rendering a kind into subject + text + HTML and assembling
//!   the raw RFC 5322 message.
//! * [`queue`] — the outbox table: queueing, claiming, backoff, retention.
//! * [`transport`] — SMTP with mandatory certificate validation.
//! * [`detect`] — whether a login is from a device/browser the account has not
//!   used before.
//!
//! # Rules every caller can rely on
//!
//! * **Nothing here can fail a request.** Notifications are queued and sent in
//!   the background; a delivery failure is recorded on the queue row and in the
//!   log, never returned to the user.
//! * **The master switch is in the config file.** [`Mailer::ready`] is false
//!   unless `config.email.enabled` is set *and* the SMTP settings are complete,
//!   and while it is false nothing is minted, queued or sent — which is what
//!   keeps the password-reset flow from pretending to work.
//! * **A reset link is never stored in the clear.** The rendered message is
//!   encrypted with the same domain-separated cipher as repository sync tokens
//!   and is wiped from the row as soon as the message is delivered.

pub mod detect;
pub mod i18n;
pub mod message;
pub mod queue;
pub mod settings;
pub mod transport;

use std::sync::Arc;

use base::error::AppError;
#[cfg(test)]
use infra::config::Config;
use infra::crypto::token_encryption::TokenCipher;
use infra::entity::email_message;

use crate::repository::Repositories;

pub use i18n::MailStrings;
pub use message::MailParams;
pub use settings::{EmailSettings, TlsMode};

/// Scheduler task name for the outbox drainer.
///
/// A constant because three places must agree on it: the registration in
/// `scheduler_setup`, the admin page's "deliver now" button, and the task list
/// that button's label is read from.
pub const TASK_NAME: &str = "email delivery";

/// What a message is for. The persisted form is [`MailKind::id`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailKind {
    /// The password-reset link itself.
    PasswordReset,
    /// A client device signed in for the first time.
    NewDevice,
    /// An API key was created.
    ApiKeyCreated,
    /// A browser signed in from a fingerprint the account had not used.
    NewLogin,
    /// The administrator's own probe from `/sysadmin/email/`.
    Test,
}

impl MailKind {
    pub const ALL: [MailKind; 5] = [
        MailKind::PasswordReset,
        MailKind::NewDevice,
        MailKind::ApiKeyCreated,
        MailKind::NewLogin,
        MailKind::Test,
    ];

    /// Stable identifier, persisted in `email_messages.kind`.
    pub const fn id(self) -> &'static str {
        match self {
            MailKind::PasswordReset => "password_reset",
            MailKind::NewDevice => "new_device",
            MailKind::ApiKeyCreated => "api_key_created",
            MailKind::NewLogin => "new_login",
            MailKind::Test => "test",
        }
    }

    /// Parse a persisted identifier.
    pub fn from_id(value: &str) -> Option<Self> {
        MailKind::ALL.into_iter().find(|kind| kind.id() == value)
    }

    /// The locale key prefix, for the admin page's row labels.
    pub const fn i18n_key(self) -> &'static str {
        match self {
            MailKind::PasswordReset => "admin.email_kind_password_reset",
            MailKind::NewDevice => "admin.email_kind_new_device",
            MailKind::ApiKeyCreated => "admin.email_kind_api_key_created",
            MailKind::NewLogin => "admin.email_kind_new_login",
            MailKind::Test => "admin.email_kind_test",
        }
    }
}

/// A reason a message could not be delivered.
///
/// Carried as text because that is exactly what the queue row and the log
/// record; converting it into an [`AppError`] is for the one caller that shows
/// a failure to an administrator (the test message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailError(String);

impl MailError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MailError {}

impl From<MailError> for AppError {
    fn from(error: MailError) -> Self {
        AppError::Internal(error.0)
    }
}

/// The mail subsystem, held on `AppState`.
///
/// It keeps the configuration *handle*, not a copy: a setting saved at
/// `/sysadmin/settings/email/` is read by the very next delivery, and there is
/// no cache to invalidate because there is no second source of truth.
pub struct Mailer {
    repos: Arc<Repositories>,
    cipher: Arc<TokenCipher>,
    config: crate::settings::RuntimeConfig,
}

impl Mailer {
    pub fn new(
        repos: Arc<Repositories>,
        cipher: Arc<TokenCipher>,
        config: crate::settings::RuntimeConfig,
    ) -> Self {
        Self {
            repos,
            cipher,
            config,
        }
    }

    /// Whether `[email] enabled` is set. Says nothing about whether the SMTP
    /// settings are complete — that is [`Self::ready`].
    pub fn config_enabled(&self) -> bool {
        self.config.get().email.enabled
    }

    /// This server's public URL, for links and prose in a body.
    pub fn site_url(&self) -> String {
        self.config.get().server.site_url.clone()
    }

    /// The domain used for `EHLO` and the generated `Message-ID`.
    pub fn hello_name(&self) -> String {
        message::message_id_domain(&self.site_url())
    }

    /// The default UI language, used when a recipient has no preference.
    pub fn default_language(&self) -> String {
        self.config.get().ui.default_language.clone()
    }

    /// The settings in force for this process right now.
    ///
    /// One snapshot read: a save at `/sysadmin/settings/email/` replaces the
    /// snapshot, so the very next delivery uses it and no cache has to be
    /// invalidated.
    pub fn settings(&self) -> EmailSettings {
        EmailSettings::from_config(&self.config.get())
    }

    /// Whether a message could be delivered right now.
    pub fn ready(&self) -> bool {
        self.settings().ready()
    }

    /// Whether this kind should be sent: the subsystem must be ready and the
    /// kind's switch on.
    pub fn allows(&self, kind: MailKind) -> bool {
        let settings = self.settings();
        settings.ready() && settings.allows(kind)
    }

    /// Send one message from the admin page and report the result.
    ///
    /// The only synchronous send in the subsystem, and deliberately so: an
    /// administrator clicking "send test" is waiting for exactly this answer.
    /// Notifications go the other way (queued, backgrounded) because a user's
    /// login must not wait on SMTP.
    pub async fn send_test(&self, to: &str, language: Option<&str>) -> Result<(), AppError> {
        let settings = self.settings();
        if !settings.enabled {
            return Err(AppError::BadRequest(
                "outbound mail is switched off ([email] enabled)".to_string(),
            ));
        }
        if settings.paused {
            return Err(AppError::BadRequest(
                "delivery is paused in the email settings".to_string(),
            ));
        }
        let missing = settings.missing();
        if !missing.is_empty() {
            return Err(AppError::BadRequest(format!(
                "the email settings are incomplete: {}",
                missing.join(", ")
            )));
        }
        if !settings::is_valid_address(to) {
            return Err(AppError::BadRequest(format!(
                "not a valid email address: {to}"
            )));
        }

        let now = chrono::Utc::now().timestamp();
        let params = MailParams {
            site_url: Some(self.site_url().to_string()),
            time_ts: Some(now),
            ..Default::default()
        };
        let row = self
            .queue(&settings, MailKind::Test, to, None, language, &params, now)
            .await?;

        let delivered = queue::attempt(
            &self.repos,
            &self.cipher,
            &settings,
            &self.hello_name(),
            &row,
            now,
        )
        .await?;
        if delivered {
            return Ok(());
        }
        // Report the recorded reason rather than a generic failure: the
        // administrator is about to go and fix it.
        let reason = self
            .repos
            .email_message
            .find_by_id(row.id)
            .await?
            .and_then(|row| row.last_error)
            .unwrap_or_else(|| "delivery failed".to_string());
        Err(AppError::BadRequest(reason))
    }

    /// Queue a notification and try to deliver it in the background.
    ///
    /// Returns immediately. Every failure path (mail disabled, the kind switched
    /// off, an unparseable recipient, a database error) is logged and swallowed:
    /// no notification is worth failing a sign-in over.
    pub async fn notify(
        &self,
        kind: MailKind,
        to: &str,
        user_id: Option<i32>,
        language: Option<&str>,
        params: MailParams,
    ) {
        if let Err(e) = self.notify_inner(kind, to, user_id, language, params).await {
            tracing::warn!("could not queue a {} notification: {e}", kind.id());
        }
    }

    async fn notify_inner(
        &self,
        kind: MailKind,
        to: &str,
        user_id: Option<i32>,
        language: Option<&str>,
        params: MailParams,
    ) -> Result<(), AppError> {
        let settings = self.settings();
        // Cheapest checks first: while mail is off — the default — a login
        // costs nothing at all, not even an address validation.
        if !settings.ready() || !settings.allows(kind) {
            return Ok(());
        }
        if !settings::is_valid_address(to) {
            tracing::warn!(
                "not sending a {} notification: unusable recipient address",
                kind.id()
            );
            return Ok(());
        }

        let now = chrono::Utc::now().timestamp();
        let row = self
            .queue(&settings, kind, to, user_id, language, &params, now)
            .await?;
        self.spawn_delivery(settings, row);
        Ok(())
    }

    /// Tell the owner that a client device signed in for the first time.
    pub async fn notify_new_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
        device_name: &str,
        ip: &str,
    ) {
        // The identifier is only for matching; the reader wants the name the
        // client reported, falling back to the platform when it reported none.
        let device = if device_name.trim().is_empty() {
            if device_id.trim().is_empty() {
                platform
            } else {
                device_id
            }
        } else {
            device_name
        };
        let params = MailParams {
            device: Some(device.to_string()),
            platform: Some(platform.to_string()),
            ip: Some(ip.to_string()),
            ..Default::default()
        };
        self.notify_for_user(MailKind::NewDevice, user_id, params)
            .await;
    }

    /// Tell the owner that a browser they had not used signed in.
    ///
    /// The browser is described the same way the credentials page describes it,
    /// so the recipient can match the message against what they see in
    /// *Settings → Credentials*.
    pub async fn notify_new_login(&self, user_id: i32, user_agent: Option<&str>, ip: &str) {
        let browser = user_agent
            .map(crate::ui::user_agent::describe)
            .filter(|label| !label.trim().is_empty());
        let params = MailParams {
            browser,
            ip: Some(ip.to_string()),
            ..Default::default()
        };
        self.notify_for_user(MailKind::NewLogin, user_id, params)
            .await;
    }

    /// Tell the owner that an API key was created.
    pub async fn notify_api_key_created(&self, user_id: i32, key_name: &str) {
        let params = MailParams {
            key_name: Some(key_name.to_string()),
            ..Default::default()
        };
        self.notify_for_user(MailKind::ApiKeyCreated, user_id, params)
            .await;
    }

    /// Notify an account, resolving the recipient from the user row.
    ///
    /// One place that decides which address and which language a notification
    /// goes to: `users.email` is the unique login identity, and a preferred
    /// language makes the message readable — an unset preference falls back to
    /// the site default inside [`MailStrings::get`].
    async fn notify_for_user(&self, kind: MailKind, user_id: i32, mut params: MailParams) {
        let user = match self.repos.user.find_by_id(user_id).await {
            Ok(Some(user)) => user,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("could not load user {user_id} for a notification: {e}");
                return;
            }
        };
        if params.display_name.is_none() {
            params.display_name = Some(user.nickname());
        }
        self.notify(
            kind,
            &user.email,
            Some(user_id),
            user.language.as_deref(),
            params,
        )
        .await;
    }

    /// Send the password-reset link.
    ///
    /// Same shape as [`Self::notify`], but the caller has already established
    /// that the reset flow is available, and the link is only good for this
    /// message: it is rendered once, encrypted into the queue and never kept in
    /// the clear — which is what the old code's dropped `reset_url` was missing.
    pub async fn send_password_reset(
        &self,
        to: &str,
        user_id: i32,
        language: Option<&str>,
        reset_url: &str,
        ip: Option<&str>,
    ) {
        let params = MailParams {
            link: Some(reset_url.to_string()),
            ttl_days: Some(crate::service::auth::password_reset::RESET_TOKEN_TTL_SECONDS / 86_400),
            ip: ip.map(str::to_string),
            time_ts: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        };
        self.notify(MailKind::PasswordReset, to, Some(user_id), language, params)
            .await;
    }

    /// Deliver everything that is due and apply retention.
    pub async fn drain_once(&self) -> Result<queue::DrainReport, AppError> {
        let settings = self.settings();
        if !settings.ready() {
            // Not an error: the drainer runs on a timer and mail may simply be
            // switched off or paused at the moment.
            return Ok(queue::DrainReport::default());
        }
        let now = chrono::Utc::now().timestamp();
        queue::drain(
            &self.repos,
            &self.cipher,
            &settings,
            &self.hello_name(),
            now,
        )
        .await
    }

    /// Report at startup how outbound mail is configured.
    ///
    /// Two failures are worth a log line every time the server starts, because
    /// both are silent from the outside:
    ///
    /// * password reset is enabled while mail is not — the request form accepts
    ///   an address, answers "if that address is registered…", and never sends
    ///   anything;
    /// * mail is enabled but the settings cannot deliver — nothing will ever
    ///   arrive and the queue only fills up.
    ///
    /// Which *source* each value came from — and whether the config file is
    /// being superseded by a saved setting — is reported by the settings service
    /// (`SettingsService::log_startup_diagnostics`), which is the only place
    /// that knows.
    pub fn log_startup_diagnostics(&self) {
        let config = self.config.get();
        if !config.email.enabled {
            if config.auth.enable_password_reset {
                tracing::warn!(
                    "[auth] enable_password_reset is on but [email] enabled is off: \
                     /accounts/password/reset/ renders a generic page, mints no token and \
                     sends no mail. Set [email] enabled = true and configure SMTP at \
                     /sysadmin/settings/email/, or set enable_password_reset = false."
                );
            }
            if !config.email.host.trim().is_empty() || !config.email.from_address.trim().is_empty()
            {
                tracing::info!(
                    "[email] has a host/sender configured but enabled = false; outbound mail \
                     is inert until the switch is on."
                );
            }
            return;
        }

        let missing = self.settings().missing();
        if !missing.is_empty() {
            tracing::warn!(
                "email is enabled but cannot deliver yet: {} missing. Configure it at \
                 /sysadmin/settings/email/.",
                missing.join(", ")
            );
        }
    }

    /// Put a failed message back in the queue and try it once.
    pub async fn retry(&self, id: i32) -> Result<bool, AppError> {
        let settings = self.settings();
        let now = chrono::Utc::now().timestamp();
        if !self.repos.email_message.requeue(id, now).await? {
            return Ok(false);
        }
        let Some(row) = self.repos.email_message.find_by_id(id).await? else {
            return Ok(false);
        };
        queue::attempt(
            &self.repos,
            &self.cipher,
            &settings,
            &self.hello_name(),
            &row,
            now,
        )
        .await?;
        Ok(true)
    }

    /// Render and queue one message, returning the row.
    async fn queue(
        &self,
        settings: &EmailSettings,
        kind: MailKind,
        to: &str,
        user_id: Option<i32>,
        language: Option<&str>,
        params: &MailParams,
        now: i64,
    ) -> Result<email_message::Model, AppError> {
        let strings = MailStrings::get(language, &self.default_language());
        let mut params = params.clone();
        if params.site_url.is_none() {
            params.site_url = Some(self.site_url().to_string());
        }
        if params.time_ts.is_none() {
            params.time_ts = Some(now);
        }

        let content = message::render(strings, kind, &params);
        let raw = message::format_message(
            settings,
            to,
            &content,
            &message::message_id_domain(&self.site_url()),
        )
        .map_err(AppError::from)?;

        queue::enqueue(
            &self.repos,
            &self.cipher,
            kind.id(),
            to,
            user_id,
            &content.subject,
            &raw,
            now,
        )
        .await
    }

    /// Deliver a queued message without making the caller wait for it.
    fn spawn_delivery(&self, settings: EmailSettings, row: email_message::Model) {
        let repos = self.repos.clone();
        let cipher = self.cipher.clone();
        let hello_name = self.hello_name();
        tokio::spawn(async move {
            let now = chrono::Utc::now().timestamp();
            if let Err(e) = queue::attempt(&repos, &cipher, &settings, &hello_name, &row, now).await
            {
                tracing::warn!(
                    "the queued {} message (id {}) could not be attempted: {e}",
                    row.kind,
                    row.id
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RuntimeConfig;
    use infra::entity::email_message::Status;
    use migration::MigratorTrait;
    use sea_orm::Database;

    async fn mailer_with(config: Config) -> (Arc<Repositories>, Mailer) {
        let (repos, mailer, _runtime) = mailer_with_handle(config).await;
        (repos, mailer)
    }

    /// The mailer shares the configuration handle with its caller, so a test can
    /// replace the snapshot the way a settings save does.
    async fn mailer_with_handle(config: Config) -> (Arc<Repositories>, Mailer, RuntimeConfig) {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        let repos = Arc::new(Repositories::new_for_tests(Arc::new(db)));
        let cipher = Arc::new(TokenCipher::from_master_key(b"test-secret"));
        let runtime = RuntimeConfig::new(config);
        let mailer = Mailer::new(repos.clone(), cipher, runtime.clone());
        (repos, mailer, runtime)
    }

    fn ready_config() -> Config {
        let mut config = Config::default();
        config.server.site_url = "https://files.example.com".to_string();
        config.email.enabled = true;
        config.email.host = "127.0.0.1".to_string();
        config.email.port = 1;
        config.email.tls = "none".to_string();
        config.email.from_address = "nanofile@example.com".to_string();
        config.email.timeout_secs = 1;
        config.email.max_attempts = 2;
        config
    }

    #[test]
    fn kind_ids_round_trip() {
        for kind in MailKind::ALL {
            assert_eq!(MailKind::from_id(kind.id()), Some(kind));
            assert!(kind.i18n_key().starts_with("admin.email_kind_"));
        }
        assert_eq!(MailKind::from_id("digest"), None);
    }

    /// The default deployment: mail off. Nothing may reach the database.
    #[tokio::test]
    async fn with_email_disabled_nothing_is_queued() {
        let (repos, mailer) = mailer_with(Config::default()).await;
        assert!(!mailer.ready());
        assert!(!mailer.allows(MailKind::NewLogin));

        mailer
            .notify(
                MailKind::NewLogin,
                "user@example.com",
                Some(1),
                Some("en"),
                MailParams::default(),
            )
            .await;
        mailer
            .send_password_reset("user@example.com", 1, Some("en"), "https://x/y/", None)
            .await;

        assert!(
            repos
                .email_message
                .list_recent(10, None)
                .await
                .unwrap()
                .is_empty(),
            "a disabled mail subsystem must not write queue rows"
        );
        assert_eq!(mailer.drain_once().await.unwrap().attempted(), 0);
    }

    /// Disabled, enabled-but-incomplete, and paused are three ways to be not
    /// ready; each has to keep the reset flow closed.
    #[tokio::test]
    async fn readiness_needs_the_switch_and_complete_settings() {
        let mut config = Config::default();
        config.email.enabled = false;
        config.email.host = "smtp.example.com".to_string();
        config.email.from_address = "nanofile@example.com".to_string();
        let (_repos, mailer) = mailer_with(config).await;
        assert!(!mailer.ready(), "the master switch still decides");

        let (_repos, mailer) = mailer_with(ready_config()).await;
        assert!(mailer.ready());

        let mut paused = ready_config();
        paused.email.paused = true;
        let (_repos, mailer) = mailer_with(paused).await;
        assert!(!mailer.ready(), "paused stops delivery");
    }

    /// A settings save replaces the configuration snapshot; the very next
    /// delivery must read it. This is what removed the old 30-second cache.
    #[tokio::test]
    async fn a_settings_change_is_visible_to_the_next_delivery() {
        let (_repos, mailer, runtime) = mailer_with_handle(ready_config()).await;
        assert_eq!(mailer.settings().from_name, "");

        let mut next = ready_config();
        next.email.from_name = "Mail Bot".to_string();
        next.email.host = "smtp.example.com".to_string();
        next.email.port = 2525;
        runtime.replace(next);

        let settings = mailer.settings();
        assert_eq!(settings.host, "smtp.example.com");
        assert_eq!(settings.port, 2525);
        assert_eq!(settings.from_name, "Mail Bot");
        assert!(settings.enabled);
    }

    #[tokio::test]
    async fn the_test_message_reports_why_it_could_not_be_sent() {
        let (_repos, mailer) = mailer_with(Config::default()).await;
        let error = mailer
            .send_test("admin@example.com", Some("en"))
            .await
            .expect_err("disabled mail cannot send a test");
        assert!(error.to_string().contains("switched off"));

        let (repos, mailer) = mailer_with(ready_config()).await;
        let error = mailer
            .send_test("not-an-address", Some("en"))
            .await
            .expect_err("an invalid recipient is refused before queueing");
        assert!(error.to_string().contains("valid email address"));
        assert!(
            repos
                .email_message
                .list_recent(10, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn notifications_can_be_switched_off_individually() {
        let mut config = ready_config();
        config.email.notify_new_login = false;
        config.email.notify_new_device = false;
        let (repos, mailer) = mailer_with(config).await;

        mailer
            .notify(
                MailKind::NewLogin,
                "user@example.com",
                Some(1),
                Some("en"),
                MailParams::default(),
            )
            .await;
        mailer
            .notify(
                MailKind::NewDevice,
                "user@example.com",
                Some(1),
                Some("en"),
                MailParams::default(),
            )
            .await;
        assert!(
            repos
                .email_message
                .list_recent(10, None)
                .await
                .unwrap()
                .is_empty(),
            "switched-off kinds are not queued at all"
        );
    }

    /// A queued notification that cannot be delivered lands in the table as a
    /// failed row with the reason, and its body is still encrypted.
    #[tokio::test]
    async fn an_undeliverable_notification_leaves_an_auditable_row() {
        let (repos, mailer) = mailer_with(ready_config()).await;

        // Port 1 has nothing listening, and the settings allow two attempts.
        let row = mailer
            .queue(
                &mailer.settings(),
                MailKind::NewLogin,
                "user@example.com",
                Some(1),
                Some("en"),
                &MailParams {
                    display_name: Some("Ada".to_string()),
                    browser: Some("Firefox on Linux".to_string()),
                    ip: Some("203.0.113.7".to_string()),
                    ..Default::default()
                },
                chrono::Utc::now().timestamp(),
            )
            .await
            .unwrap();

        // Two drains with an explicit clock: the first retries with a backoff,
        // the second (after that backoff) is the last attempt allowed.
        let settings = mailer.settings();
        let cipher = TokenCipher::from_master_key(b"test-secret");
        let start = chrono::Utc::now().timestamp();
        queue::drain(&repos, &cipher, &settings, &mailer.hello_name(), start)
            .await
            .unwrap();
        let after_first = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_first.status, Status::Pending.id());
        assert!(after_first.next_attempt_at > start);

        queue::drain(
            &repos,
            &cipher,
            &settings,
            &mailer.hello_name(),
            after_first.next_attempt_at,
        )
        .await
        .unwrap();

        let after = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, Status::Failed.id());
        assert_eq!(after.attempts, 2);
        assert!(after.last_error.is_some(), "the failure reason is recorded");
        assert!(after.subject.contains("sign-in"));
        assert!(after.body_enc.is_some(), "a retryable row keeps its body");

        // Retention still sees the body as ciphertext, never the link.
        assert!(!after.body_enc.as_deref().unwrap().contains("http"));
    }
}
