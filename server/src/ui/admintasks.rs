/// Admin Web UI — task management (view/trigger all scheduled tasks).
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
use crate::scheduler::{TaskKind, TaskMetrics};
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

pub struct TaskRow {
    pub name: String,
    pub kind_label: String,
    pub interval_secs_display: String,
    pub run_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub last_run_at_ts: Option<i64>,
    pub last_duration_ms: u64,
    pub last_success_message: String,
    pub last_error_message: String,
    pub total_processed: u64,
    pub can_trigger: bool,
}

/// Build a display label for the task interval.
fn interval_display(kind: &TaskKind) -> String {
    match kind {
        TaskKind::Periodic { interval_secs } if *interval_secs >= 3600 => {
            format!("{}h", interval_secs / 3600)
        }
        TaskKind::Periodic { interval_secs } if *interval_secs >= 60 => {
            format!("{}m", interval_secs / 60)
        }
        TaskKind::Periodic { interval_secs } => format!("{}s", interval_secs),
        TaskKind::Continuous | TaskKind::Manual => "—".to_string(),
    }
}

/// Build a display label for the task kind.
fn kind_label(kind: &TaskKind) -> &'static str {
    match kind {
        TaskKind::Periodic { .. } => "Periodic",
        TaskKind::Continuous => "Continuous",
        TaskKind::Manual => "Manual",
    }
}

/// Convert TaskMetrics + metadata into a template-friendly row.
fn to_task_row(name: &str, kind: &TaskKind, metrics: &TaskMetrics) -> TaskRow {
    TaskRow {
        name: name.to_string(),
        kind_label: kind_label(kind).to_string(),
        interval_secs_display: interval_display(kind),
        run_count: metrics.run_count,
        success_count: metrics.success_count,
        error_count: metrics.error_count,
        last_run_at_ts: metrics.last_run_at,
        last_duration_ms: metrics.last_duration_ms,
        last_success_message: metrics.last_success_message.clone(),
        last_error_message: metrics.last_error_message.clone(),
        total_processed: metrics.total_processed,
        can_trigger: !matches!(kind, TaskKind::Continuous),
    }
}

/// What a POST redirect lands with, so the page can confirm the action. An
/// unrecognised value — a hand-typed URL, a stale bookmark — renders no banner
/// rather than echoing the value back into the page.
#[derive(Deserialize)]
pub struct TasksQuery {
    pub action: Option<String>,
}

/// GET /sysadmin/tasks/ — list all scheduled tasks (admin only).
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

/// Build and render the task list, carrying at most one banner.
async fn render_page(
    state: &Arc<AppState>,
    user: &WebUser,
    error: Option<String>,
    success: Option<String>,
) -> Result<Response, AppError> {
    // Collect metrics from all scheduler handles.
    let handles = state.scheduler.handles();
    let mut tasks = Vec::with_capacity(handles.len());
    for handle in handles {
        let metrics = handle.metrics().await;
        tasks.push(to_task_row(handle.name, &handle.kind, &metrics));
    }

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

/// POST /sysadmin/tasks/{name}/trigger/ — trigger a periodic task immediately.
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

    // `false` means no task by that name is registered: re-render the list with
    // the reason rather than answering a browser form with the API's JSON error.
    if !state.scheduler.trigger_now(&name).await {
        let msg =
            I18n::get(user.language.as_deref()).trf("admin.task_not_found", &[("name", &name)]);
        return render_page(&state, &user, Some(msg), None).await;
    }

    Ok((
        StatusCode::FOUND,
        [("Location", "/sysadmin/tasks/?action=triggered")],
    )
        .into_response())
}
