//! Admin Web UI — email management: SMTP settings, the outbox and the
//! notifications switchboard.
//!
//! The page exists because the alternative was a config file plus a restart.
//! What it can and cannot do is part of the design: it configures *how* mail is
//! sent and turns individual notifications on or off, but it cannot enable
//! outbound mail at all — that stays with `[email] enabled` in `config.toml`,
//! so a compromised admin session cannot start mailing the server's users.
//!
//! Settings are read through `Mailer::reload`, not the cached read every other
//! caller uses: an administrator who just saved must see what they saved.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::repository::email_settings::EmailSettingsUpdate;
use crate::service::mail::settings::SettingsOrigin;
use crate::service::mail::{self, MailKind, TlsMode};
use base::error::AppError;

use super::auth_extractor::WebUser;

/// How many outbox rows one page shows.
const PAGE_SIZE: u64 = 25;

/// One row of the outbox table, ready for the template.
pub struct MailRow {
    pub id: i32,
    pub kind: String,
    /// A status id (`pending`/`sent`/`failed`), or `unknown` for a value this
    /// build does not recognise.
    pub status: String,
    pub to_address: String,
    pub subject: String,
    pub attempts: i32,
    /// Unix seconds — the page renders them in the reader's own timezone, like
    /// every other timestamp in the Web UI.
    pub created_at_ts: i64,
    pub next_attempt_at_ts: Option<i64>,
    pub sent_at_ts: Option<i64>,
    pub last_error: Option<String>,
    pub can_retry: bool,
}

/// The state of the switchboard, for the summary strip.
pub struct MailSummary {
    pub pending: u64,
    pub sent: u64,
    pub failed: u64,
}

#[derive(Template)]
#[template(path = "sysadmin/email.html")]
pub struct EmailTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,

    // ── Current state ─────────────────────────────────────────────
    /// `[email] enabled` — the switch this page cannot flip.
    pub config_enabled: bool,
    /// Everything needed for delivery is in place (and the config switch is on).
    pub ready: bool,
    pub paused: bool,
    /// The settings still come from `config.toml`; nothing is saved yet.
    pub from_config: bool,
    /// `tls = "none"`: credentials and message bodies travel in the clear.
    pub plaintext: bool,
    /// A password is stored (the form shows a placeholder instead of it).
    pub password_set: bool,
    /// A stored password cannot be decrypted, so delivery is impossible.
    pub password_broken: bool,
    /// Missing/broken settings, already translated and joined. `None` when the
    /// configuration is complete.
    pub missing_text: Option<String>,
    pub updated_at_ts: Option<i64>,
    pub summary: MailSummary,

    // ── Settings form ─────────────────────────────────────────────
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub tls_options: Vec<TlsOption>,
    pub username: String,
    pub from_address: String,
    pub from_name: String,
    pub timeout_secs: u64,
    pub max_attempts: u32,
    pub notify_new_device: bool,
    pub notify_api_key_created: bool,
    pub notify_new_login: bool,

    // ── Outbox ────────────────────────────────────────────────────
    pub messages: Vec<MailRow>,
    /// `""` (all), `pending`, `sent` or `failed`.
    pub status_filter: String,
    /// The status filter buttons, including the "everything" one.
    pub filters: Vec<FilterOption>,
    pub page: u64,
    pub has_previous: bool,
    pub has_next: bool,
    /// Whether the drainer task is registered (only when mail is enabled).
    pub drain_available: bool,
    /// Address left in the test form after a failed attempt.
    pub test_to: String,

    pub error: Option<String>,
    pub success: Option<String>,
}

/// One status filter button.
pub struct FilterOption {
    /// `""` for "all", otherwise a `email_messages.status` value.
    pub id: &'static str,
    pub label: String,
}

/// One `<option>` of the TLS picker.
pub struct TlsOption {
    pub id: &'static str,
    pub label: String,
}

/// Query parameters of the page.
#[derive(Deserialize, Default)]
pub struct EmailQuery {
    /// Confirmation carried by the redirect that follows a successful POST.
    pub action: Option<String>,
    pub status: Option<String>,
    pub page: Option<u64>,
}

/// Reasons a POST redirect can report, as locale keys.
fn success_message(t: &I18n, action: Option<&str>) -> Option<String> {
    let key = match action {
        Some("saved") => "admin.email_saved",
        Some("tested") => "admin.email_test_sent",
        Some("retried") => "admin.email_retried",
        Some("deleted") => "admin.email_deleted",
        Some("cleared") => "admin.email_cleared",
        Some("drained") => "admin.email_drain_triggered",
        _ => return None,
    };
    Some(t.tr(key).to_string())
}

/// Translate the field names `EmailSettings::missing` reports.
fn missing_labels(t: &I18n, fields: &[&'static str]) -> Vec<String> {
    fields
        .iter()
        .map(|field| t.tr(&format!("admin.email_field_{field}")).to_string())
        .collect()
}

fn status_filter(query: &EmailQuery) -> Option<infra::entity::email_message::Status> {
    use infra::entity::email_message::Status;
    query
        .status
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(Status::from_id)
}

/// GET /sysadmin/email/ — settings, outbox and delivery state (admin only).
pub async fn email_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<EmailQuery>,
) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }
    let success = success_message(I18n::get(user.language.as_deref()), query.action.as_deref());
    match render_page(&state, &user, &query, None, success, None, None).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// Build and render the page, carrying at most one banner.
async fn render_page(
    state: &Arc<AppState>,
    user: &WebUser,
    query: &EmailQuery,
    error: Option<String>,
    success: Option<String>,
    test_to: Option<String>,
    submitted: Option<EmailSettingsUpdate>,
) -> Result<Response, AppError> {
    let t = I18n::get(user.language.as_deref());
    let settings = state.mail.reload().await?;

    let filter = status_filter(query);
    let page = query.page.unwrap_or(0);
    // One extra row tells us whether a next page exists without a second count.
    let mut messages = state
        .repos
        .email_message
        .list_recent_page(filter, PAGE_SIZE + 1, page * PAGE_SIZE)
        .await?;
    let has_next = messages.len() as u64 > PAGE_SIZE;
    messages.truncate(PAGE_SIZE as usize);

    let counts = state.repos.email_message.count_by_status().await?;
    let count_of = |status: infra::entity::email_message::Status| {
        counts.get(status.id()).copied().unwrap_or(0)
    };
    use infra::entity::email_message::Status;

    let rows: Vec<MailRow> = messages
        .into_iter()
        .map(|row| MailRow {
            id: row.id,
            kind: MailKind::from_id(&row.kind)
                .map(|kind| t.tr(kind.i18n_key()).to_string())
                .unwrap_or_else(|| row.kind.clone()),
            status: row.status.clone(),
            to_address: row.to_address,
            subject: row.subject,
            attempts: row.attempts,
            created_at_ts: row.created_at,
            next_attempt_at_ts: (row.status == Status::Pending.id()
                && row.next_attempt_at > 0
                && row.attempts > 0)
                .then_some(row.next_attempt_at),
            sent_at_ts: row.sent_at,
            can_retry: row.status == Status::Failed.id(),
            last_error: row.last_error,
        })
        .collect();

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    // A rejected save shows what was typed rather than what is stored, so the
    // administrator can correct one field instead of retyping the form. The
    // password is the exception: it is never re-rendered into HTML, so a failed
    // save that included one has to be retyped (the safer trade).
    let form = submitted.as_ref();
    let shown_host = form.map_or_else(|| settings.host.clone(), |f| f.host.clone());
    let shown_port = form.map_or(settings.port, |f| f.port.clamp(0, u16::MAX as i32) as u16);
    let shown_tls = form.map_or_else(
        || settings.tls.id().to_string(),
        |f| {
            TlsMode::from_id(&f.tls)
                .unwrap_or(TlsMode::StartTls)
                .id()
                .to_string()
        },
    );
    let shown_username = form.map_or_else(|| settings.username.clone(), |f| f.username.clone());
    let shown_from_address =
        form.map_or_else(|| settings.from_address.clone(), |f| f.from_address.clone());
    let shown_from_name = form.map_or_else(|| settings.from_name.clone(), |f| f.from_name.clone());
    let shown_timeout = form.map_or(settings.timeout_secs, |f| f.timeout_secs.max(0) as u64);
    let shown_attempts = form.map_or(settings.max_attempts, |f| f.max_attempts.max(0) as u32);
    let shown_notify_device = form.map_or(settings.notify_new_device, |f| f.notify_new_device);
    let shown_notify_login = form.map_or(settings.notify_new_login, |f| f.notify_new_login);
    let shown_notify_key = form.map_or(settings.notify_api_key_created, |f| {
        f.notify_api_key_created
    });
    let shown_paused = form.map_or(settings.paused, |f| f.paused);

    let tpl = EmailTemplate {
        urls: ctx.urls,
        t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: ctx.csrf_token,
        active_page: "sysadmin_email",
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,

        config_enabled: settings.enabled,
        ready: settings.ready(),
        paused: shown_paused,
        from_config: settings.origin == SettingsOrigin::Config,
        plaintext: settings.tls.is_plaintext(),
        password_set: settings.password_set,
        password_broken: settings.password_broken,
        missing_text: {
            let labels = missing_labels(t, &settings.missing());
            (!labels.is_empty()).then(|| labels.join(", "))
        },
        updated_at_ts: (settings.updated_at > 0).then_some(settings.updated_at),
        summary: MailSummary {
            pending: count_of(Status::Pending),
            sent: count_of(Status::Sent),
            failed: count_of(Status::Failed),
        },

        host: shown_host,
        port: shown_port,
        tls: shown_tls,
        tls_options: [
            (TlsMode::StartTls, "admin.email_tls_starttls"),
            (TlsMode::ImplicitTls, "admin.email_tls_tls"),
            (TlsMode::None, "admin.email_tls_none"),
        ]
        .into_iter()
        .map(|(mode, key)| TlsOption {
            id: mode.id(),
            label: t.tr(key).to_string(),
        })
        .collect(),
        username: shown_username,
        from_address: shown_from_address,
        from_name: shown_from_name,
        timeout_secs: shown_timeout,
        max_attempts: shown_attempts,
        notify_new_device: shown_notify_device,
        notify_api_key_created: shown_notify_key,
        notify_new_login: shown_notify_login,

        messages: rows,
        status_filter: query.status.clone().unwrap_or_default(),
        filters: [
            ("", "admin.email_filter_all"),
            ("pending", "admin.email_status_pending"),
            ("sent", "admin.email_status_sent"),
            ("failed", "admin.email_status_failed"),
        ]
        .into_iter()
        .map(|(id, key)| FilterOption {
            id,
            label: t.tr(key).to_string(),
        })
        .collect(),
        page,
        has_previous: page > 0,
        has_next,
        drain_available: state.mail.config_enabled(),
        test_to: test_to.unwrap_or_default(),

        error,
        success,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

// ─── POST handlers ────────────────────────────────────────────────────────

fn flag(form: &HashMap<String, String>, name: &str) -> bool {
    form.get(name)
        .is_some_and(|value| matches!(value.as_str(), "on" | "1" | "true"))
}

fn number<T: std::str::FromStr>(form: &HashMap<String, String>, name: &str) -> Option<T> {
    form.get(name)
        .map(|value| value.trim())
        .and_then(|value| value.parse().ok())
}

/// Parse the settings form into the update the repository takes.
///
/// Kept free of the request so the three-way password rule (absent = keep,
/// empty = clear, present = replace) can be tested directly — it is the one
/// field whose absence and whose emptiness mean different things.
fn parse_settings_form(form: &HashMap<String, String>) -> Result<EmailSettingsUpdate, AppError> {
    let password = if flag(form, "clear_password") {
        Some(None)
    } else {
        match form.get("password").map(|value| value.as_str()) {
            // Left blank: the form never received the stored secret, so a blank
            // field means "do not change it", not "erase it".
            Some("") | None => None,
            Some(value) => Some(Some(value.to_string())),
        }
    };

    Ok(EmailSettingsUpdate {
        paused: flag(form, "paused"),
        host: form.get("host").cloned().unwrap_or_default(),
        port: number(form, "port").unwrap_or(587),
        tls: form
            .get("tls")
            .cloned()
            .unwrap_or_else(|| TlsMode::StartTls.id().to_string()),
        username: form.get("username").cloned().unwrap_or_default(),
        password,
        from_address: form.get("from_address").cloned().unwrap_or_default(),
        from_name: form.get("from_name").cloned().unwrap_or_default(),
        timeout_secs: number(form, "timeout_secs").unwrap_or(10),
        max_attempts: number(form, "max_attempts").unwrap_or(5),
        notify_new_device: flag(form, "notify_new_device"),
        notify_api_key_created: flag(form, "notify_api_key_created"),
        notify_new_login: flag(form, "notify_new_login"),
        updated_by: None,
    })
}

async fn require_admin_csrf(
    state: &Arc<AppState>,
    user: &WebUser,
    form: &HashMap<String, String>,
) -> Result<(), AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    crate::service::auth::csrf::check_form_csrf(
        state,
        &user.session_token,
        form.get("csrf_token").map(String::as_str),
    )
}

/// POST /sysadmin/email/settings/ — save the SMTP settings and switches.
pub async fn save_settings(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    let query = EmailQuery {
        action: None,
        status: form.get("status").cloned().filter(|v| !v.is_empty()),
        page: form.get("page").and_then(|value| value.parse().ok()),
    };

    let update = parse_settings_form(&form)?;
    // Cloned for the failure path, which re-renders what was submitted.
    match state.mail.save(update.clone(), Some(user.user_id)).await {
        Ok(_) => Ok((
            StatusCode::FOUND,
            [("Location", "/sysadmin/email/?action=saved")],
        )
            .into_response()),
        // Re-render with the reason: a rejected value (a bad port, an invalid
        // sender) has to be visible on the form that produced it.
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            render_page(&state, &user, &query, Some(msg), None, None, Some(update)).await
        }
    }
}

/// POST /sysadmin/email/test/ — send one message and report the outcome.
pub async fn send_test(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    let query = EmailQuery {
        action: None,
        status: form.get("status").cloned().filter(|v| !v.is_empty()),
        page: form.get("page").and_then(|value| value.parse().ok()),
    };
    let to = form.get("to").cloned().unwrap_or_default();

    match state.mail.send_test(&to, user.language.as_deref()).await {
        Ok(()) => Ok((
            StatusCode::FOUND,
            [("Location", "/sysadmin/email/?action=tested")],
        )
            .into_response()),
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            render_page(&state, &user, &query, Some(msg), None, Some(to), None).await
        }
    }
}

/// POST /sysadmin/email/drain/ — run the delivery task immediately.
pub async fn drain_now(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    state.scheduler.trigger_now(mail::TASK_NAME).await;
    Ok((
        StatusCode::FOUND,
        [("Location", "/sysadmin/email/?action=drained")],
    )
        .into_response())
}

/// POST /sysadmin/email/{id}/retry/ — put a failed message back in the queue.
pub async fn retry_message(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    let query = EmailQuery {
        action: None,
        status: form.get("status").cloned().filter(|v| !v.is_empty()),
        page: form.get("page").and_then(|value| value.parse().ok()),
    };

    if state.mail.retry(id).await? {
        return Ok((
            StatusCode::FOUND,
            [("Location", "/sysadmin/email/?action=retried")],
        )
            .into_response());
    }
    let msg = I18n::get(user.language.as_deref())
        .tr("admin.email_not_retryable")
        .to_string();
    render_page(&state, &user, &query, Some(msg), None, None, None).await
}

/// POST /sysadmin/email/{id}/delete/ — drop one outbox row.
pub async fn delete_message(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    if !state.repos.email_message.delete_by_id(id).await? {
        let query = EmailQuery {
            action: None,
            status: form.get("status").cloned().filter(|v| !v.is_empty()),
            page: form.get("page").and_then(|value| value.parse().ok()),
        };
        let msg = I18n::get(user.language.as_deref())
            .tr("admin.email_not_found")
            .to_string();
        return render_page(&state, &user, &query, Some(msg), None, None, None).await;
    }
    Ok((
        StatusCode::FOUND,
        [("Location", "/sysadmin/email/?action=deleted")],
    )
        .into_response())
}

/// POST /sysadmin/email/clear/ — drop every finished row.
pub async fn clear_finished(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    state.repos.email_message.delete_finished().await?;
    Ok((
        StatusCode::FOUND,
        [("Location", "/sysadmin/email/?action=cleared")],
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_blank_password_keeps_the_stored_one_and_clear_erases_it() {
        let blank = parse_settings_form(&form(&[("host", "smtp"), ("password", "")])).unwrap();
        assert_eq!(blank.password, None, "blank means unchanged");

        let absent = parse_settings_form(&form(&[("host", "smtp")])).unwrap();
        assert_eq!(absent.password, None);

        let typed = parse_settings_form(&form(&[("password", " hunter2 ")])).unwrap();
        assert_eq!(typed.password, Some(Some(" hunter2 ".to_string())));

        let cleared =
            parse_settings_form(&form(&[("password", "hunter2"), ("clear_password", "on")]))
                .unwrap();
        assert_eq!(
            cleared.password,
            Some(None),
            "the checkbox wins over the field"
        );
    }

    #[test]
    fn checkbox_switches_default_to_off_when_absent_but_accept_the_usual_values() {
        let parsed = parse_settings_form(&form(&[
            ("notify_new_login", "on"),
            ("notify_new_device", "1"),
            ("paused", "true"),
        ]))
        .unwrap();
        assert!(parsed.notify_new_login);
        assert!(parsed.notify_new_device);
        assert!(!parsed.notify_api_key_created, "an unchecked box is absent");
        assert!(parsed.paused);
    }

    #[test]
    fn numbers_fall_back_to_the_defaults_when_blank_or_unparsable() {
        let parsed = parse_settings_form(&form(&[("port", ""), ("timeout_secs", "abc")])).unwrap();
        assert_eq!(parsed.port, 587);
        assert_eq!(parsed.timeout_secs, 10);
        assert_eq!(parsed.max_attempts, 5);

        let parsed =
            parse_settings_form(&form(&[("port", "2525"), ("max_attempts", "9")])).unwrap();
        assert_eq!(parsed.port, 2525);
        assert_eq!(parsed.max_attempts, 9);
    }

    #[test]
    fn the_tls_picker_defaults_to_encryption() {
        let parsed = parse_settings_form(&form(&[("host", "smtp")])).unwrap();
        assert_eq!(parsed.tls, TlsMode::StartTls.id());
    }

    #[tokio::test]
    async fn unknown_actions_render_no_banner() {
        let t = I18n::get(Some("en"));
        assert!(success_message(t, Some("saved")).is_some());
        assert!(success_message(t, Some("../../etc/passwd")).is_none());
        assert!(success_message(t, None).is_none());
    }
}
