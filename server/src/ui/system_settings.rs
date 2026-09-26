//! Admin Web UI — system management: every setting that can be managed at
//! runtime, on one page per area.
//!
//! The page is *data-driven*: it renders the catalog (`infra::settings::CATALOG`)
//! rather than naming fields, so a setting cannot exist without appearing here.
//! A page is rendered as its groups (`infra::settings::GROUPS`), and each row
//! says what the setting does, where its effective value came from — the
//! environment, the config file, the database, or the built-in default — and the
//! one state it is in: waiting for a restart, needing one, or read-only.
//!
//! What the page deliberately cannot do is unchanged from the rest of the admin
//! area: read-only settings (the master secret, the database URL, the state
//! directories) are shown but not editable, and a secret is never rendered back —
//! only replaced or cleared.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::settings::SecretState;
use base::error::AppError;
use infra::settings::{Apply, Kind, Origin, Section, SettingDef};

use super::auth_extractor::WebUser;

/// One row of the page.
pub struct SettingRow {
    pub key: String,
    /// The locale key, so the template renders the label.
    pub label_key: String,
    /// Everything the filter matches this row against: the key, the label and
    /// the help sentence, so a half-remembered name, a config key and a word
    /// from the description all find it.
    pub search_text: String,
    /// The one sentence that says what this setting does. Every catalog entry
    /// has one in both languages; `every_catalog_string_is_translated` is what
    /// keeps it that way, because a row with only a label is a row an operator
    /// has to guess at.
    pub help: String,
    /// The unit the value is expressed in, already translated. The label does
    /// not repeat it.
    pub unit: Option<String>,
    /// The form control: `bool`, `number`, `text`, `enum`, `list`, `path`,
    /// `secret`.
    pub control: &'static str,
    /// The value the control starts with (never a secret's value).
    pub value: String,
    pub checked: bool,
    /// The `<option>`s of an enum control, with the current one already marked:
    /// deciding that here keeps the template free of string comparisons.
    pub options: Vec<SelectOption>,
    /// The form field name is the key for a value, prefixed for a secret.
    pub field: String,
    /// Where the effective value came from: `environment`, `config`,
    /// `database`, `default`.
    pub origin: &'static str,
    /// The origin badge's tooltip: the `config.toml` key that supplied the
    /// value, or who saved it. The badge says the kind, and this says which one
    /// — on hover, because a page of keys is a page nobody reads.
    pub origin_title: String,
    /// The `NANOFILE_*` variable the value came from, when the environment owns
    /// it. This is the one piece of provenance a row prints as text: it is also
    /// the only one an operator has to go and change.
    pub env_var: Option<String>,
    /// Unix seconds of the last save, for a database-sourced value.
    pub origin_at: Option<i64>,
    /// The value in the config file, when a saved value is superseding it —
    /// otherwise an operator editing the file sees no effect and no reason why.
    pub config_value: Option<String>,
    /// The one state the row is in, by precedence: waiting for a restart
    /// outranks needing one, which outranks being read-only. A row that said
    /// all of them at once said none of them.
    pub state_badge: Option<StateBadge>,
    /// Only the environment or the config file can set this.
    pub locked: bool,
    /// Read-only settings are shown for their origin, not for editing.
    pub read_only: bool,
    /// A secret is configured (the field renders a placeholder, not a value).
    pub secret_set: bool,
    /// A stored secret cannot be decrypted any more.
    pub secret_broken: bool,
}

/// The badge for the one state a row is in.
pub struct StateBadge {
    /// `badge-red` for something the operator still has to do, `badge-gray`
    /// otherwise.
    pub class: &'static str,
    pub label: String,
}

/// One group of rows on a page, ready for the template.
///
/// A page is rendered as its groups rather than as one flat list, so a heading
/// and the rows under it can be hidden together when the page is filtered.
pub struct SettingGroup {
    pub id: &'static str,
    pub title_key: &'static str,
    pub rows: Vec<SettingRow>,
}

/// One `<option>` of an enum control.
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// One entry of the section navigation.
pub struct SectionLink {
    pub id: &'static str,
    pub label_key: &'static str,
    pub active: bool,
}

/// The `[settings]` policy, for the advanced page.
pub struct PolicyView {
    pub config_policy: &'static str,
    pub override_keys: Vec<String>,
    pub refresh_interval_secs: u64,
}

#[derive(Template)]
#[template(path = "sysadmin/settings.html")]
pub struct SystemSettingsTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,

    pub section: &'static str,
    pub section_title_key: &'static str,
    pub section_subtitle_key: &'static str,
    pub sections: Vec<SectionLink>,
    pub groups: Vec<SettingGroup>,
    /// Keys whose saved value supersedes a config-file entry that disagrees.
    pub drift_keys: Vec<String>,
    pub pending_restart: Vec<String>,
    /// Keys only a full process restart applies, shown apart from the ones the
    /// header's restart button does cover.
    pub pending_process_restart: Vec<String>,
    pub policy: PolicyView,
    pub error: Option<String>,
    pub success: Option<String>,
    /// The page is the "restarting" notice rather than the settings form: the
    /// browser polls `/health` and comes back to `restart_return`.
    pub restarting: bool,
    pub restart_return: String,
    /// The generation this page was rendered by. The browser only reloads once
    /// `/health` reports a *different* one, so it cannot mistake the server
    /// that is still shutting down for the one that came back.
    pub restart_generation: u64,
    /// Why the last in-place restart kept the previous listener, if it did.
    pub restart_error: Option<String>,
    /// The measured sandbox, on the Sandbox page only.
    pub sandbox: Option<SandboxView>,
}

/// What this host actually gives, for the Sandbox page.
///
/// The grade is the one number an admin acts on; the items say which protection
/// is missing and what its absence means, the notes say what the platform still
/// leaves open, and the decision says whether the features run at all under the
/// policy the admin set. None of it is a claim: every value comes from the report
/// a confined child printed, plus the one question only this side can answer —
/// whether `sandbox.min_level` accepts it.
pub struct SandboxView {
    /// The effective value of `sandbox.enabled`.
    pub enabled: bool,
    /// The effective `sandbox.min_level`, already translated.
    pub min_level: String,
    /// The grade label, already translated.
    pub grade: String,
    /// `badge-green` when every protection is there, `badge-red` when the files
    /// layer is missing or nothing beyond resource limits is, `badge-gray`
    /// otherwise.
    pub grade_class: &'static str,
    /// Whether the features run, and why not when they do not.
    pub decision: SandboxDecisionView,
    /// The four items, in the order the page lists them.
    pub items: Vec<SandboxItemView>,
    /// The media profile's own line.
    pub media: SandboxMediaView,
    /// The one verdict the page leads with.
    pub verdict: SandboxVerdictView,
    /// The raw detail a probe printed, for the disclosure.
    pub detail: String,
}

/// Whether the policy lets the document and image profiles run.
///
/// The grade above says what the host *could* give; this says what the settings
/// made of it. They differ whenever `sandbox.min_level` refuses a host, and an
/// admin looking at a red item needs to know which of the two they are reading
/// before they change either.
pub struct SandboxDecisionView {
    /// Whether the features run.
    pub running: bool,
    /// `badge-green` when the features run, `badge-red` when they do not.
    pub class: &'static str,
    pub label: String,
    /// Why they do not run, when they do not.
    pub reason: Option<String>,
}

/// One protection: whether it is in place, how much it matters, and what its
/// absence means.
pub struct SandboxItemView {
    pub label_key: &'static str,
    pub present: bool,
    /// `badge-red` for a critical item that is missing, `badge-gray` otherwise.
    pub class: &'static str,
    /// 关键 / 重要 / 建议 — how much the item is worth, so that a host missing
    /// the files layer does not read like a host missing the process bound.
    pub severity: String,
    /// One sentence on what the absence opens, shown only when it is absent.
    pub impact: Option<String>,
    /// The residuals that weaken this item, already translated.
    pub notes: Vec<String>,
}

/// The media profile's line: it is a second child with its own report.
pub struct SandboxMediaView {
    /// What the media worker is doing, already translated.
    pub label: String,
    pub class: &'static str,
    /// The media profile's own grade, when it reported one.
    ///
    /// Its own because it is its own child: this is the profile that starts a
    /// program by definition, so it grades below the document profile on every
    /// platform, and saying so is the point of the row.
    pub grade: Option<String>,
    /// `badge-*` for [`SandboxMediaView::grade`].
    pub grade_class: &'static str,
    /// The notes on the media report, already translated.
    pub notes: Vec<String>,
    /// Why the media profile is not available, when it is not. Shown beside the
    /// row rather than only inside the raw report: a min-level that the media
    /// profile can never reach is the common cause, and it is not a fault.
    pub reason: Option<String>,
    /// That the process item does not apply to this profile, and what bounds the
    /// copies instead. Always present for media: the item cannot exist here, and
    /// leaving the row without it would read as an item that was overlooked.
    pub process_note: String,
    pub detail: String,
}

/// The one line the page leads with: is this configuration safe?
pub struct SandboxVerdictView {
    pub label: String,
    /// `badge-*` for the tone of the verdict.
    pub class: &'static str,
    /// `is-ok` / `is-warn` / `is-err` — the banner behind it. Separate from the
    /// badge because the page reads them at different sizes: a weak verdict is a
    /// grey badge in a soft banner, and an unsafe one is red in a loud one.
    pub banner: &'static str,
    /// The explanation, when the verdict is not simply "safe".
    pub detail: Option<String>,
}

/// Query parameters of a settings page.
#[derive(Deserialize, Default)]
pub struct SettingsQuery {
    /// Confirmation carried by the redirect that follows a successful POST.
    pub action: Option<String>,
}

/// Reasons a POST redirect can report, as locale keys.
fn success_message(t: &I18n, action: Option<&str>, restarted: bool) -> Option<String> {
    let key = match action {
        Some("saved") if restarted => "setting.saved_restart_pending",
        Some("saved") => "setting.saved",
        Some("reset") => "setting.reset_done",
        Some("refreshed") => "setting.refreshed",
        Some("restarted") => "setting.restarted",
        _ => return None,
    };
    Some(t.tr(key).to_string())
}

/// Parse a section id from the URL, or `None` for a path we do not serve.
fn section_of(id: &str) -> Option<Section> {
    Section::from_id(id)
}

/// GET /sysadmin/settings/ and /sysadmin/settings/{section}/.
pub async fn settings_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    path: Option<Path<String>>,
    Query(query): Query<SettingsQuery>,
) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }
    let section = match path.as_ref().map(|Path(id)| id.as_str()) {
        None => Section::Server,
        // An unknown page falls back to the first one rather than 404ing: the
        // area is a handful of links, and a stale bookmark should land somewhere
        // useful.
        Some(id) => section_of(id).unwrap_or(Section::Server),
    };
    let success = success_message(
        I18n::get(user.language.as_deref()),
        query.action.as_deref(),
        !state.settings.pending_restart().is_empty(),
    );
    let flags = RenderFlags {
        success,
        ..RenderFlags::default()
    };
    match render(&state, &user, section, flags, None).await {
        Ok(response) => response,
        Err(e) => e.into_response(),
    }
}

/// What one rendered settings page shows besides its rows.
#[derive(Default)]
struct RenderFlags {
    /// A rejected action, shown above the form.
    error: Option<String>,
    /// A completed action, shown above the form.
    success: Option<String>,
    /// The page is the "restart in progress" notice: no form, and the browser
    /// watches `/health` until the server is back.
    restarting: bool,
}

/// Build and render one section, carrying at most one banner.
async fn render(
    state: &Arc<AppState>,
    user: &WebUser,
    section: Section,
    flags: RenderFlags,
    submitted: Option<&HashMap<String, String>>,
) -> Result<Response, AppError> {
    let t = I18n::get(user.language.as_deref());
    let service = &state.settings;
    let policy = service.policy();
    let pending = service.pending_restart();

    let mut by_key: HashMap<&'static str, SettingRow> = HashMap::new();
    let mut drift_keys = Vec::new();
    for def in infra::settings::section(section) {
        let row = build_row(t, service, def, &pending, submitted);
        if row.config_value.is_some() {
            drift_keys.push(def.key.to_string());
        }
        by_key.insert(def.key, row);
    }
    let groups = build_groups(section, by_key);

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;
    let tpl = SystemSettingsTemplate {
        urls: ctx.urls,
        t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: ctx.csrf_token,
        active_page: "sysadmin_settings",
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,

        section: section.id(),
        section_title_key: section.title_key(),
        section_subtitle_key: section.subtitle_key(),
        sections: Section::ALL
            .iter()
            .map(|id| SectionLink {
                id: id.id(),
                label_key: id.title_key(),
                active: *id == section,
            })
            .collect(),
        groups,
        drift_keys,
        pending_restart: pending.iter().cloned().collect(),
        pending_process_restart: service.pending_process_restart().iter().cloned().collect(),
        policy: PolicyView {
            config_policy: match policy.config_policy {
                infra::settings::ConfigPolicy::Bootstrap => "bootstrap",
                infra::settings::ConfigPolicy::Override => "override",
            },
            override_keys: policy.config_override_keys.iter().cloned().collect(),
            refresh_interval_secs: policy.refresh_interval_secs,
        },
        error: flags.error,
        success: flags.success,
        restarting: flags.restarting,
        restart_return: settings_url(section, "restarted"),
        restart_generation: crate::restart::generation(),
        restart_error: crate::restart::failure(),
        sandbox: match section {
            Section::Sandbox => {
                // The two settings the panel needs, resolved here because the
                // service cannot move into the blocking task.
                let enabled = service
                    .resolved_one("sandbox.enabled")
                    .map(|entry| entry.value == "true")
                    .unwrap_or(true);
                let min_level = service
                    .resolved_one("sandbox.min_level")
                    .map(|entry| entry.value)
                    .unwrap_or_else(|| "partial".to_string());
                let min_level = crate::sandbox::Level::parse(&min_level)
                    .unwrap_or(crate::sandbox::Level::Partial);
                // The probe spawns a child per profile, so it belongs on a
                // blocking thread: the rest of the page must not wait on it, and
                // an executor thread must not be parked on a process.
                Some(
                    tokio::task::spawn_blocking(move || sandbox_view(t, enabled, min_level))
                        .await
                        .map_err(|e| {
                            AppError::internal(format!("the sandbox probe panicked: {e}"))
                        })?,
                )
            }
            _ => None,
        },
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// The URL of one section's page, with an optional action banner.
fn settings_url(section: Section, action: &str) -> String {
    match section {
        Section::Server => format!("/sysadmin/settings/?action={action}"),
        other => format!("/sysadmin/settings/{}/?action={action}", other.id()),
    }
}

/// What this host actually gives, for the Sandbox page.
///
/// The grade is measured, not asserted: it comes from a child that confined
/// itself and printed what took. The items say which protection is missing and
/// the notes say what the platform still leaves open, so an admin can tell "this
/// host cannot do better" from "something here is broken".
/// Build the Sandbox panel from what the host measured.
///
/// Synchronous, and it spawns children: every value on the panel comes from a
/// probe this call runs once per profile, so the caller hands it to
/// `spawn_blocking` rather than blocking the runtime's thread on a child process.
/// The two settings it needs are resolved by the caller for the same reason —
/// the settings service cannot cross into the blocking task.
fn sandbox_view(t: &'static I18n, enabled: bool, min_level: crate::sandbox::Level) -> SandboxView {
    use crate::sandbox::{Profile, worker};

    let answer = worker::page_answer(Profile::Documents);
    let (grade, grade_class, items, detail, host_level, decision) = match &answer {
        worker::PageAnswer::Measured {
            report,
            running,
            reason,
        } => {
            let level = report.level();
            let notes = report.notes();
            (
                t.tr(level_label(level)).to_string(),
                level_class(level),
                report
                    .protections
                    .items()
                    .into_iter()
                    .map(|(item, present)| SandboxItemView {
                        label_key: item_label(item),
                        present,
                        class: item_class(item, present),
                        severity: t.tr(item_severity(item)).to_string(),
                        impact: (!present).then(|| t.tr(item_impact(item)).to_string()),
                        notes: notes
                            .iter()
                            .filter(|note| note_belongs(note, item))
                            .map(|note| t.tr(note_label(note)).to_string())
                            .collect(),
                    })
                    .collect(),
                report.detail.clone(),
                Some(level),
                SandboxDecisionView {
                    running: *running,
                    class: if *running { "badge-green" } else { "badge-red" },
                    label: t
                        .tr(if *running {
                            "sandbox.decision_running"
                        } else {
                            "sandbox.decision_disabled"
                        })
                        .to_string(),
                    reason: reason.clone(),
                },
            )
        }
        worker::PageAnswer::Unavailable(why) => (
            t.tr("sandbox.grade_unavailable").to_string(),
            "badge-red",
            Vec::new(),
            why.clone(),
            None,
            SandboxDecisionView {
                running: false,
                class: "badge-red",
                label: t.tr("sandbox.decision_disabled").to_string(),
                reason: Some(why.clone()),
            },
        ),
    };

    // The media profile is a second child with its own report: it may execute
    // the helper where nothing else may, so it is shown on its own line — with
    // its own grade, because it is the profile that cannot have the process item
    // and therefore grades below the one above it on every platform.
    let media_answer = worker::page_answer(Profile::Media);
    let mut media_level = None;
    let mut media = match &media_answer {
        worker::PageAnswer::Measured { report, .. } => {
            media_level = Some(report.level());
            let (label, class) = if report.detail.contains("media-ok") {
                (t.tr("sandbox.media_ok"), "badge-green")
            } else if report.detail.contains("media-no-helper") {
                (t.tr("sandbox.media_no_helper"), "badge-gray")
            } else {
                (t.tr("sandbox.media_failed"), "badge-red")
            };
            SandboxMediaView {
                label: label.to_string(),
                class,
                grade: Some(t.tr(level_label(report.level())).to_string()),
                grade_class: level_class(report.level()),
                notes: report
                    .notes()
                    .into_iter()
                    .map(|note| t.tr(media_note_label(note)).to_string())
                    .collect(),
                reason: None,
                process_note: t.tr("sandbox.media_process_na").to_string(),
                detail: report.detail.clone(),
            }
        }
        worker::PageAnswer::Unavailable(why) => SandboxMediaView {
            label: t.tr("sandbox.media_unavailable").to_string(),
            class: "badge-red",
            grade: None,
            grade_class: "badge-red",
            notes: Vec::new(),
            reason: Some(why.clone()),
            process_note: t.tr("sandbox.media_process_na").to_string(),
            detail: why.clone(),
        },
    };
    // A minimum the media profile can never reach is not a fault to hunt: the
    // profile starts a program by definition, so with `sandbox.min_level` above
    // what it grades, every media request is refused. Saying so here is the
    // difference between "media thumbnails are off" and "media thumbnails are
    // off and here is the setting that did it".
    let media_refused_by_min = enabled && media_level.is_some_and(|level| level < min_level);
    if media_refused_by_min {
        media.label = t.tr("sandbox.media_below_min").to_string();
        media.class = "badge-red";
        media.reason = Some(t.tr("sandbox.media_below_min_reason").to_string());
    }

    let verdict = sandbox_verdict(t, enabled, &decision, &items, host_level);

    SandboxView {
        enabled,
        min_level: t.tr(min_level_label(min_level)).to_string(),
        grade,
        grade_class,
        decision,
        items,
        media,
        verdict,
        detail,
    }
}

/// The one line the page leads with.
///
/// Ordered by what an admin has to do first, and phrased as a verdict rather
/// than as a warning: "this host is missing a protection" and "the policy you
/// set accepts a host that lets a parser read your files" are different
/// sentences, and only the second is a decision the admin made. The files item
/// is what separates them — it is the one whose absence the shipped default
/// refuses, so seeing it missing means `sandbox.min_level` was lowered past the
/// point where the feature is safe.
fn sandbox_verdict(
    t: &I18n,
    enabled: bool,
    decision: &SandboxDecisionView,
    items: &[SandboxItemView],
    host_level: Option<crate::sandbox::Level>,
) -> SandboxVerdictView {
    use crate::sandbox::Level;

    /// The three tones the verdict can be read in, which decide both the badge
    /// beside it and the banner behind it. `Err` is the one that means something
    /// is wrong now rather than something being weaker than it could be.
    enum Tone {
        Ok,
        Weak,
        Bad,
    }
    let verdict = |label: &str, tone: Tone, detail: Option<String>| SandboxVerdictView {
        label: label.to_string(),
        class: match tone {
            Tone::Ok => "badge-green",
            Tone::Weak => "badge-gray",
            Tone::Bad => "badge-red",
        },
        banner: match tone {
            Tone::Ok => "is-ok",
            Tone::Weak => "is-warn",
            Tone::Bad => "is-err",
        },
        detail,
    };

    let missing = |name: &str| {
        items
            .iter()
            .find(|item| item.label_key == name)
            .is_some_and(|item| !item.present)
    };

    if !enabled {
        return verdict(
            t.tr("sandbox.verdict_disabled"),
            Tone::Weak,
            Some(t.tr("sandbox.warning_disabled").to_string()),
        );
    }
    if host_level.is_none() {
        return verdict(
            t.tr("sandbox.verdict_unavailable"),
            Tone::Bad,
            decision.reason.clone(),
        );
    }
    if !decision.running {
        // The host can confine the worker; the settings refuse what it gives.
        return verdict(
            t.tr("sandbox.verdict_refused"),
            Tone::Bad,
            Some(t.tr("sandbox.warning_below_min").to_string()),
        );
    }
    if missing("sandbox.item_files") {
        // Only reachable by lowering `min_level`: the default refuses this host,
        // so the admin chose to run without the one layer that keeps a parser out
        // of the server user's files.
        return verdict(
            t.tr("sandbox.verdict_unsafe"),
            Tone::Bad,
            Some(t.tr("sandbox.verdict_unsafe_detail").to_string()),
        );
    }
    match host_level {
        Some(Level::Full) => verdict(t.tr("sandbox.verdict_safe"), Tone::Ok, None),
        _ => verdict(
            t.tr("sandbox.verdict_degraded"),
            Tone::Weak,
            Some(t.tr("sandbox.warning_incomplete").to_string()),
        ),
    }
}

/// `badge-*` for one item: red only when a critical one is missing.
fn item_class(item: &str, present: bool) -> &'static str {
    match (item, present) {
        (_, true) => "badge-green",
        ("limits" | "files", false) => "badge-red",
        _ => "badge-gray",
    }
}

/// How much one item is worth, which is what tells an admin where to look first.
///
/// The files layer is critical because it is the one that keeps a parser out of
/// the server user's data; the network is important because it is how what a
/// parser did read would leave; the process bound is advice, because a parser
/// that cannot read or send anything has little to do with a second process.
fn item_severity(item: &str) -> &'static str {
    match item {
        "limits" | "files" => "sandbox.severity_critical",
        "network" => "sandbox.severity_important",
        _ => "sandbox.severity_advisory",
    }
}

/// What one item's absence means, in the terms the operator cares about.
fn item_impact(item: &str) -> &'static str {
    match item {
        "limits" => "sandbox.impact_limits",
        "files" => "sandbox.impact_files",
        "network" => "sandbox.impact_network",
        _ => "sandbox.impact_process",
    }
}

fn level_label(level: crate::sandbox::Level) -> &'static str {
    match level {
        crate::sandbox::Level::Full => "sandbox.grade_full",
        crate::sandbox::Level::Partial => "sandbox.grade_partial",
        crate::sandbox::Level::None => "sandbox.grade_none",
    }
}

fn level_class(level: crate::sandbox::Level) -> &'static str {
    match level {
        crate::sandbox::Level::Full => "badge-green",
        crate::sandbox::Level::Partial => "badge-gray",
        crate::sandbox::Level::None => "badge-red",
    }
}

fn item_label(item: &str) -> &'static str {
    match item {
        "limits" => "sandbox.item_limits",
        "files" => "sandbox.item_files",
        "network" => "sandbox.item_network",
        _ => "sandbox.item_process",
    }
}

/// Which item a residual note belongs beside.
fn note_belongs(note: &str, item: &str) -> bool {
    match note {
        "fork" | "helper" | "media_process" => item == "process",
        "system_tree" | "writes" | "writes_user" | "metadata" | "ll_gaps" | "helper_libs"
        | "helper_trees" | "container_plain" | "container_no_token" | "container_refused"
        | "lpac_off" => item == "files",
        _ => false,
    }
}

fn note_label(note: &str) -> &'static str {
    match note {
        "fork" => "sandbox.note_fork",
        "helper" => "sandbox.note_helper",
        "system_tree" => "sandbox.note_system_tree",
        "writes" => "sandbox.note_writes",
        "writes_user" => "sandbox.note_writes_user",
        "metadata" => "sandbox.note_metadata",
        "media_process" => "sandbox.note_media_process",
        "helper_libs" => "sandbox.note_helper_libs",
        "helper_trees" => "sandbox.note_helper_trees",
        "container_plain" => "sandbox.note_container_plain",
        "container_no_token" => "sandbox.note_container_no_token",
        "container_refused" => "sandbox.note_container_refused",
        "lpac_off" => "sandbox.note_lpac_off",
        _ => "sandbox.note_ll_gaps",
    }
}

/// The note key for the media row, where `fork` means something else.
///
/// On the document profile it is a platform concession that still leaves the
/// bound on starting programs; on the media profile the fork *is* how the helper
/// is started, so its note says what actually bounds the copies instead of
/// claiming a bound that is not there.
fn media_note_label(note: &str) -> &'static str {
    match note {
        "fork" => "sandbox.note_media_fork",
        other => note_label(other),
    }
}

fn min_level_label(level: crate::sandbox::Level) -> &'static str {
    match level {
        crate::sandbox::Level::Full => "setting.sandbox_min_level_option_full",
        crate::sandbox::Level::Partial => "setting.sandbox_min_level_option_partial",
        crate::sandbox::Level::None => "setting.sandbox_min_level_option_none",
    }
}

/// Assemble a page's rows into its groups, in render order.
///
/// A catalog key that `GROUPS` does not name would silently vanish from the
/// page, so anything left over is collected into a trailing "Other" group rather
/// than dropped; `the_groups_partition_the_catalog` is what stops that from
/// being the normal case.
fn build_groups(
    section: Section,
    mut rows: HashMap<&'static str, SettingRow>,
) -> Vec<SettingGroup> {
    let mut groups: Vec<SettingGroup> = Vec::new();
    for group in infra::settings::groups(section) {
        let held: Vec<SettingRow> = group
            .keys
            .iter()
            .filter_map(|key| rows.remove(key))
            .collect();
        if !held.is_empty() {
            groups.push(SettingGroup {
                id: group.id,
                title_key: group.title_key,
                rows: held,
            });
        }
    }
    // Catalog order, not map order, so the leftovers read like a page. Anything
    // else the map held (it should hold only this page's keys) follows by key,
    // so the order stays deterministic whatever it is.
    let mut leftovers: Vec<SettingRow> = infra::settings::section(section)
        .filter_map(|def| rows.remove(def.key))
        .collect();
    let mut rest: Vec<&'static str> = rows.keys().copied().collect();
    rest.sort_unstable();
    leftovers.extend(rest.into_iter().filter_map(|key| rows.remove(key)));
    if !leftovers.is_empty() {
        groups.push(SettingGroup {
            id: "other",
            title_key: "setting.group_other",
            rows: leftovers,
        });
    }
    groups
}

/// Render one catalog entry into its row view.
fn build_row(
    t: &I18n,
    service: &crate::settings::SettingsService,
    def: &'static SettingDef,
    pending: &BTreeSet<String>,
    submitted: Option<&HashMap<String, String>>,
) -> SettingRow {
    let resolved = service.resolved_one(def.key);
    let origin = resolved.as_ref().map(|entry| entry.origin);
    let stored_value = resolved
        .as_ref()
        .map(|entry| entry.value.clone())
        .unwrap_or_default();
    let submitted_value = submitted
        .and_then(|form| form.get(def.key).cloned())
        .filter(|_| def.kind != Kind::Secret);

    let is_secret = def.kind == Kind::Secret;
    let value = if is_secret {
        String::new()
    } else {
        submitted_value
            .clone()
            .unwrap_or_else(|| stored_value.clone())
    };

    // The config file's value is only interesting when a saved value is
    // superseding it: otherwise the file *is* the origin, or it agrees.
    let saved_supersedes = matches!(
        origin,
        Some(Origin::Database { .. }) | Some(Origin::Environment { .. })
    ) && (def.get)(service.base())
        != (def.get)(&infra::config::Config::default())
        && (def.get)(service.base()) != stored_value;
    let config_value = saved_supersedes.then(|| (def.get)(service.base()));

    // An enum's choices are named in the locale rather than shown as the wire
    // value: `starttls` and `lazy` are the server's vocabulary, not the
    // operator's.
    let options = match def.kind {
        Kind::Enum(options) => options
            .iter()
            .map(|option| SelectOption {
                value: (*option).to_string(),
                label: t.tr(&def.option_label_key(option)).to_string(),
                selected: value == *option,
            })
            .collect(),
        _ => Vec::new(),
    };

    let secret_state = service.secret_state(def.key);
    // The environment's variable is the only provenance a row prints: it is the
    // one an operator has to go and change. The rest is a tooltip on the badge.
    let (origin_id, origin_title, env_var, origin_at) = match origin {
        Some(Origin::Environment { var }) => {
            ("environment", String::new(), Some(var.to_string()), None)
        }
        Some(Origin::ConfigFile { key }) => ("config", format!("config.toml: {key}"), None, None),
        Some(Origin::Database {
            updated_at,
            updated_by,
        }) => (
            "database",
            match updated_by {
                Some(id) => t.trf("setting.saved_by", &[("id", id.to_string())]),
                None => String::new(),
            },
            None,
            (updated_at > 0).then_some(updated_at),
        ),
        Some(Origin::Default) | None => ("default", String::new(), None, None),
    };

    let read_only = !def.is_stored();
    let locked = read_only || matches!(origin, Some(Origin::Environment { .. }));

    // One state, by precedence. A value waiting for the next start is the most
    // urgent thing about its row; that a change would need a start at all comes
    // next; that the row cannot be edited here comes last.
    let state_badge = if pending.contains(def.key) {
        Some(StateBadge {
            class: "badge-red",
            label: t.tr("setting.pending_restart").to_string(),
        })
    } else if def.apply == Apply::ProcessRestart {
        Some(StateBadge {
            class: "badge-gray",
            label: t.tr("setting.apply_process_restart").to_string(),
        })
    } else if def.apply.is_in_place_restart() {
        Some(StateBadge {
            class: "badge-gray",
            label: t.tr("setting.apply_restart").to_string(),
        })
    } else if read_only {
        Some(StateBadge {
            class: "badge-gray",
            label: t.tr("setting.apply_read_only").to_string(),
        })
    } else {
        None
    };

    // `tr` borrows the key it is given, so the strings are owned before they
    // outlive the temporaries.
    let label = t.tr(&def.label_key()).to_string();
    let help = t.tr(&def.help_key()).to_string();
    let search_text = format!("{} {label} {help}", def.key);
    SettingRow {
        key: def.key.to_string(),
        search_text,
        label_key: def.label_key(),
        help,
        unit: def.unit_key().map(|key| t.tr(&key).to_string()),
        control: match def.kind {
            Kind::Bool => "bool",
            Kind::U16 | Kind::U32 | Kind::U64 | Kind::I32 | Kind::Usize => "number",
            Kind::Enum(_) => "enum",
            Kind::TextList | Kind::NumList => "list",
            Kind::Secret => "secret",
            Kind::Path => "path",
            Kind::Text | Kind::OptText | Kind::OptBool => "text",
        },
        field: if is_secret {
            format!("secret:{}", def.key)
        } else {
            def.key.to_string()
        },
        checked: value == "true",
        value,
        options,
        origin: origin_id,
        origin_title,
        env_var,
        origin_at,
        config_value,
        state_badge,
        locked,
        read_only,
        secret_set: matches!(
            secret_state,
            Some(SecretState::Stored)
                | Some(SecretState::FromEnvironment)
                | Some(SecretState::FromConfigFile)
        ),
        secret_broken: secret_state == Some(SecretState::StoredUnreadable),
    }
}

// ─── POST handlers ────────────────────────────────────────────────────────

fn require_admin_csrf(
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

/// Fold a submitted body into a map, letting the **last** value of a repeated
/// field win.
///
/// A checkbox submits a hidden `false` followed by a checked `true`, so
/// last-wins is what makes an unchecked box mean `false` rather than "absent".
fn fold_form(pairs: &[(String, String)]) -> HashMap<String, String> {
    pairs.iter().cloned().collect()
}

/// Build the service's form from the submitted body.
///
/// A secret is three-way: filled in = replace, the clear box = erase, and a
/// rendered-but-empty field = leave what is stored alone. A checkbox that is
/// absent submits its hidden `false`, so a boolean has no "absent" case.
fn parse_form(
    section: Section,
    form: &HashMap<String, String>,
) -> Result<crate::settings::SettingsForm, AppError> {
    let mut values = BTreeMap::new();
    let mut secrets = BTreeMap::new();

    for def in infra::settings::section(section) {
        if !def.is_stored() {
            continue;
        }
        if def.kind == Kind::Secret {
            let field = format!("secret:{}", def.key);
            let choice = if form.contains_key(&format!("clear:{}", def.key)) {
                Some(String::new())
            } else {
                match form.get(&field) {
                    Some(value) if !value.is_empty() => Some(value.clone()),
                    _ => None,
                }
            };
            secrets.insert(def.key.to_string(), choice);
        } else if let Some(value) = form.get(def.key) {
            values.insert(def.key.to_string(), value.clone());
        }
    }

    Ok(crate::settings::SettingsForm { values, secrets })
}

/// POST /sysadmin/settings/{section}/save/.
pub async fn save(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = fold_form(&pairs);
    require_admin_csrf(&state, &user, &form)?;

    let Some(section) = section_of(&id) else {
        return Err(AppError::BadRequest("unknown settings page".to_string()));
    };

    let update = parse_form(section, &form)?;
    match state
        .settings
        .save(section, &update, Some(user.user_id))
        .await
    {
        Ok(outcome) => {
            state.apply_settings_hooks(&outcome.hooks);
            tracing::info!(
                keys = ?outcome.changed.iter().map(String::as_str).collect::<Vec<_>>(),
                admin = user.user_id,
                section = section.id(),
                "settings saved"
            );
            let action = if outcome.restart_pending.is_empty() {
                "saved"
            } else {
                "saved&restart=1"
            };
            Ok((
                StatusCode::FOUND,
                [(
                    "Location",
                    format!("/sysadmin/settings/{}/?action={action}", section.id()),
                )],
            )
                .into_response())
        }
        // Re-render with the reason, and with what was submitted: a rejected
        // value (a bad port, an invalid sender) has to be visible on the form
        // that produced it.
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            let flags = RenderFlags {
                error: Some(msg),
                ..RenderFlags::default()
            };
            render(&state, &user, section, flags, Some(&form)).await
        }
    }
}

/// POST /sysadmin/settings/reset/ — drop the saved override for one key, so the
/// config file (or the built-in default) supplies it again.
pub async fn reset(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = fold_form(&pairs);
    require_admin_csrf(&state, &user, &form)?;

    let key = form
        .get("key")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("no setting given".to_string()))?;
    let section = form
        .get("section")
        .and_then(|id| section_of(id))
        .ok_or_else(|| AppError::BadRequest("unknown settings page".to_string()))?;
    // Only a key that belongs to the page being viewed, so a hand-typed form
    // cannot clear a setting the caller was not shown.
    if !infra::settings::section(section).any(|def| def.key == key) {
        return Err(AppError::BadRequest(
            "that setting is not on this page".to_string(),
        ));
    }

    match state.settings.clear(std::slice::from_ref(&key)).await {
        Ok(outcome) => {
            state.apply_settings_hooks(&outcome.hooks);
            tracing::info!(key = %key, admin = user.user_id, "setting override cleared");
            Ok((
                StatusCode::FOUND,
                [(
                    "Location",
                    format!("/sysadmin/settings/{}/?action=reset", section.id()),
                )],
            )
                .into_response())
        }
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            let flags = RenderFlags {
                error: Some(msg),
                ..RenderFlags::default()
            };
            render(&state, &user, section, flags, None).await
        }
    }
}

/// POST /sysadmin/settings/restart/ — restart the server in place.
///
/// The signal is raised *after* this handler has returned and the response is
/// on its way out: the run loop tears the server down through the normal
/// graceful shutdown, so an in-flight response is always delivered. The
/// submitted values are deliberately ignored — restarting is not a save, and a
/// silent save would be the wrong side effect for a button labelled "restart".
pub async fn restart(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = fold_form(&pairs);
    require_admin_csrf(&state, &user, &form)?;
    let section = form
        .get("section")
        .and_then(|id| section_of(id))
        .unwrap_or(Section::Server);

    tracing::warn!(
        admin = user.user_id,
        section = section.id(),
        "restart requested from the settings page"
    );

    let signal = state.restart.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        signal.request();
    });

    let flags = RenderFlags {
        restarting: true,
        ..RenderFlags::default()
    };
    render(&state, &user, section, flags, None).await
}

/// POST /sysadmin/settings/refresh/ — re-read the settings table now.
pub async fn refresh(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = fold_form(&pairs);
    require_admin_csrf(&state, &user, &form)?;
    let section = form
        .get("section")
        .and_then(|id| section_of(id))
        .unwrap_or(Section::Server);

    let outcome = state.settings.reload().await?;
    if !outcome.is_empty() {
        state.apply_settings_hooks(&outcome.hooks);
        tracing::info!(
            keys = ?outcome.changed.iter().map(String::as_str).collect::<Vec<_>>(),
            "settings re-read from the database"
        );
    }
    Ok((
        StatusCode::FOUND,
        [(
            "Location",
            format!("/sysadmin/settings/{}/?action=refreshed", section.id()),
        )],
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

    /// The media row reads `fork` differently from the document profile's
    /// process item: there the fork is how the helper is started, so it gets
    /// its own sentence, and every other note keeps the shared one.
    #[test]
    fn the_media_row_has_its_own_sentence_for_fork() {
        assert_eq!(media_note_label("fork"), "sandbox.note_media_fork");
        assert_eq!(media_note_label("helper"), note_label("helper"));
        assert_eq!(media_note_label("ll_gaps"), note_label("ll_gaps"));
        assert_eq!(
            media_note_label("something_else"),
            note_label("something_else")
        );
        assert_eq!(note_label("fork"), "sandbox.note_fork");
    }

    /// The notes belong to the item they weaken, so the page shows each one
    /// exactly once and under the right protection.
    #[test]
    fn a_note_is_shown_under_the_item_it_weakens() {
        assert!(note_belongs("fork", "process"));
        assert!(note_belongs("helper", "process"));
        assert!(note_belongs("media_process", "process"));
        assert!(note_belongs("system_tree", "files"));
        assert!(note_belongs("writes", "files"));
        assert!(note_belongs("writes_user", "files"));
        assert!(note_belongs("helper_libs", "files"));
        assert!(note_belongs("helper_trees", "files"));
        assert!(note_belongs("container_plain", "files"));
        assert!(note_belongs("container_no_token", "files"));
        assert!(note_belongs("container_refused", "files"));
        assert!(note_belongs("lpac_off", "files"));
        assert!(note_belongs("metadata", "files"));
        assert!(note_belongs("ll_gaps", "files"));
        assert!(!note_belongs("helper", "files"));
        assert!(!note_belongs("system_tree", "process"));
        assert!(!note_belongs("unknown", "process"));
    }

    /// Every note key the report can produce has a label, so a new token cannot
    /// reach the page as its own identifier.
    #[test]
    fn every_note_has_a_label() {
        for note in [
            "fork",
            "helper",
            "media_process",
            "system_tree",
            "writes",
            "writes_user",
            "metadata",
            "helper_libs",
            "helper_trees",
            "container_plain",
            "container_no_token",
            "container_refused",
            "lpac_off",
            "ll_gaps",
        ] {
            let label = note_label(note);
            assert_ne!(label, note, "{note} has no label of its own");
            assert!(label.starts_with("sandbox.note_"), "{note} -> {label}");
        }
    }

    /// The verdict is what tells an admin whether the *policy* they chose is
    /// safe, which is a different question from how strong the host is. The case
    /// that matters most is the one the shipped default refuses and a lowered
    /// minimum accepts: a host whose files layer is missing.
    #[test]
    fn the_verdict_separates_an_unsafe_policy_from_a_weak_host() {
        let t = I18n::get(Some("en"));
        /// The files item, present or not — the one the verdict turns on.
        fn files_item(present: bool) -> SandboxItemView {
            SandboxItemView {
                label_key: "sandbox.item_files",
                present,
                class: if present { "badge-green" } else { "badge-red" },
                severity: String::new(),
                impact: None,
                notes: Vec::new(),
            }
        }
        let decision = |running: bool| SandboxDecisionView {
            running,
            class: if running { "badge-green" } else { "badge-red" },
            label: String::new(),
            reason: (!running).then(|| "below the minimum".to_string()),
        };
        let level = crate::sandbox::Level::None;

        // Running, but without the files layer: only a lowered minimum gets
        // here, and the verdict says so.
        let unsafe_verdict =
            sandbox_verdict(t, true, &decision(true), &[files_item(false)], Some(level));
        assert_eq!(unsafe_verdict.label, t.tr("sandbox.verdict_unsafe"));
        assert!(unsafe_verdict.detail.is_some());

        // The same host with the shipped default: the policy refuses it, and
        // that is a different verdict with a different remedy.
        let refused = sandbox_verdict(t, true, &decision(false), &[files_item(false)], Some(level));
        assert_eq!(refused.label, t.tr("sandbox.verdict_refused"));

        // Files in place and everything else too: nothing to explain.
        let safe = sandbox_verdict(
            t,
            true,
            &decision(true),
            &[files_item(true)],
            Some(crate::sandbox::Level::Full),
        );
        assert_eq!(safe.label, t.tr("sandbox.verdict_safe"));
        assert!(safe.detail.is_none());
    }

    #[test]
    fn an_unchecked_box_submits_the_hidden_false() {
        // What the browser sends for a checkbox that is off: only the hidden
        // field. Last-wins folding makes both cases look the same to the parser.
        let off = fold_form(&[
            ("server.share_link_enabled".to_string(), "false".to_string()),
            ("csrf_token".to_string(), "x".to_string()),
        ]);
        assert_eq!(off.get("server.share_link_enabled").unwrap(), "false");

        let on = fold_form(&[
            ("server.share_link_enabled".to_string(), "false".to_string()),
            ("server.share_link_enabled".to_string(), "true".to_string()),
        ]);
        assert_eq!(on.get("server.share_link_enabled").unwrap(), "true");
    }

    #[test]
    fn a_secret_is_three_way_and_never_read_back() {
        // Blank field: keep what is stored.
        let parsed = parse_form(
            Section::Email,
            &form(&[("secret:email.password", ""), ("csrf_token", "x")]),
        )
        .unwrap();
        assert_eq!(parsed.secrets.get("email.password"), Some(&None));

        // Filled in: replace.
        let parsed = parse_form(
            Section::Email,
            &form(&[("secret:email.password", "hunter2")]),
        )
        .unwrap();
        assert_eq!(
            parsed.secrets.get("email.password"),
            Some(&Some("hunter2".to_string()))
        );

        // Cleared: erase, even if the field still carries a value.
        let parsed = parse_form(
            Section::Email,
            &form(&[
                ("secret:email.password", "hunter2"),
                ("clear:email.password", "on"),
            ]),
        )
        .unwrap();
        assert_eq!(
            parsed.secrets.get("email.password"),
            Some(&Some(String::new()))
        );

        // A read-only secret is never submitted, whatever the form says.
        let parsed = parse_form(
            Section::Security,
            &form(&[("secret:server.secret_key", "injected")]),
        )
        .unwrap();
        assert!(!parsed.secrets.contains_key("server.secret_key"));
    }

    #[test]
    fn only_the_pages_own_keys_are_parsed() {
        let parsed = parse_form(
            Section::Server,
            &form(&[
                ("server.port", "8082"),
                ("server.share_link_enabled", "false"),
            ]),
        )
        .unwrap();
        assert_eq!(parsed.values.get("server.port").unwrap(), "8082");
        assert!(
            !parsed.values.contains_key("server.share_link_enabled"),
            "a key from another page must be ignored, not silently saved"
        );
    }

    #[test]
    fn banners_are_whitelisted_and_distinguish_a_pending_restart() {
        let t = I18n::get(Some("en"));
        assert!(success_message(t, Some("saved"), false).is_some());
        assert_ne!(
            success_message(t, Some("saved"), false),
            success_message(t, Some("saved"), true),
            "a change that needs a restart must say so"
        );
        assert!(success_message(t, Some("../../etc/passwd"), false).is_none());
        assert!(success_message(t, None, false).is_none());
    }

    #[test]
    fn section_ids_round_trip() {
        for section in Section::ALL {
            assert_eq!(section_of(section.id()), Some(section));
        }
        assert_eq!(section_of("nope"), None);
    }

    /// A row with nothing in it: the grouping code only looks at `key`.
    fn row(key: &'static str) -> SettingRow {
        SettingRow {
            key: key.to_string(),
            search_text: String::new(),
            label_key: String::new(),
            help: String::new(),
            unit: None,
            control: "text",
            value: String::new(),
            checked: false,
            options: Vec::new(),
            field: String::new(),
            origin: "default",
            origin_title: String::new(),
            env_var: None,
            origin_at: None,
            config_value: None,
            state_badge: None,
            locked: false,
            read_only: false,
            secret_set: false,
            secret_broken: false,
        }
    }

    #[test]
    fn a_page_is_rendered_as_its_groups_in_order() {
        let mut rows = HashMap::new();
        for def in infra::settings::section(Section::RateLimits) {
            rows.insert(def.key, row(def.key));
        }
        let groups = build_groups(Section::RateLimits, rows);
        let ids: Vec<&str> = groups.iter().map(|group| group.id).collect();
        assert_eq!(
            ids,
            vec![
                "rate_limits_sign_in",
                "rate_limits_account_flows",
                "rate_limits_protected",
                "rate_limits_content",
            ]
        );
        let listed: Vec<&str> = groups
            .iter()
            .flat_map(|group| group.rows.iter().map(|row| row.key.as_str()))
            .collect();
        let declared: Vec<&str> = infra::settings::section(Section::RateLimits)
            .map(|def| def.key)
            .collect();
        assert_eq!(listed, declared);
    }

    /// A key with no group still has to reach the page: silently dropping a
    /// setting is worse than showing it under a heading that says "Other".
    #[test]
    fn a_key_with_no_group_is_not_dropped() {
        let mut rows = HashMap::new();
        rows.insert("server.addr", row("server.addr"));
        rows.insert("email.host", row("email.host"));
        let groups = build_groups(Section::Server, rows);
        let last = groups.last().expect("a fallback group");
        assert_eq!(last.id, "other");
        assert_eq!(last.title_key, "setting.group_other");
        let keys: Vec<&str> = last.rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, vec!["email.host"]);
    }
}
