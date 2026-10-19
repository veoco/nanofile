/// Admin Web UI — the task pages: what is running, and what is registered.
/// Two pages rather than one: the run list is about this moment, the registry
/// about the jobs themselves, which only change when the server does.
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use infra::entity::job_run;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::repository::job_run::{RunHistoryFilter, RunOutcome};
use crate::tasks::JobStats;
use crate::tasks::registry::RegisteredJob;
use crate::tasks::run::{JobRun, JobState, Origin};
use crate::tasks::spec::{
    Durability, JobKey, JobSpec, OverlapPolicy, Priority, Resource, RetryPolicy, ServiceKey,
    SkipReason, Trigger,
};
use crate::tasks::store::RunFilter;
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
    pub active: Vec<ActiveRunRow>,
    pub runs: Vec<RunRow>,
    /// The job filter buttons: "everything", then the jobs the journal holds.
    pub job_filters: Vec<FilterChip>,
    pub outcome_filters: Vec<FilterChip>,
    /// Whether a filter is narrowing the list: an empty list then means
    /// "nothing matches" rather than "nothing has happened".
    pub filtered: bool,
    /// How long a run stays readable, in the reader's words: the journal's
    /// policy, not a per-job setting.
    pub retention_hint: String,
    pub load: LoadRow,
    pub error: Option<String>,
    pub success: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub sharing_enabled: bool,
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
    pub groups: Vec<TaskGroup>,
    /// Jobs the catalog declares that this server did not register, with the
    /// reason, so a missing job reads as switched off rather than forgotten.
    pub skipped: Vec<SkippedRow>,
    pub error: Option<String>,
    pub success: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub sharing_enabled: bool,
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

/// One run still in flight, from the in-memory run table: the journal only
/// holds finished runs, so this is the only place a live run appears.
pub struct ActiveRunRow {
    /// The full id, which is what the log line for this run carries.
    pub id: String,
    pub name: String,
    pub state: StateBadge,
    /// The account that submitted it, named. The row's dim line.
    pub owner: String,
    /// How far the run says it has got.
    pub progress: String,
    /// The one time a live run has, which the browser renders.
    pub time_ts: i64,
    /// `started` or `submitted`: which time that is, as a test hook.
    pub time_key: &'static str,
    pub time_label: String,
    /// What the run is doing now, in the job's own words.
    pub message: String,
}

/// One recorded run, from the durable journal.
pub struct RunRow {
    /// The full id, which is what the log line for this run carries.
    pub id: String,
    /// The job's name, or its slug when the job no longer exists.
    pub name: String,
    pub state: StateBadge,
    /// The account that submitted it, named. The row's dim line.
    pub owner: String,
    /// When it finished. The journal only holds finished rows.
    pub finished_ts: Option<i64>,
    /// How long it took, when the row recorded both ends: a run whose start
    /// was never written has no duration, which is not a duration of zero.
    pub duration: Option<String>,
    /// The job's own error, when the run did not succeed: a failure is what
    /// the list is scanned for.
    pub error: Option<String>,
    /// Items the run reported it processed, when it reports one.
    pub processed: Option<i64>,
    /// How many attempts the run took. Only more than one is worth a row.
    pub attempt: i32,
    pub summary: String,
}

/// The one state a row is in, as a badge: class and label travel together, so
/// a state cannot get a colour without a name in the reader's language.
pub struct StateBadge {
    pub class: &'static str,
    pub label: String,
}

/// One labelled policy of a job, shown in the row's disclosure: the decisions
/// an operator would otherwise have to read the catalog to learn.
pub struct DetailRow {
    pub label: String,
    pub value: String,
}

/// The job's own most recent report, shown as written: an operator greps the
/// log for this prose, and translating it would only make it harder to find.
pub struct MessageRow {
    /// Whether the last run failed, which is what decides the colour.
    pub failed: bool,
    pub text: String,
}

/// One row of the registry: a job, or a long-lived service.
pub struct RegisteredRow {
    /// Stable slug, used in the trigger URL and as the row's DOM handle.
    pub slug: String,
    pub name: String,
    /// One sentence on what the row actually does.
    pub desc: String,
    /// Which icon the row carries, from how it comes to run.
    pub icon: &'static str,
    /// `job` or `service`: a service has no counters to show at all.
    pub kind: &'static str,
    /// When it runs, as a sentence rather than a bare interval.
    pub schedule: String,
    /// The verdict of the most recent run. `None` for a job that has never run
    /// and for every service.
    pub state: Option<StateBadge>,
    /// Whether the row is a job that has never run, so it says so, not zeroes.
    pub never_run: bool,
    /// When the job last ran, as a raw stamp the browser renders. `None` for a
    /// service and for a job that has never run.
    pub last_run_ts: Option<i64>,
    pub last_duration: Option<String>,
    /// Lifetime counters for the row's disclosure, empty for a service and for
    /// a never-run job: zero and "no such number" must not look the same.
    pub counters: Vec<DetailRow>,
    /// The policies the job runs under, shown when the row is opened.
    pub details: Vec<DetailRow>,
    /// What the job reported last, if it has ever run.
    pub last_message: Option<MessageRow>,
    /// Which group of the registry the row belongs to, as an index into `GROUP_ORDER`.
    pub group_index: usize,
    /// The interval, when there is one, so the schedule group can be ordered.
    pub interval_secs: Option<u64>,
    /// The job's translated name as a JSON object, built here because a name
    /// with a quote in it would break a JSON literal in the template.
    pub confirm_args: String,
    /// Whether a manual trigger has a meaning. An on-demand job needs
    /// parameters a bare button cannot supply, and a service never finishes.
    pub triggerable: bool,
}

/// A job the catalog declares that this server did not register.
pub struct SkippedRow {
    /// The job's stable slug, so the row has a handle that is not its label.
    pub slug: &'static str,
    pub name: String,
    pub reason: String,
}

/// A page of the registry: the heading, and the rows under it.
pub struct TaskGroup {
    pub id: &'static str,
    pub title_key: &'static str,
    pub rows: Vec<RegisteredRow>,
}

/// The groups the registry renders, in the order they are read: grouped by how
/// a job comes to run, not listed flat.
const GROUP_ORDER: &[(&str, &str)] = &[
    ("periodic", "admin.task_periodic"),
    ("on_demand", "admin.task_on_demand"),
    ("manual", "admin.task_manual"),
    ("startup", "admin.task_startup"),
    ("service", "admin.task_service"),
];

/// Index of the group a job belongs to, matching `GROUP_ORDER`: an index
/// rather than an id, so a row cannot name a group that does not exist.
fn trigger_group_index(trigger: Trigger) -> usize {
    match trigger {
        Trigger::Periodic { .. } => 0,
        Trigger::OnDemand => 1,
        Trigger::Manual => 2,
        Trigger::StartupOnly => 3,
    }
}

/// Index of the group the long-lived services are listed under.
const SERVICE_GROUP_INDEX: usize = 4;

/// A locale key of the form `admin.job_<slug>[_desc]` or
/// `admin.service_<slug>[_desc]`, derived from the stable slug; the coverage
/// test walks the declared keys, so a job cannot be added without a name.
fn job_name_key(key: JobKey) -> String {
    key.name_key()
}

fn job_desc_key(key: JobKey) -> String {
    key.desc_key()
}

fn service_name_key(key: ServiceKey) -> String {
    key.name_key()
}

fn service_desc_key(key: ServiceKey) -> String {
    key.desc_key()
}

/// The compact form of an interval: `1h`, `5m`, `30s`, unit-suffixed so one
/// string reads the same in every language and no plural rule is needed.
fn interval_display(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// When a job runs, as a sentence.
fn schedule_label(t: &I18n, trigger: Trigger) -> String {
    match trigger {
        Trigger::Periodic { interval_secs, .. } => t.trf(
            "admin.schedule_every",
            &[("interval", &interval_display(interval_secs))],
        ),
        Trigger::Manual => t.tr("admin.schedule_manual").to_string(),
        Trigger::StartupOnly => t.tr("admin.schedule_startup").to_string(),
        Trigger::OnDemand => t.tr("admin.schedule_on_demand").to_string(),
    }
}

/// The priority a job competes at, as a label key.
fn priority_label_key(priority: Priority) -> &'static str {
    match priority {
        Priority::Interactive => "admin.task_priority_interactive",
        Priority::Normal => "admin.task_priority_normal",
        Priority::Background => "admin.task_priority_background",
    }
}

/// The icon a row carries, from how it comes to run.
fn trigger_icon(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Periodic { .. } => "clock",
        Trigger::OnDemand => "refresh",
        Trigger::Manual => "cog",
        Trigger::StartupOnly => "play",
    }
}

/// The verdict of a run, as a badge. An unknown phase keeps its wire name and
/// the neutral colour rather than disappearing, since the journal can outlive
/// the phase set that wrote it.
fn state_badge(t: &I18n, phase: &str) -> StateBadge {
    let (class, label) = match phase {
        "queued" => ("badge-gray", t.tr("admin.run_state_queued")),
        "running" => ("badge-brand", t.tr("admin.run_state_running")),
        "yielded" => ("badge-gray", t.tr("admin.run_state_yielded")),
        "succeeded" => ("badge-green", t.tr("admin.run_state_succeeded")),
        "failed" => ("badge-red", t.tr("admin.run_state_failed")),
        "cancelled" => ("badge-gray", t.tr("admin.run_state_cancelled")),
        "timed_out" => ("badge-red", t.tr("admin.run_state_timed_out")),
        "interrupted" => ("badge-red", t.tr("admin.run_state_interrupted")),
        other => ("badge-gray", other),
    };
    StateBadge {
        class,
        label: label.to_string(),
    }
}

/// The resource a job mostly contends for, as a label key.
fn resource_label_key(resource: Resource) -> &'static str {
    match resource {
        Resource::BlockIo => "admin.resource_block_io",
        Resource::Cpu => "admin.resource_cpu",
        Resource::DbWrite => "admin.resource_db_write",
    }
}

/// What a periodic job does when a tick arrives mid-run.
fn overlap_label_key(overlap: OverlapPolicy) -> &'static str {
    match overlap {
        OverlapPolicy::Skip => "admin.detail_overlap_skip",
        OverlapPolicy::Queue => "admin.detail_overlap_queue",
        OverlapPolicy::Allow => "admin.detail_overlap_allow",
    }
}

/// What a crash leaves behind for this job.
fn durability_label_key(durability: Durability) -> &'static str {
    match durability {
        Durability::Memory => "admin.detail_durability_memory",
        Durability::Audit => "admin.detail_durability_audit",
        Durability::Durable => "admin.detail_durability_durable",
    }
}

/// How long a run may take, and how long it may stall.
fn timeout_label(t: &I18n, spec: &JobSpec) -> String {
    let mut parts = Vec::new();
    if let Some(secs) = spec.timeout.no_progress_for_secs {
        parts.push(t.trf("admin.detail_no_progress", &[("secs", secs.to_string())]));
    }
    if let Some(secs) = spec.timeout.max_total_secs {
        parts.push(t.trf("admin.detail_max_total", &[("secs", secs.to_string())]));
    }
    if parts.is_empty() {
        t.tr("admin.detail_timeout_none").to_string()
    } else {
        parts.join(" · ")
    }
}

/// How a failed run is retried, in the reader's words.
fn retry_label(t: &I18n, spec: &JobSpec) -> String {
    match spec.retry {
        RetryPolicy::Never => t.tr("admin.detail_retry_never").to_string(),
        RetryPolicy::Fixed {
            attempts,
            delay_secs,
        } => t.trf(
            "admin.detail_retry_fixed",
            &[
                ("attempts", attempts.to_string()),
                ("delay", delay_secs.to_string()),
            ],
        ),
        RetryPolicy::Backoff {
            attempts,
            base_secs,
            max_secs,
        } => t.trf(
            "admin.detail_retry_backoff",
            &[
                ("attempts", attempts.to_string()),
                ("base", base_secs.to_string()),
                ("max", max_secs.to_string()),
            ],
        ),
    }
}

/// Whether a job steps aside for a busy server, and for how long it may wait.
fn deferral_label(t: &I18n, spec: &JobSpec) -> String {
    let Some(quiet) = spec.quiet else {
        return t.tr("admin.detail_deferrable_no").to_string();
    };
    if quiet.max_deferral_hours == 0 {
        return t.tr("admin.detail_deferrable_never_forced").to_string();
    }
    t.trf(
        "admin.detail_deferrable_yes",
        &[
            ("idle", quiet.min_idle_for_secs.to_string()),
            ("hours", quiet.max_deferral_hours.to_string()),
        ],
    )
}

/// How finely a job can be interrupted, which is what load awareness acts on.
fn interrupt_label(t: &I18n, spec: &JobSpec) -> String {
    let Some(chunk) = spec.chunkable else {
        return t.tr("admin.detail_not_interruptible").to_string();
    };
    t.trf(
        "admin.detail_chunk",
        &[
            ("ms", chunk.max_chunk_ms.to_string()),
            ("unit", chunk.unit.to_string()),
        ],
    )
}

/// The most recent report the job left behind: the last state alone decides
/// which of the kept messages is the current one.
fn job_message(stats: &JobStats) -> Option<MessageRow> {
    let text = match stats.last_state.as_ref() {
        Some(JobState::Succeeded) => stats.last_success_message.clone(),
        Some(_) => stats.last_error_message.clone(),
        None => String::new(),
    };
    if text.is_empty() {
        return None;
    }
    Some(MessageRow {
        failed: !matches!(stats.last_state, Some(JobState::Succeeded)),
        text,
    })
}

/// The policies a job runs under, not every field of the spec: the visibility,
/// dedup and idempotency rules stay in the catalog with their reasoning.
fn job_details(spec: &JobSpec, t: &I18n) -> Vec<DetailRow> {
    let detail = |label_key: &str, value: String| DetailRow {
        label: t.tr(label_key).to_string(),
        value,
    };
    let mut rows = vec![
        detail(
            "admin.task_priority",
            t.tr(priority_label_key(spec.priority)).to_string(),
        ),
        detail(
            "admin.detail_resource",
            t.tr(resource_label_key(spec.resource)).to_string(),
        ),
        detail("admin.task_concurrency", spec.max_concurrent.to_string()),
    ];
    if let Trigger::Periodic { overlap, .. } = spec.trigger {
        rows.push(detail(
            "admin.detail_overlap",
            t.tr(overlap_label_key(overlap)).to_string(),
        ));
    }
    rows.push(detail("admin.detail_timeout", timeout_label(t, spec)));
    rows.push(detail("admin.detail_retry", retry_label(t, spec)));
    rows.push(detail("admin.detail_deferrable", deferral_label(t, spec)));
    rows.push(detail("admin.detail_interrupt", interrupt_label(t, spec)));
    rows.push(detail(
        "admin.detail_cancel",
        t.tr(if spec.cancellable || spec.resumable {
            "admin.detail_cancel_yes"
        } else {
            "admin.detail_cancel_no"
        })
        .to_string(),
    ));
    rows.push(detail(
        "admin.detail_durability",
        t.tr(durability_label_key(spec.durability)).to_string(),
    ));
    rows
}

/// The lifetime counters of a job that has run, shown in the row's disclosure;
/// a job that never ran has none, which is not the same as showing zeroes.
fn job_counters(stats: &JobStats, t: &I18n) -> Vec<DetailRow> {
    if stats.run_count == 0 {
        return Vec::new();
    }
    let count = |label_key: &str, value: u64| DetailRow {
        label: t.tr(label_key).to_string(),
        value: value.to_string(),
    };
    let mut counters = vec![
        count("admin.fact_runs", stats.run_count),
        count("admin.fact_success", stats.success_count),
        count("admin.fact_error", stats.error_count),
    ];
    if stats.total_processed > 0 {
        counters.push(count("admin.fact_processed", stats.total_processed));
    }
    counters
}

/// How long the last run took. Zero is the absence of a duration rather than a
/// duration of zero; the row shows a dash so the column keeps its width.
fn last_duration(stats: &JobStats) -> Option<String> {
    (stats.last_duration_ms > 0).then(|| format!("{}ms", stats.last_duration_ms))
}

/// Row for a registered job.
fn job_row(job: &RegisteredJob, stats: &JobStats, t: &I18n) -> RegisteredRow {
    let spec = &job.spec;
    let name = t.tr(&job_name_key(spec.key)).to_string();
    RegisteredRow {
        slug: spec.key.as_str().to_string(),
        confirm_args: serde_json::json!({ "name": name }).to_string(),
        name,
        desc: t.tr(&job_desc_key(spec.key)).to_string(),
        icon: trigger_icon(spec.trigger),
        kind: "job",
        schedule: schedule_label(t, spec.trigger),
        state: stats
            .last_state
            .as_ref()
            .map(|state| state_badge(t, state.as_str())),
        // Not `run_count == 0`: a restart empties the counters but not the
        // journal, and a job whose last run was read back from it has run.
        never_run: stats.run_count == 0 && stats.last_run_at.is_none(),
        last_run_ts: stats.last_run_at,
        last_duration: last_duration(stats),
        counters: job_counters(stats, t),
        details: job_details(spec, t),
        last_message: job_message(stats),
        group_index: trigger_group_index(spec.trigger),
        interval_secs: match spec.trigger {
            Trigger::Periodic { interval_secs, .. } => Some(interval_secs),
            _ => None,
        },
        triggerable: matches!(spec.trigger, Trigger::Periodic { .. } | Trigger::Manual),
    }
}

/// Row for a long-lived service: it never finishes, so a run count, a last-run
/// time or a schedule beyond "while the server runs" would be invented numbers.
fn service_row(key: ServiceKey, t: &I18n) -> RegisteredRow {
    RegisteredRow {
        slug: key.as_str().to_string(),
        name: t.tr(&service_name_key(key)).to_string(),
        desc: t.tr(&service_desc_key(key)).to_string(),
        icon: "inbox",
        kind: "service",
        schedule: t.tr("admin.schedule_service").to_string(),
        state: None,
        never_run: false,
        last_run_ts: None,
        last_duration: None,
        counters: Vec::new(),
        details: Vec::new(),
        last_message: None,
        group_index: SERVICE_GROUP_INDEX,
        interval_secs: None,
        confirm_args: String::new(),
        triggerable: false,
    }
}

/// Sort rows into their groups, dropping empty ones. The schedule group is
/// ordered by interval, shortest first.
fn build_groups(rows: Vec<RegisteredRow>) -> Vec<TaskGroup> {
    let mut groups: Vec<TaskGroup> = GROUP_ORDER
        .iter()
        .map(|(id, title_key)| TaskGroup {
            id,
            title_key,
            rows: Vec::new(),
        })
        .collect();
    for row in rows {
        groups[row.group_index].rows.push(row);
    }
    if let Some(periodic) = groups.first_mut() {
        periodic
            .rows
            .sort_by_key(|row| row.interval_secs.unwrap_or(u64::MAX));
    }
    groups.retain(|group| !group.rows.is_empty());
    groups
}

/// What a POST redirect lands with, so the page can confirm the action. An
/// unrecognised value renders no banner rather than echoing it into the page.
#[derive(Deserialize)]
pub struct TasksQuery {
    pub action: Option<String>,
}

/// What the run list was asked to show; an unrecognised value shows everything.
#[derive(Deserialize, Default)]
pub struct RunsQuery {
    /// A job slug.
    pub kind: Option<String>,
    /// `succeeded` or `failed`.
    pub outcome: Option<String>,
}

/// The filter the run list was opened with, as a reader chose it.
#[derive(Default)]
struct RunListChoice {
    kind: Option<String>,
    outcome: Option<RunOutcome>,
}

impl RunListChoice {
    /// Read the choice out of a query string, validated rather than trusted: a
    /// hand-typed URL cannot ask the page for something that does not exist.
    fn from_query(query: &RunsQuery) -> Self {
        let kind = query
            .kind
            .as_deref()
            .filter(|kind| {
                !kind.is_empty()
                    && kind.len() <= 64
                    && kind
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            })
            .map(str::to_string);
        let outcome = match query.outcome.as_deref() {
            Some("succeeded") => Some(RunOutcome::Succeeded),
            Some("failed") => Some(RunOutcome::Failed),
            _ => None,
        };
        Self { kind, outcome }
    }

    /// The list's own URL with one choice replaced, keeping the other, so a page
    /// filtered by job and verdict can change one without losing the other.
    fn href(&self, kind: Option<&str>, outcome: Option<&str>) -> String {
        let mut params: Vec<String> = Vec::new();
        if let Some(kind) = kind.filter(|k| !k.is_empty()) {
            params.push(format!("kind={kind}"));
        }
        if let Some(outcome) = outcome.filter(|o| !o.is_empty()) {
            params.push(format!("outcome={outcome}"));
        }
        if params.is_empty() {
            "/sysadmin/tasks/".to_string()
        } else {
            format!("/sysadmin/tasks/?{}", params.join("&"))
        }
    }
}

/// One filter button: what clicking it asks for, and whether it is current.
pub struct FilterChip {
    /// The query value, empty for "everything", used as the row's DOM handle.
    pub id: String,
    pub label: String,
    pub href: String,
    pub active: bool,
}

/// GET /sysadmin/tasks/ — what has been running (admin only).
pub async fn runs_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<RunsQuery>,
) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }
    match render_runs(&state, &user, RunListChoice::from_query(&query)).await {
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

/// The reason a declared job is not registered here, as a sentence rather than
/// a name: no naming rule would produce "notifications are switched off".
fn skip_reason_key(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::NotificationsOff => "admin.skip_no_notifications",
        SkipReason::GcDisabled => "admin.skip_gc_disabled",
        SkipReason::EncryptionNotLazy => "admin.skip_encryption_not_lazy",
        SkipReason::IndexOff => "admin.skip_index_off",
        SkipReason::IndexBackfillOff => "admin.skip_index_backfill_off",
        SkipReason::MailOff => "admin.skip_mail_off",
    }
}

/// The name of the job a run belongs to. A run outlives its job, so the lookup
/// walks: a job this build runs, a retired slug it still names, the slug itself.
fn run_name(slug: &str, t: &I18n) -> String {
    if let Some(key) = JobKey::from_slug(slug) {
        return t.tr(&job_name_key(key)).to_string();
    }
    match crate::tasks::spec::retired_job_name_key(slug) {
        Some(key) => t.tr(key).to_string(),
        None => slug.to_string(),
    }
}

/// How many of the newest finished runs are scanned for the jobs a reader can
/// filter by: a bounded window, not a `GROUP BY` over the journal.
const KIND_WINDOW: u64 = 500;

/// The verdict of a chosen filter as a query value: written out because these
/// are the words the query string and the state badges share.
fn outcome_query(outcome: Option<RunOutcome>) -> Option<&'static str> {
    match outcome {
        Some(RunOutcome::Succeeded) => Some("succeeded"),
        Some(RunOutcome::Failed) => Some("failed"),
        None => None,
    }
}

/// The job filter buttons: everything, then the jobs the journal holds, newest
/// first. Each group's "everything" names the dimension it clears.
fn kind_filters(choice: &RunListChoice, kinds: &[String], t: &I18n) -> Vec<FilterChip> {
    let mut chips = vec![FilterChip {
        id: String::new(),
        label: t.tr("admin.runs_filter_all_jobs").to_string(),
        href: choice.href(None, outcome_query(choice.outcome)),
        active: choice.kind.is_none(),
    }];
    for kind in kinds {
        chips.push(FilterChip {
            id: kind.clone(),
            label: run_name(kind, t),
            href: choice.href(Some(kind), outcome_query(choice.outcome)),
            active: choice.kind.as_deref() == Some(kind.as_str()),
        });
    }
    chips
}

/// The verdict filter buttons.
fn outcome_filters(choice: &RunListChoice, t: &I18n) -> Vec<FilterChip> {
    [
        (None, "admin.runs_filter_all_results"),
        (Some(RunOutcome::Succeeded), "admin.run_state_succeeded"),
        (Some(RunOutcome::Failed), "admin.run_state_failed"),
    ]
    .into_iter()
    .map(|(outcome, label_key)| FilterChip {
        id: outcome_query(outcome).unwrap_or_default().to_string(),
        label: t.tr(label_key).to_string(),
        href: choice.href(choice.kind.as_deref(), outcome_query(outcome)),
        active: choice.outcome == outcome,
    })
    .collect()
}

/// How far a run has got, as the run reports it. `total` is optional: a job
/// that cannot say would get an invented percentage.
fn progress_value(run: &JobRun) -> String {
    match run.progress.total {
        Some(total) => format!("{} / {}", run.progress.done, total),
        None => run.progress.done.to_string(),
    }
}

/// A duration a reader can compare at a glance.
fn duration_display(secs: i64) -> String {
    if secs >= 60 {
        format!("{}m", secs / 60)
    } else if secs >= 1 {
        format!("{secs}s")
    } else {
        // Escaped by the template, not here.
        "<1s".to_string()
    }
}

/// The account behind each owner id, so the page names the submitter.
async fn owner_emails(state: &Arc<AppState>, ids: &[i32]) -> HashMap<i32, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    match state.repos.user.find_by_ids(ids).await {
        Ok(users) => users
            .into_iter()
            .map(|user| (user.id, user.email))
            .collect(),
        // A page that cannot name the submitter still has the run to show.
        Err(_) => HashMap::new(),
    }
}

/// Who to name as the submitter of a run.
fn owner_label(t: &I18n, owners: &HashMap<i32, String>, owner: Option<i32>) -> String {
    match owner {
        // A scheduled pass is the server's own work, not somebody's request.
        None => t.tr("admin.owner_system").to_string(),
        Some(id) => owners
            .get(&id)
            .cloned()
            .unwrap_or_else(|| t.tr("admin.owner_unknown").to_string()),
    }
}

/// One row for a run that is still going.
fn active_run_row(run: &JobRun, owners: &HashMap<i32, String>, t: &I18n) -> ActiveRunRow {
    // A run that has not started yet is still queued, and saying when it was
    // submitted is the only time it has.
    let (time_ts, time_key, time_label) = match run.started_at {
        Some(ts) => (ts, "started", t.tr("admin.fact_started")),
        None => (run.created_at, "submitted", t.tr("admin.fact_submitted")),
    };
    ActiveRunRow {
        id: run.id.as_str().to_string(),
        name: run_name(run.key.as_str(), t),
        state: state_badge(t, run.state.as_str()),
        owner: owner_label(t, owners, run.owner),
        progress: progress_value(run),
        time_ts,
        time_key,
        time_label: time_label.to_string(),
        message: run.progress.message.clone(),
    }
}

/// One row for a run the journal kept.
fn journal_row(row: &job_run::Model, owners: &HashMap<i32, String>, t: &I18n) -> RunRow {
    let duration = row
        .started_at
        .zip(row.finished_at)
        .map(|(started, finished)| duration_display(finished.saturating_sub(started)));
    RunRow {
        id: row.id.clone(),
        name: run_name(&row.kind, t),
        state: state_badge(t, &row.phase),
        owner: owner_label(t, owners, row.owner),
        finished_ts: row.finished_at,
        duration,
        // An empty string and "no error" are the same thing here.
        error: row.error.clone().filter(|error| !error.is_empty()),
        processed: row.processed,
        attempt: row.attempt,
        summary: row.summary.clone(),
    }
}

/// Build and render the run list.
async fn render_runs(
    state: &Arc<AppState>,
    user: &WebUser,
    choice: RunListChoice,
) -> Result<Response, AppError> {
    let t = I18n::get(user.language.as_deref());

    // Every active run is shown: they are bounded by each job's own
    // concurrency, not by a page size, and a truncated list would be a lie.
    let active_runs = state.tasks.store().list(&RunFilter {
        include_active: true,
        ..RunFilter::default()
    });

    // The durable journal, which survives a restart, filtered if the reader
    // asked for a job or a verdict.
    let filter = RunHistoryFilter {
        kind: choice.kind.clone(),
        outcome: choice.outcome,
    };
    let journal = state
        .repos
        .job_run
        .recent_matching(20, &filter)
        .await
        .unwrap_or_default();
    // What the filter can offer is what the journal holds: a chip for a job
    // with no rows would only ever show an empty page.
    let kinds = state
        .repos
        .job_run
        .recent_kinds(KIND_WINDOW)
        .await
        .unwrap_or_default();

    // One lookup for both lists, so a page of runs is not a query per row.
    let mut owner_ids: Vec<i32> = active_runs.iter().filter_map(|run| run.owner).collect();
    owner_ids.extend(journal.iter().filter_map(|row| row.owner));
    owner_ids.sort_unstable();
    owner_ids.dedup();
    let owners = owner_emails(state, &owner_ids).await;

    let active = active_runs
        .iter()
        .map(|run| active_run_row(run, &owners, t))
        .collect();
    let runs = journal
        .iter()
        .map(|row| journal_row(row, &owners, t))
        .collect();

    let job_filters = kind_filters(&choice, &kinds, t);
    let outcome_filters = outcome_filters(&choice, t);

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    let policy = crate::tasks::queue::JobHistoryPolicy::DEFAULT;
    let retention_hint = t.trf(
        "admin.runs_retention_hint",
        &[
            ("rows", policy.max_rows.to_string()),
            (
                "days",
                (policy.succeeded_retention_secs / 86_400).to_string(),
            ),
            (
                "failed_days",
                (policy.failed_retention_secs / 86_400).to_string(),
            ),
        ],
    );

    let tpl = RunsTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: Some(ctx.csrf_token),
        active_page: "admintasks",
        active,
        runs,
        job_filters,
        outcome_filters,
        filtered: choice.kind.is_some() || choice.outcome.is_some(),
        retention_hint,
        load: LoadRow::from_state(state),
        error: None,
        success: None,
        left_panel_repos: ctx.left_panel_repos,
        sharing_enabled: ctx.sharing_enabled,
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
    let mut rows: Vec<RegisteredRow> = state
        .tasks
        .jobs()
        .iter()
        .map(|job| job_row(job, &state.tasks.stats(job.spec.key), t))
        .collect();
    // Services are listed too, since they run in the same system: they are not
    // jobs, so they carry no counters — the row builder keeps that true.
    rows.extend(
        state
            .tasks
            .services()
            .iter()
            .map(|key| service_row(*key, t)),
    );
    let groups = build_groups(rows);

    // Say which declared jobs and services this server does not run, and why,
    // rather than leaving them out silently.
    let skipped = state
        .tasks
        .skipped()
        .into_iter()
        .map(|task| SkippedRow {
            slug: task.slug,
            name: t.tr(&task.name_key).to_string(),
            reason: t.tr(skip_reason_key(task.reason)).to_string(),
        })
        .collect();

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    let tpl = RegisteredTasksTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: Some(ctx.csrf_token),
        active_page: "admintasks",
        groups,
        skipped,
        error,
        success,
        left_panel_repos: ctx.left_panel_repos,
        sharing_enabled: ctx.sharing_enabled,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// POST /sysadmin/tasks/{name}/trigger/ — queue a job to run now: the run is
/// submitted, not executed inside this request.
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

    // No job by that slug: re-render the registry with the reason, not JSON.
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

    // Submitted as an operator's run: the record is the answer to the press, so
    // it is kept even when the job finds nothing to do. No summary.
    if let Err(e) = state
        .tasks
        .submit_system(
            Origin::Operator,
            key,
            serde_json::Value::Null,
            String::new(),
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
    use crate::tasks::run::Params;
    use crate::tasks::spec::OverlapPolicy;
    use crate::tasks::spec::Visibility;

    /// The languages the server can render, as `I18n::get` resolves them.
    fn languages() -> [&'static I18n; 2] {
        [I18n::get(None), I18n::get(Some("zh"))]
    }

    /// Every job must have a name, a description and a group heading in every
    /// language: a derived key means a new job would silently get no label.
    #[test]
    fn every_job_is_named_and_described_in_every_language() {
        for key in JobKey::ALL {
            let trigger = crate::tasks::catalog::policy(*key).trigger;
            let (_, group_title_key) = GROUP_ORDER[trigger_group_index(trigger)];
            for locale_key in [
                job_name_key(*key),
                job_desc_key(*key),
                group_title_key.to_string(),
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

    /// A group is named by the same word it is id'd by, so the two cannot drift apart.
    #[test]
    fn every_group_is_titled_in_every_language() {
        for (id, title_key) in GROUP_ORDER {
            assert_eq!(*title_key, format!("admin.task_{id}"));
            for t in languages() {
                assert_ne!(t.tr(title_key), *title_key, "{id} has no heading");
            }
        }
    }

    /// The same for the long-lived services, which the catalog walk above misses.
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

    /// The derived key is the contract between a slug and its label, so pin the shape.
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

    /// A startup job must not be filed under "manual", which says it never runs alone.
    #[test]
    fn a_startup_job_is_not_filed_under_manual() {
        assert_ne!(
            trigger_group_index(Trigger::StartupOnly),
            trigger_group_index(Trigger::Manual)
        );
        let (id, title_key) = GROUP_ORDER[trigger_group_index(Trigger::StartupOnly)];
        assert_eq!(id, "startup");
        assert_eq!(I18n::get(None).tr(title_key), "Startup");
    }

    /// A registered job as the registry sees it, with the real catalog policy.
    fn registered(key: JobKey) -> RegisteredJob {
        RegisteredJob::new(
            crate::tasks::catalog::policy(key),
            std::sync::Arc::new(|_ctx, _params| {
                Box::pin(async { Ok(crate::tasks::run::Outcome::ok()) })
            }),
        )
    }

    /// Every job and service, as the page builds them.
    fn all_rows(t: &I18n) -> Vec<RegisteredRow> {
        JobKey::ALL
            .iter()
            .map(|key| job_row(&registered(*key), &JobStats::default(), t))
            .chain(ServiceKey::ALL.iter().map(|key| service_row(*key, t)))
            .collect()
    }

    /// A row must not be dropped or listed twice, and the groups stay in order.
    #[test]
    fn every_job_lands_in_exactly_one_group() {
        let rows = all_rows(I18n::get(None));
        let expected = rows.len();
        let groups = build_groups(rows);

        let mut slugs: Vec<&str> = groups
            .iter()
            .flat_map(|group| group.rows.iter().map(|row| row.slug.as_str()))
            .collect();
        assert_eq!(slugs.len(), expected, "a row was dropped");
        slugs.sort_unstable();
        let total = slugs.len();
        slugs.dedup();
        assert_eq!(slugs.len(), total, "a row was listed twice");

        let ids: Vec<&str> = groups.iter().map(|group| group.id).collect();
        let declared: Vec<&str> = GROUP_ORDER
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| ids.contains(id))
            .collect();
        assert_eq!(ids, declared, "the groups are out of order");
    }

    /// The group index is only as good as the table it indexes, so pin which
    /// group each trigger lands in.
    #[test]
    fn a_group_index_names_the_group_its_trigger_belongs_to() {
        for trigger in [
            Trigger::Periodic {
                interval_secs: 60,
                overlap: OverlapPolicy::Skip,
            },
            Trigger::OnDemand,
            Trigger::Manual,
            Trigger::StartupOnly,
        ] {
            let (id, title_key) = GROUP_ORDER[trigger_group_index(trigger)];
            assert_eq!(title_key, format!("admin.task_{id}"));
        }
        assert_eq!(GROUP_ORDER[SERVICE_GROUP_INDEX].0, "service");
    }

    /// A service never finishes, so a run count or a last-run time would be invented.
    #[test]
    fn a_service_row_carries_no_job_counters() {
        let t = I18n::get(None);
        for key in ServiceKey::ALL {
            let row = service_row(*key, t);
            assert_eq!(row.kind, "service");
            assert!(row.state.is_none(), "{key:?} claimed a last run");
            assert!(row.last_run_ts.is_none(), "{key:?} carried a last run");
            assert!(row.last_duration.is_none(), "{key:?} carried a duration");
            assert!(row.counters.is_empty(), "{key:?} carried counters");
            assert!(!row.never_run, "{key:?} is not a job");
            assert!(!row.triggerable, "{key:?} has nothing to trigger");
            assert_eq!(row.group_index, SERVICE_GROUP_INDEX);
        }
    }

    /// Zero and "no such number" must not look the same.
    #[test]
    fn a_job_that_has_never_run_reports_no_counters() {
        let row = job_row(
            &registered(JobKey::GarbageCollection),
            &JobStats::default(),
            I18n::get(None),
        );
        assert!(row.never_run);
        assert!(row.counters.is_empty());
        assert!(row.last_run_ts.is_none(), "it has no last run to report");
        assert!(row.last_duration.is_none(), "it has no duration to report");
        assert!(row.state.is_none());
    }

    /// A job that has run reports labelled numbers in its disclosure.
    #[test]
    fn a_job_that_has_run_reports_labelled_counters() {
        let stats = JobStats {
            run_count: 12,
            success_count: 11,
            error_count: 1,
            last_run_at: Some(1_700_000_000),
            last_duration_ms: 250,
            total_processed: 37,
            ..JobStats::default()
        };
        let row = job_row(
            &registered(JobKey::ExpiredDataCleanup),
            &stats,
            I18n::get(None),
        );
        assert!(!row.never_run);
        assert!(row.state.is_none(), "the default state is no state");

        let counters: Vec<(&str, &str)> = row
            .counters
            .iter()
            .map(|counter| (counter.label.as_str(), counter.value.as_str()))
            .collect();
        assert_eq!(
            counters,
            vec![
                ("Runs", "12"),
                ("Succeeded", "11"),
                ("Failed", "1"),
                ("Processed", "37"),
            ]
        );

        assert_eq!(row.last_run_ts, Some(1_700_000_000));
        assert_eq!(row.last_duration.as_deref(), Some("250ms"));
    }
    /// The schedule group reads shortest interval first.
    #[test]
    fn the_schedule_group_reads_shortest_interval_first() {
        let t = I18n::get(None);
        let groups = build_groups(vec![
            job_row(
                &registered(JobKey::ExpiredDataCleanup),
                &JobStats::default(),
                t,
            ),
            job_row(&registered(JobKey::IndexCommit), &JobStats::default(), t),
        ]);
        let periodic = groups
            .iter()
            .find(|group| group.id == "periodic")
            .expect("both jobs are periodic");
        assert_eq!(periodic.rows[0].slug, "index-commit", "30s comes before 1h");
    }

    /// A state is never printed as the wire name the database happens to hold.
    #[test]
    fn a_state_is_never_rendered_as_its_wire_name() {
        let phases = [
            "queued",
            "running",
            "yielded",
            "succeeded",
            "failed",
            "cancelled",
            "timed_out",
            "interrupted",
        ];
        for t in languages() {
            for phase in phases {
                let badge = state_badge(t, phase);
                assert_ne!(badge.label, phase, "{phase} rendered its wire name");
                assert!(badge.class.starts_with("badge-"));
            }
            // An unknown phase keeps its name rather than disappearing.
            assert_eq!(state_badge(t, "future_phase").label, "future_phase");
        }
    }

    /// A run that outlives its job still has a name to show.
    #[test]
    fn a_run_names_its_job_or_falls_back_to_its_slug() {
        let t = I18n::get(None);
        assert_eq!(run_name("gc", t), "Garbage collection");
        assert_eq!(run_name("retired-job", t), "retired-job");
        // A slug this build retired still has a name, so its rows do not read
        // as code vocabulary.
        assert_eq!(run_name("share-link-cleanup", t), "Share link cleanup");
        assert_eq!(
            run_name("temp-upload-cleanup", t),
            "Temporary upload cleanup"
        );
    }

    /// A stale bookmark must not ask for a slug or verdict that does not exist.
    #[test]
    fn a_filter_that_makes_no_sense_shows_everything() {
        let choice = RunListChoice::from_query(&RunsQuery {
            kind: Some("Share Link Cleanup!".to_string()),
            outcome: Some("exploded".to_string()),
        });
        assert!(choice.kind.is_none(), "a slug is lower-case words");
        assert!(choice.outcome.is_none());

        let choice = RunListChoice::from_query(&RunsQuery {
            kind: Some("share-link-cleanup".to_string()),
            outcome: Some("failed".to_string()),
        });
        assert_eq!(choice.kind.as_deref(), Some("share-link-cleanup"));
        assert_eq!(choice.outcome, Some(RunOutcome::Failed));
    }

    /// Each filter keeps the other.
    #[test]
    fn a_filter_link_keeps_the_other_choice() {
        let choice = RunListChoice {
            kind: Some("gc".to_string()),
            outcome: Some(RunOutcome::Failed),
        };
        assert_eq!(
            choice.href(None, Some("failed")),
            "/sysadmin/tasks/?outcome=failed"
        );
        assert_eq!(choice.href(Some("gc"), None), "/sysadmin/tasks/?kind=gc");
        assert_eq!(choice.href(None, None), "/sysadmin/tasks/");
        assert_eq!(
            choice.href(Some("gc"), Some("failed")),
            "/sysadmin/tasks/?kind=gc&outcome=failed"
        );
    }

    /// The buttons say which one is current.
    #[test]
    fn exactly_one_button_per_filter_is_current() {
        let t = I18n::get(None);
        let choice = RunListChoice {
            kind: Some("gc".to_string()),
            outcome: None,
        };
        let jobs = kind_filters(&choice, &["gc".to_string(), "reindex".to_string()], t);
        assert_eq!(jobs.len(), 3, "everything, then the jobs the journal holds");
        assert_eq!(jobs.iter().filter(|chip| chip.active).count(), 1);
        // Each group's "everything" names the dimension it clears.
        assert_eq!(jobs[0].label, "All jobs");
        assert_eq!(jobs[1].label, "Garbage collection");

        let outcomes = outcome_filters(&choice, t);
        assert_eq!(outcomes.len(), 3);
        assert_eq!(
            outcomes.iter().filter(|chip| chip.active).count(),
            1,
            "an unset verdict has exactly one current button too"
        );
        assert_eq!(outcomes[0].id, "");
        assert_eq!(outcomes[0].label, "All results");
        assert_eq!(outcomes[1].id, "succeeded");
        assert_eq!(outcomes[1].label, "Succeeded");
    }

    /// Every retired slug has a name in every language: its rows stay in the journal.
    #[test]
    fn every_retired_job_is_named_in_every_language() {
        for (slug, name_key) in crate::tasks::spec::RETIRED_JOBS {
            assert_eq!(
                crate::tasks::spec::retired_job_name_key(slug),
                Some(*name_key),
                "{slug} is not resolvable"
            );
            assert!(
                !name_key.ends_with("_desc"),
                "{slug} has no registry row to describe"
            );
            for t in languages() {
                assert_ne!(t.tr(name_key), *name_key, "{name_key} is missing");
            }
        }
    }

    /// A submitted run has no account, and an account can be deleted while its
    /// runs stay in the journal; neither should be printed as a number.
    #[test]
    fn a_run_names_its_submitter_or_says_why_it_cannot() {
        let t = I18n::get(None);
        let owners = HashMap::from([(7, "someone@example.com".to_string())]);
        assert_eq!(owner_label(t, &owners, Some(7)), "someone@example.com");
        assert_eq!(owner_label(t, &owners, Some(9)), "Deleted account");
        assert_eq!(owner_label(t, &owners, None), "The server");
    }

    /// A one-second run and a one-minute run are not both "0m".
    #[test]
    fn a_duration_reads_at_the_scale_it_happened_on() {
        assert_eq!(duration_display(0), "<1s");
        assert_eq!(duration_display(1), "1s");
        assert_eq!(duration_display(59), "59s");
        assert_eq!(duration_display(60), "1m");
        assert_eq!(duration_display(3600), "60m");
    }

    /// A run reports only the total it knows.
    #[test]
    fn progress_reports_only_the_total_the_run_knows() {
        let mut run = JobRun::queued(
            JobKey::Reindex,
            Origin::Request,
            Visibility::OwnerOrAdmin,
            None,
            Params::Null,
            "reindex",
            None,
            0,
        );
        assert_eq!(progress_value(&run), "0");
        run.progress.done = 128;
        run.progress.total = Some(512);
        assert_eq!(progress_value(&run), "128 / 512");
    }

    /// Every skip reason must be spelled out, or the panel prints the key.
    #[test]
    fn every_skip_reason_is_written_in_every_language() {
        for reason in SkipReason::ALL {
            let key = skip_reason_key(*reason);
            assert_ne!(key, "", "{reason:?} has no reason key");
            for t in languages() {
                assert_ne!(t.tr(key), key, "{key} is missing for {}", t.lang);
            }
        }
    }

    /// A job's counters and policies are written in the reader's language, never
    /// left as the locale keys — `tr` falls back to the key.
    #[test]
    fn every_detail_is_written_in_the_reader_language() {
        // A run of everything, so the counters are not the empty case.
        let stats = JobStats {
            run_count: 12,
            success_count: 11,
            error_count: 1,
            last_run_at: Some(1_700_000_000),
            last_duration_ms: 250,
            total_processed: 37,
            ..JobStats::default()
        };
        for t in languages() {
            for key in JobKey::ALL {
                let spec = crate::tasks::catalog::policy(*key);
                let counters = job_counters(&stats, t);
                assert_eq!(counters.len(), 4, "{key:?} has no labelled counters");
                for detail in job_details(&spec, t).iter().chain(counters.iter()) {
                    assert!(
                        !detail.label.starts_with("admin."),
                        "{key:?} leaked a label key: {}",
                        detail.label
                    );
                    assert!(
                        !detail.value.starts_with("admin."),
                        "{key:?} leaked a value key: {}",
                        detail.value
                    );
                    assert!(
                        !detail.value.contains('{') && !detail.value.is_empty(),
                        "{key:?} has an unsubstituted or empty value: {}",
                        detail.value
                    );
                }
            }
        }
    }

    /// A job's policies are behind a disclosure; a service has none to disclose.
    #[test]
    fn a_service_has_no_policies_to_disclose() {
        let t = I18n::get(None);
        let service = service_row(ServiceKey::EventListener, t);
        assert!(service.details.is_empty());
        assert!(service.counters.is_empty());
        assert!(service.last_message.is_none());

        let job = job_row(
            &registered(JobKey::GarbageCollection),
            &JobStats::default(),
            t,
        );
        let labels: Vec<&str> = job.details.iter().map(|row| row.label.as_str()).collect();
        assert!(labels.contains(&"Priority"), "{labels:?}");
        assert!(labels.contains(&"Waits for a quiet server"), "{labels:?}");
        assert!(labels.contains(&"After a crash"), "{labels:?}");
        assert!(job.last_message.is_none(), "it has never run");
    }

    /// Only the message matching the current state is the current report.
    #[test]
    fn the_last_message_follows_the_last_state() {
        let mut stats = JobStats {
            run_count: 2,
            success_count: 1,
            error_count: 1,
            last_success_message: "no expired entries".to_string(),
            last_error_message: "database is locked".to_string(),
            last_state: Some(JobState::Failed("database is locked".to_string())),
            ..JobStats::default()
        };
        let message = job_message(&stats).expect("a run failed");
        assert!(message.failed);
        assert_eq!(message.text, "database is locked");

        stats.last_state = Some(JobState::Succeeded);
        let message = job_message(&stats).expect("a run succeeded");
        assert!(!message.failed, "the earlier failure is stale");
        assert_eq!(message.text, "no expired entries");
    }

    /// The JSON is built here rather than interpolated in the template, so a
    /// name containing a quote cannot break it.
    #[test]
    fn the_confirm_args_are_json_naming_the_job() {
        let row = job_row(
            &registered(JobKey::GarbageCollection),
            &JobStats::default(),
            I18n::get(None),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&row.confirm_args).expect("the args are JSON");
        assert_eq!(parsed["name"], serde_json::json!("Garbage collection"));
    }
}
