//! Admin Web UI — email management: the outbox, the delivery state and the test
//! message.
//!
//! The *configuration* moved to `/sysadmin/settings/email/` when the settings
//! pages arrived, so this page does one thing: it shows what this server has
//! tried to send, why anything is stuck, and lets an administrator probe
//! delivery. The settings link at the top is the only way to change how mail is
//! sent, which also means an operator cannot be looking at a second, stale copy
//! of the SMTP host on this page.

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
use crate::service::mail::MailKind;
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
    /// The master switch, `[email] enabled`.
    pub config_enabled: bool,
    /// Everything needed for delivery is in place (and the master switch is on).
    pub ready: bool,
    pub paused: bool,
    /// Where the SMTP settings come from: `environment`, `config`, `database`,
    /// `default`.
    pub settings_origin: &'static str,
    /// The variable or file key the settings come from, when there is one.
    pub settings_origin_detail: String,
    /// `tls = "none"`: credentials and message bodies travel in the clear.
    pub plaintext: bool,
    /// Missing settings, already translated and joined. `None` when the
    /// configuration is complete.
    pub missing_text: Option<String>,
    /// A saved setting is waiting for the next start.
    pub restart_pending: bool,
    pub summary: MailSummary,

    // ── Outbox ────────────────────────────────────────────────────
    pub messages: Vec<MailRow>,
    /// `""` (all), `pending`, `sent` or `failed`.
    pub status_filter: String,
    /// The status filter buttons, including the "everything" one.
    pub filters: Vec<FilterOption>,
    pub page: u64,
    pub has_previous: bool,
    pub has_next: bool,
    /// Whether the drainer task is registered (it always is).
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

/// GET /sysadmin/email/ — delivery state, outbox and the test message.
pub async fn email_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<EmailQuery>,
) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }
    let success = success_message(I18n::get(user.language.as_deref()), query.action.as_deref());
    match render_page(&state, &user, &query, None, success, None).await {
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
) -> Result<Response, AppError> {
    let t = I18n::get(user.language.as_deref());
    let settings = state.mail.settings();

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

    // Where the SMTP settings come from is the one thing this page still has to
    // say about configuration: everything else lives on the settings page.
    let resolved = state.settings.resolved_one("email.host");
    let settings_origin = resolved
        .as_ref()
        .map(|entry| entry.origin.id())
        .unwrap_or("default");
    let settings_origin_detail = match resolved.as_ref().map(|entry| entry.origin) {
        Some(infra::settings::Origin::Environment { var }) => var.to_string(),
        Some(infra::settings::Origin::ConfigFile { key }) => format!("config.toml: {key}"),
        _ => String::new(),
    };

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
        paused: settings.paused,
        settings_origin,
        settings_origin_detail,
        plaintext: settings.tls.is_plaintext(),
        missing_text: {
            let labels = missing_labels(t, &settings.missing());
            (!labels.is_empty()).then(|| labels.join(", "))
        },
        restart_pending: state
            .settings
            .pending_restart()
            .iter()
            .any(|key| key.starts_with("email.")),
        summary: MailSummary {
            pending: count_of(Status::Pending),
            sent: count_of(Status::Sent),
            failed: count_of(Status::Failed),
        },

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
        // Registered unconditionally, so the button is always honest.
        drain_available: true,
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

fn query_from(form: &HashMap<String, String>) -> EmailQuery {
    EmailQuery {
        action: None,
        status: form.get("status").cloned().filter(|v| !v.is_empty()),
        page: form.get("page").and_then(|value| value.parse().ok()),
    }
}

/// POST /sysadmin/email/test/ — send one message and report the outcome.
pub async fn send_test(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin_csrf(&state, &user, &form).await?;
    let query = query_from(&form);
    let to = form.get("to").cloned().unwrap_or_default();

    match state.mail.send_test(&to, user.language.as_deref()).await {
        Ok(()) => Ok((
            StatusCode::FOUND,
            [("Location", "/sysadmin/email/?action=tested")],
        )
            .into_response()),
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            render_page(&state, &user, &query, Some(msg), None, Some(to)).await
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
    // Queued rather than run here: the old scheduler ran the whole drain inside
    // this request.
    state.tasks.submit(
        crate::tasks::spec::JobKey::MailDelivery,
        None,
        serde_json::Value::Null,
        "mail delivery",
        None,
    )?;
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
    let query = query_from(&form);

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
    render_page(&state, &user, &query, Some(msg), None, None).await
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
        let query = query_from(&form);
        let msg = I18n::get(user.language.as_deref())
            .tr("admin.email_not_found")
            .to_string();
        return render_page(&state, &user, &query, Some(msg), None, None).await;
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

    #[test]
    fn unknown_actions_render_no_banner() {
        let t = I18n::get(Some("en"));
        assert!(success_message(t, Some("tested")).is_some());
        assert!(success_message(t, Some("drained")).is_some());
        // The settings form moved out of this page, so its confirmation is gone.
        assert!(success_message(t, Some("saved")).is_none());
        assert!(success_message(t, Some("../../etc/passwd")).is_none());
        assert!(success_message(t, None).is_none());
    }

    #[test]
    fn missing_fields_are_translated_one_by_one() {
        let t = I18n::get(Some("en"));
        let labels = missing_labels(t, &["host", "from_address"]);
        assert_eq!(labels.len(), 2);
        assert_ne!(labels[0], "admin.email_field_host");
        assert_ne!(labels[1], "admin.email_field_from_address");
    }

    #[test]
    fn the_query_is_round_tripped_through_the_form() {
        let form: HashMap<String, String> = [
            ("status".to_string(), "failed".to_string()),
            ("page".to_string(), "3".to_string()),
        ]
        .into_iter()
        .collect();
        let query = query_from(&form);
        assert_eq!(query.status.as_deref(), Some("failed"));
        assert_eq!(query.page, Some(3));
    }
}
