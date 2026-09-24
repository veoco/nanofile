/// Admin Web UI — the task pages: what is running, and what is registered.
///
/// Two pages rather than one. The run list is about this moment — what is
/// running, what finished, how busy the server is — while the registry is about
/// the jobs themselves, which only change when the server does. They were one
/// page, and the result mixed a flat list of jobs and services with the history
/// of their runs and a load panel in between.
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
use crate::tasks::spec::{JobKey, Priority, ServiceKey, Trigger};
use base::error::AppError;

use super::auth_extractor::WebUser;

/// `/sysadmin/tasks/` — the runs, and how busy the server is.
#[derive(Template)]
#[template(path = "admintasks/runs.html")]
pub struct RunsTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: Option<String>,
    pub active_page: &'static str,
    pub runs: Vec<RunRow>,
    pub load: LoadRow,
    pub error: Option<String>,
    pub success: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// `/sysadmin/tasks/registered/` — every job and service this server runs.
#[derive(Template)]
#[template(path = "admintasks/registered.html")]
pub struct RegisteredTasksTemplate {
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

/// The measurements load-aware deferral acts on, shown so an administrator can
/// see why a job is waiting — and calibrate the thresholds before trusting them.
pub struct LoadRow {
    pub inflight_requests: u64,
    pub background_active: u64,
    pub db_in_use: u64,
    pub db_max: u64,
    pub worker_busy_pct: u32,
    pub queue_depth: u64,
    pub alive_tasks: u64,
    /// Whether the sample is recent enough to act on. A stopped sampler must
    /// never read as a quiet server.
    pub fresh: bool,
    pub load_aware: bool,
}

impl LoadRow {
    fn from_state(state: &Arc<AppState>) -> Self {
        let snapshot = state.tasks.load_snapshot();
        let now = chrono::Utc::now().timestamp();
        Self {
            inflight_requests: snapshot.inflight_requests,
            background_active: snapshot.background_active,
            db_in_use: snapshot.db_in_use,
            db_max: snapshot.db_max,
            worker_busy_pct: snapshot.worker_busy_pct,
            queue_depth: snapshot.queue_depth,
            alive_tasks: snapshot.alive_tasks,
            fresh: state
                .tasks
                .load()
                .is_fresh(std::time::Duration::from_secs(30), now),
            load_aware: state.tasks.load_aware(),
        }
    }
}

/// One recorded run, from the durable journal.
pub struct RunRow {
    pub kind: String,
    pub state: String,
    pub owner: String,
    pub finished_at_ts: Option<i64>,
    pub processed: Option<i64>,
    pub error: String,
}

/// One row of the listing: a job, or a long-lived service.
pub struct TaskRow {
    /// Stable slug, used in the trigger URL and as the row's DOM handle.
    pub name: String,
    /// Human-readable label shown in the listing.
    pub display_name: String,
    /// One sentence on what the row actually does, in the reader's language.
    pub desc: String,
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

/// A locale key of the form `admin.job_<slug>[_desc]` or
/// `admin.service_<slug>[_desc]`.
///
/// Derived from the stable slug rather than tabulated here, exactly as the
/// settings catalog derives `setting.<key>`: one place decides how a name is
/// spelled, and the coverage test walks the declared keys, so a job cannot be
/// added without a name in every language.
fn job_key(key: JobKey, suffix: &str) -> String {
    format!("admin.job_{}{suffix}", key.as_str().replace('-', "_"))
}

fn service_key(key: ServiceKey, suffix: &str) -> String {
    format!("admin.service_{}{suffix}", key.as_str().replace('-', "_"))
}

fn job_name_key(key: JobKey) -> String {
    job_key(key, "")
}

fn job_desc_key(key: JobKey) -> String {
    job_key(key, "_desc")
}

fn service_name_key(key: ServiceKey) -> String {
    service_key(key, "")
}

fn service_desc_key(key: ServiceKey) -> String {
    service_key(key, "_desc")
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

/// The label naming how a job comes to run.
///
/// `StartupOnly` needs a label of its own: folding it into "manual" told the
/// reader a job never runs on its own while it in fact runs at every start.
fn trigger_label_key(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Periodic { .. } => "admin.task_periodic",
        Trigger::Manual => "admin.task_manual",
        Trigger::StartupOnly => "admin.task_startup",
        Trigger::OnDemand => "admin.task_on_demand",
    }
}

fn priority_label_key(priority: Priority) -> &'static str {
    match priority {
        Priority::Interactive => "admin.task_priority_interactive",
        Priority::Normal => "admin.task_priority_normal",
        Priority::Background => "admin.task_priority_background",
    }
}

/// Row for a long-lived service, which has counters of no kind.
fn service_row(key: ServiceKey, t: &I18n) -> TaskRow {
    TaskRow {
        name: key.as_str().to_string(),
        display_name: t.tr(&service_name_key(key)).to_string(),
        desc: t.tr(&service_desc_key(key)).to_string(),
        triggerable: false,
        kind_label: t.tr("admin.task_service").to_string(),
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

/// GET /sysadmin/tasks/ — what has been running (admin only).
pub async fn runs_page(user: WebUser, State(state): State<Arc<AppState>>) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }
    match render_runs(&state, &user).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// GET /sysadmin/tasks/registered/ — every job and service (admin only).
pub async fn registered_page(
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

    match render_registered(&state, &user, None, success).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// Build and render the run list.
async fn render_runs(state: &Arc<AppState>, user: &WebUser) -> Result<Response, AppError> {
    // Durable history, so a run that crashed is visible and not only the most
    // recent in-memory state.
    let runs = state
        .repos
        .job_run
        .recent(20)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| RunRow {
            kind: row.kind,
            state: row.phase,
            owner: row
                .owner
                .map(|id| id.to_string())
                .unwrap_or_else(|| "—".to_string()),
            finished_at_ts: row.finished_at,
            processed: row.processed,
            error: row.error.unwrap_or_default(),
        })
        .collect();

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    let tpl = RunsTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: Some(ctx.csrf_token),
        active_page: "admintasks",
        runs,
        load: LoadRow::from_state(state),
        error: None,
        success: None,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// Build and render the registry, carrying at most one banner.
async fn render_registered(
    state: &Arc<AppState>,
    user: &WebUser,
    error: Option<String>,
    success: Option<String>,
) -> Result<Response, AppError> {
    let t = I18n::get(user.language.as_deref());
    let mut tasks: Vec<TaskRow> = state
        .tasks
        .jobs()
        .iter()
        .map(|job| {
            let spec = &job.spec;
            let stats = state.tasks.stats(spec.key);
            TaskRow {
                name: spec.key.as_str().to_string(),
                display_name: t.tr(&job_name_key(spec.key)).to_string(),
                desc: t.tr(&job_desc_key(spec.key)).to_string(),
                // A manual or periodic job can be started with no input; an
                // on-demand job needs parameters a bare button cannot supply.
                triggerable: matches!(spec.trigger, Trigger::Periodic { .. } | Trigger::Manual),
                kind_label: t.tr(trigger_label_key(spec.trigger)).to_string(),
                priority_label: t.tr(priority_label_key(spec.priority)).to_string(),
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
    tasks.extend(
        state
            .tasks
            .services()
            .iter()
            .map(|key| service_row(*key, t)),
    );

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    let tpl = RegisteredTasksTemplate {
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

    // No job by that slug is registered: re-render the registry with the
    // reason rather than answering a browser form with the API's JSON error.
    let Some(key) = JobKey::ALL
        .iter()
        .copied()
        .find(|key| key.as_str() == name)
        .filter(|key| state.tasks.job(*key).is_some())
    else {
        let msg =
            I18n::get(user.language.as_deref()).trf("admin.task_not_found", &[("name", &name)]);
        return render_registered(&state, &user, Some(msg), None).await;
    };

    if !matches!(
        state.tasks.job(key).map(|job| job.spec.trigger),
        Some(Trigger::Periodic { .. }) | Some(Trigger::Manual)
    ) {
        let msg = I18n::get(user.language.as_deref())
            .trf("admin.task_not_triggerable", &[("name", &name)]);
        return render_registered(&state, &user, Some(msg), None).await;
    }

    if let Err(e) = state
        .tasks
        .submit(
            key,
            None,
            serde_json::Value::Null,
            state
                .tasks
                .job(key)
                .map(|job| job.spec.name)
                .unwrap_or(&name),
            None,
        )
        .await
    {
        let msg = I18n::get(user.language.as_deref()).trf(
            "admin.task_trigger_failed",
            &[("name", &name), ("reason", &e.to_string())],
        );
        return render_registered(&state, &user, Some(msg), None).await;
    }

    Ok((
        StatusCode::FOUND,
        [("Location", "/sysadmin/tasks/registered/?action=triggered")],
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The languages the server can render, as `I18n::get` resolves them.
    fn languages() -> [&'static I18n; 2] {
        [I18n::get(None), I18n::get(Some("zh"))]
    }

    /// Every job the catalog declares must have a name and a description in
    /// every language. Derived keys mean a new job silently gets no label, and
    /// `tr` falls back to the key, so the page would put "admin.job_foo" in
    /// front of an operator.
    #[test]
    fn every_job_is_named_and_described_in_every_language() {
        for key in JobKey::ALL {
            for locale_key in [
                job_name_key(*key),
                job_desc_key(*key),
                trigger_label_key(crate::tasks::catalog::policy(*key).trigger).to_string(),
            ] {
                for t in languages() {
                    assert_ne!(
                        t.tr(&locale_key),
                        locale_key,
                        "{} is missing for {}",
                        locale_key,
                        t.lang
                    );
                }
            }
        }
    }

    /// The same for the long-lived services, which are not jobs and so are not
    /// covered by the catalog walk above.
    #[test]
    fn every_service_is_named_and_described_in_every_language() {
        for key in ServiceKey::ALL {
            for locale_key in [service_name_key(*key), service_desc_key(*key)] {
                for t in languages() {
                    assert_ne!(
                        t.tr(&locale_key),
                        locale_key,
                        "{} is missing for {}",
                        locale_key,
                        t.lang
                    );
                }
            }
        }
    }

    /// The derived key is the contract between a slug and its label, so pin the
    /// shape rather than trusting the format string.
    #[test]
    fn a_slug_becomes_a_flat_locale_key() {
        assert_eq!(job_name_key(JobKey::GarbageCollection), "admin.job_gc");
        assert_eq!(
            job_desc_key(JobKey::TokenExpiryCheck),
            "admin.job_token_expiry_check_desc"
        );
        assert_eq!(
            service_name_key(ServiceKey::EventListener),
            "admin.service_event_listener"
        );
    }

    /// A startup job used to be labelled "manual", which told the reader it
    /// never runs on its own while it in fact runs at every start.
    #[test]
    fn a_startup_job_is_not_labelled_manual() {
        assert_ne!(
            trigger_label_key(Trigger::StartupOnly),
            trigger_label_key(Trigger::Manual)
        );
        assert_eq!(
            I18n::get(None).tr(trigger_label_key(Trigger::StartupOnly)),
            "Startup"
        );
    }
}
