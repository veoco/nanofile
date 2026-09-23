/// Admin Web UI — job management (view/trigger every background job).
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::tasks::spec::{JobKey, Priority, Trigger};
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "admintasks/list.html")]
pub struct AdmintasksTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: Option<String>,
    pub active_page: &'static str,
    pub tasks: Vec<TaskRow>,
    pub error: Option<String>,
    pub success: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// One row of the listing: a job, or a long-lived service.
pub struct TaskRow {
    /// Stable slug, used in the trigger URL and as the row's DOM handle.
    pub name: String,
    /// Human-readable label shown in the listing.
    pub display_name: String,
    /// Whether a manual trigger has a meaning. An on-demand job needs
    /// parameters a bare button cannot supply, and a service never finishes.
    pub triggerable: bool,
    pub kind_label: String,
    pub priority_label: String,
    pub interval_secs_display: String,
    pub concurrency: usize,
    pub run_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub last_run_at_ts: Option<i64>,
    pub last_duration_ms: u64,
    pub last_success_message: String,
    pub last_error_message: String,
    pub total_processed: u64,
}

/// Build a display label for the interval.
fn interval_display(trigger: Trigger) -> String {
    match trigger {
        Trigger::Periodic { interval_secs, .. } if interval_secs >= 3600 => {
            format!("{}h", interval_secs / 3600)
        }
        Trigger::Periodic { interval_secs, .. } if interval_secs >= 60 => {
            format!("{}m", interval_secs / 60)
        }
        Trigger::Periodic { interval_secs, .. } => format!("{interval_secs}s"),
        Trigger::Manual | Trigger::StartupOnly | Trigger::OnDemand => "—".to_string(),
    }
}

fn kind_label(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Periodic { .. } => "periodic",
        Trigger::Manual | Trigger::StartupOnly => "manual",
        Trigger::OnDemand => "on_demand",
    }
}

fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::Interactive => "interactive",
        Priority::Normal => "normal",
        Priority::Background => "background",
    }
}

/// Row for a long-lived service, which has counters of no kind.
fn service_row(name: &str) -> TaskRow {
    TaskRow {
        name: name.to_string(),
        display_name: name.to_string(),
        triggerable: false,
        kind_label: "service".to_string(),
        priority_label: String::new(),
        interval_secs_display: "—".to_string(),
        concurrency: 1,
        run_count: 0,
        success_count: 0,
        error_count: 0,
        last_run_at_ts: None,
        last_duration_ms: 0,
        last_success_message: String::new(),
        last_error_message: String::new(),
        total_processed: 0,
    }
}

/// What a POST redirect lands with, so the page can confirm the action. An
/// unrecognised value — a hand-typed URL, a stale bookmark — renders no banner
/// rather than echoing the value back into the page.
#[derive(Deserialize)]
pub struct TasksQuery {
    pub action: Option<String>,
}

/// GET /sysadmin/tasks/ — list every background job (admin only).
pub async fn task_list_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<TasksQuery>,
) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }

    let success = match query.action.as_deref() {
        Some("triggered") => Some(
            I18n::get(user.language.as_deref())
                .tr("admin.task_triggered")
                .to_string(),
        ),
        _ => None,
    };

    match render_page(&state, &user, None, success).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// Build and render the listing, carrying at most one banner.
async fn render_page(
    state: &Arc<AppState>,
    user: &WebUser,
    error: Option<String>,
    success: Option<String>,
) -> Result<Response, AppError> {
    let mut tasks: Vec<TaskRow> = state
        .tasks
        .jobs()
        .iter()
        .map(|job| {
            let spec = &job.spec;
            let stats = state.tasks.stats(spec.key);
            TaskRow {
                name: spec.key.as_str().to_string(),
                display_name: spec.name.to_string(),
                // A manual or periodic job can be started with no input; an
                // on-demand job needs parameters a bare button cannot supply.
                triggerable: matches!(spec.trigger, Trigger::Periodic { .. } | Trigger::Manual),
                kind_label: kind_label(spec.trigger).to_string(),
                priority_label: priority_label(spec.priority).to_string(),
                interval_secs_display: interval_display(spec.trigger),
                concurrency: spec.max_concurrent,
                run_count: stats.run_count,
                success_count: stats.success_count,
                error_count: stats.error_count,
                last_run_at_ts: stats.last_run_at,
                last_duration_ms: stats.last_duration_ms,
                last_success_message: stats.last_success_message,
                last_error_message: stats.last_error_message,
                total_processed: stats.total_processed,
            }
        })
        .collect();
    // Services are listed too: they run in the same system and an administrator
    // looking at this page wants to know they exist.
    tasks.extend(state.tasks.services().iter().map(|name| service_row(name)));

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    let tpl = AdmintasksTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: Some(ctx.csrf_token),
        active_page: "admintasks",
        tasks,
        error,
        success,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// POST /sysadmin/tasks/{name}/trigger/ — run a job immediately.
///
/// The run is queued, not executed here: the old scheduler ran the whole job
/// inside this request, so triggering GC meant holding an administrator's HTTP
/// request open for its entire duration.
pub async fn trigger_task(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    // `false` means no job by that slug is registered: re-render the list with
    // the reason rather than answering a browser form with the API's JSON error.
    let Some(key) = JobKey::ALL
        .iter()
        .copied()
        .find(|key| key.as_str() == name)
        .filter(|key| state.tasks.job(*key).is_some())
    else {
        let msg =
            I18n::get(user.language.as_deref()).trf("admin.task_not_found", &[("name", &name)]);
        return render_page(&state, &user, Some(msg), None).await;
    };

    if !matches!(
        state.tasks.job(key).map(|job| job.spec.trigger),
        Some(Trigger::Periodic { .. }) | Some(Trigger::Manual)
    ) {
        let msg = I18n::get(user.language.as_deref())
            .trf("admin.task_not_triggerable", &[("name", &name)]);
        return render_page(&state, &user, Some(msg), None).await;
    }

    if let Err(e) = state.tasks.submit(
        key,
        None,
        serde_json::Value::Null,
        state
            .tasks
            .job(key)
            .map(|job| job.spec.name)
            .unwrap_or(&name),
        None,
    ) {
        let msg = I18n::get(user.language.as_deref()).trf(
            "admin.task_trigger_failed",
            &[("name", &name), ("reason", &e.to_string())],
        );
        return render_page(&state, &user, Some(msg), None).await;
    }

    Ok((
        StatusCode::FOUND,
        [("Location", "/sysadmin/tasks/?action=triggered")],
    )
        .into_response())
}
