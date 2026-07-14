/// Admin Web UI — task management (view/trigger all scheduled tasks).
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use std::sync::Arc;

use crate::AppState;
use crate::scheduler::{TaskKind, TaskMetrics};
use base::error::AppError;

use super::auth_extractor::WebUser;

fn format_ts(ts: Option<i64>) -> String {
    match ts {
        Some(t) => {
            let dt = chrono::DateTime::from_timestamp(t, 0).unwrap_or_default();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        }
        None => "Never".to_string(),
    }
}

#[derive(Template)]
#[template(path = "admintasks/list.html")]
pub struct AdmintasksTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: Option<String>,
    pub active_page: &'static str,
    pub tasks: Vec<TaskRow>,
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
    pub last_run_at: String,
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
        last_run_at: format_ts(metrics.last_run_at),
        last_duration_ms: metrics.last_duration_ms,
        last_success_message: metrics.last_success_message.clone(),
        last_error_message: metrics.last_error_message.clone(),
        total_processed: metrics.total_processed,
        can_trigger: !matches!(kind, TaskKind::Continuous),
    }
}

/// GET /sysadmin/tasks/ — list all scheduled tasks (admin only).
pub async fn task_list_page(user: WebUser, State(state): State<Arc<AppState>>) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }

    // Collect metrics from all scheduler handles.
    let handles = state.scheduler.handles();
    let mut tasks = Vec::with_capacity(handles.len());
    for handle in handles {
        let metrics = handle.metrics().await;
        tasks.push(to_task_row(handle.name, &handle.kind, &metrics));
    }

    let csrf_token = Some(crate::service::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));

    let left_panel_repos = match crate::service::repo::service::load_left_panel_repos(
        &state.repos,
        user.user_id,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return AppError::internal(e.to_string()).into_response(),
    };

    let tpl = AdmintasksTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        csrf_token,
        active_page: "admintasks",
        tasks,
        left_panel_repos,
        current_repo_id: None,
    };

    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => AppError::internal(e.to_string()).into_response(),
    }
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

    if !state.scheduler.trigger_now(&name).await {
        return Err(AppError::NotFound(format!("Task '{name}' not found")));
    }

    Ok((StatusCode::FOUND, [("Location", "/sysadmin/tasks/")]).into_response())
}
