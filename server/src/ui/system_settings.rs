//! Admin Web UI — system management: every runtime setting, one page per area.
//!
//! The page is *data-driven*: it renders the catalog (`infra::settings::CATALOG`)
//! rather than naming fields, so a setting cannot exist without appearing here,
//! and each row says what the setting does, where its effective value came from,
//! and the one state it is in.
//!
//! Read-only settings (the master secret, the database URL, the state
//! directories) are shown but not editable, and a secret is never rendered.

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
use crate::sandbox::{JobVerdict, Profile, Protections, worker};
use crate::settings::SecretState;
use base::error::AppError;
use infra::settings::{Apply, Kind, Origin, Section, SettingDef};

use super::auth_extractor::WebUser;

pub struct SettingRow {
    pub key: String,
    pub label_key: String,
    /// Everything the filter matches this row against.
    pub search_text: String,
    /// The one sentence that says what this setting does, in both languages;
    /// `every_catalog_string_is_translated` keeps it that way.
    pub help: String,
    /// The unit the value is expressed in, already translated.
    pub unit: Option<String>,
    /// The form control: `bool`, `number`, `text`, `enum`, `list`, `path`, `secret`.
    pub control: &'static str,
    /// The value the control starts with (never a secret's value).
    pub value: String,
    pub checked: bool,
    /// The `<option>`s of an enum control, with the current one marked.
    pub options: Vec<SelectOption>,
    /// The form field name is the key for a value, prefixed for a secret.
    pub field: String,
    /// Where the effective value came from: `environment`, `config`, `database`, `default`.
    pub origin: &'static str,
    /// The origin badge's tooltip: the `config.toml` key that supplied the value,
    /// or who saved it.
    pub origin_title: String,
    /// The `NANOFILE_*` variable the value came from, when the environment owns
    /// it: the only provenance a row prints as text.
    pub env_var: Option<String>,
    /// The `NANOFILE_*` variable that would override this setting, printed on every
    /// row unless [`Self::env_var`] already named it, so a variable appears once.
    pub env_name: Option<String>,
    /// Unix seconds of the last save, for a database-sourced value.
    pub origin_at: Option<i64>,
    /// The value in the config file, when a saved value is superseding it.
    pub config_value: Option<String>,
    /// The one state the row is in, by precedence: waiting for a restart
    /// outranks needing one, which outranks being read-only.
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

pub struct StateBadge {
    /// `badge-red` for something the operator still has to do, `badge-gray` otherwise.
    pub class: &'static str,
    pub label: String,
}

/// One group of rows on a page, ready for the template.
pub struct SettingGroup {
    pub id: &'static str,
    pub title_key: &'static str,
    pub rows: Vec<SettingRow>,
}

pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

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
    pub sharing_enabled: bool,
    pub current_repo_id: Option<String>,

    pub section: &'static str,
    pub section_title_key: &'static str,
    pub section_subtitle_key: &'static str,
    pub sections: Vec<SectionLink>,
    pub groups: Vec<SettingGroup>,
    /// How many rows this page holds, for the filter's "12 of 47" count.
    pub row_count: usize,
    /// Keys whose saved value supersedes a config-file entry that disagrees.
    pub drift_keys: Vec<String>,
    pub pending_restart: Vec<String>,
    /// Keys only a full process restart applies, shown apart from the rest.
    pub pending_process_restart: Vec<String>,
    pub policy: PolicyView,
    pub error: Option<String>,
    pub success: Option<String>,
    /// The page is the "restarting" notice: the browser polls `/health` and comes
    /// back to `restart_return`.
    pub restarting: bool,
    pub restart_return: String,
    /// The generation this page was rendered by. The browser reloads only once
    /// `/health` reports a *different* one, not the server still shutting down.
    pub restart_generation: u64,
    /// Why the last in-place restart kept the previous listener, if it did.
    pub restart_error: Option<String>,
    /// The measured sandbox, on the Sandbox page only.
    pub sandbox: Option<SandboxView>,
}

/// What this host actually gives, for the Sandbox page: one band per measured
/// profile, each with its own state and four items.
///
/// Every value comes from the report a confined child printed, so the page cannot
/// claim protection the host does not provide; the master switch is separate.
pub struct SandboxView {
    /// The profiles the page measured, in the order it lists them.
    pub profiles: Vec<SandboxProfileView>,
    /// The raw reports, one per profile, so the disclosure can label them.
    pub raw: Vec<SandboxRawView>,
}

/// One profile's own band: what it is for, whether it runs, and its protections.
pub struct SandboxProfileView {
    /// Stable identifier: `documents` or `media`.
    pub id: &'static str,
    pub title_key: &'static str,
    /// One line naming what the profile serves, already translated.
    pub uses: String,
    pub icon: &'static str,
    /// Whether the host answered for this profile at all: not the same question
    /// as the state, which says whether it runs. A probe that never reported
    /// leaves the band with a state and nothing under it.
    pub measured: bool,
    pub state: SandboxStateView,
    /// The four items, in the order the report carries them.
    pub items: Vec<SandboxItemView>,
}

/// Whether one profile runs, and why not when it does not.
///
/// Not the same question as an item: a profile can have every protection and not
/// run, and can run with one missing.
pub struct SandboxStateView {
    pub label: String,
    /// `badge-green` when it runs, `badge-red` when the host cannot, `badge-gray`
    /// otherwise.
    pub class: &'static str,
    /// Why it does not run, when it does not.
    pub reason: Option<String>,
    /// Where to go to fix it, when the fix is a setting on another page.
    pub action: Option<SandboxActionView>,
}

/// A link to the page that owns the setting a state names.
pub struct SandboxActionView {
    pub href: &'static str,
    pub label_key: &'static str,
}

/// One protection: whether it is in place, how much it matters, and what its absence means.
pub struct SandboxItemView {
    /// The report's own token: `limits`, `files`, `network` or `process`.
    pub key: &'static str,
    pub label_key: &'static str,
    /// The mark the row leads with: a name, so the template owns the glyph.
    pub icon: &'static str,
    pub present: bool,
    /// Whether the item applies at all: the media profile's process item is not a gap.
    pub applicable: bool,
    /// The one item no profile may run without.
    pub required: bool,
    /// `badge-red` for a critical item that is missing, `badge-gray` otherwise.
    pub class: &'static str,
    /// 关键 / 重要 / 建议 — how much the item is worth, shown only when it is missing.
    pub severity: String,
    pub show_severity: bool,
    /// One sentence on what the absence opens, shown only when it is absent.
    pub impact: Option<String>,
    /// The profile's own explanation that the item cannot exist for it.
    pub note: Option<String>,
    /// The residuals that weaken this item, already translated.
    pub notes: Vec<String>,
}

/// One raw probe report, labelled so the two lines can be told apart.
pub struct SandboxRawView {
    pub id: &'static str,
    pub title_key: &'static str,
    pub detail: String,
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
        // An unknown page falls back to the first one rather than 404ing.
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
        sharing_enabled: ctx.sharing_enabled,
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
        row_count: groups.iter().map(|group| group.rows.len()).sum(),
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
                // Resolved here because the service cannot move into the blocking
                // task. The index switch is the profile's own question: document
                // extraction exists for the search index, on a page of its own.
                let resolved = |key: &str| {
                    service
                        .resolved_one(key)
                        .map(|entry| entry.value == "true")
                        .unwrap_or(true)
                };
                let inputs = SandboxInputs {
                    enabled: resolved("sandbox.enabled"),
                    index_enabled: resolved("index.enabled"),
                };
                // The probe spawns a child per profile, so it belongs on a blocking
                // thread: the page must not wait on it, and an executor thread must
                // not be parked on a process.
                Some(
                    tokio::task::spawn_blocking(move || sandbox_view(t, inputs))
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

/// The settings the Sandbox page needs before it can render the measurement.
///
/// Resolved by the caller: the settings service cannot cross into the blocking task.
pub struct SandboxInputs {
    pub enabled: bool,
    /// The effective value of `index.enabled`, which owns document extraction.
    pub index_enabled: bool,
}

/// What this host actually gives, for the Sandbox page.
///
/// Every value is measured, not asserted, from confined children that printed
/// what took: the items say which protection is in place, the notes what the
/// platform still leaves open. It spawns children, so the caller hands it to
/// `spawn_blocking`; the answers are cached per profile.
fn sandbox_view(t: &'static I18n, inputs: SandboxInputs) -> SandboxView {
    let documents = worker::page_answer(Profile::Documents);
    let media = worker::page_answer(Profile::Media);

    let profiles = vec![
        documents_profile(t, &documents, &inputs),
        media_profile(t, &media, inputs.enabled),
    ];
    let raw = vec![
        SandboxRawView {
            id: "documents",
            title_key: "sandbox.profile_documents",
            detail: report_detail(&documents),
        },
        SandboxRawView {
            id: "media",
            title_key: "sandbox.profile_media",
            detail: report_detail(&media),
        },
    ];

    SandboxView { profiles, raw }
}

/// One profile's band: its state, its items, and the notes on each item, attached
/// to the item they weaken rather than listed under the profile.
fn profile_view(
    t: &I18n,
    profile: crate::sandbox::Profile,
    answer: &worker::PageAnswer,
    state: SandboxStateView,
) -> SandboxProfileView {
    let (measured, items) = match answer {
        worker::PageAnswer::Measured { report, .. } => {
            let notes = report.notes();
            let items = report
                .protections
                .items()
                .into_iter()
                .map(|(item, present)| {
                    let applicable = item_applicable(profile, item);
                    SandboxItemView {
                        key: item,
                        label_key: item_label(item),
                        icon: item_icon(item),
                        present,
                        applicable,
                        required: Protections::required(item),
                        class: item_class(item, present, applicable),
                        severity: t.tr(item_severity(item)).to_string(),
                        show_severity: applicable && !present,
                        impact: (applicable && !present)
                            .then(|| t.tr(item_impact(item)).to_string()),
                        note: (!applicable).then(|| t.tr("sandbox.media_process_na").to_string()),
                        notes: notes
                            .iter()
                            .filter(|note| note_belongs(note, item))
                            .map(|note| {
                                let key = if profile.runs_helper() {
                                    media_note_label(note)
                                } else {
                                    note_label(note)
                                };
                                t.tr(key).to_string()
                            })
                            .collect(),
                    }
                })
                .collect();
            (true, items)
        }
        worker::PageAnswer::Unavailable(_) => (false, Vec::new()),
    };

    SandboxProfileView {
        id: profile.as_str(),
        title_key: profile_title(profile),
        uses: t.tr(profile_uses(profile)).to_string(),
        icon: profile_icon(profile),
        measured,
        state,
        items,
    }
}

/// The parser profiles' band: documents and images, one band because both install
/// the same protections with the same grants, asserted by `sandbox_probe_test`.
fn documents_profile(
    t: &I18n,
    answer: &worker::PageAnswer,
    inputs: &SandboxInputs,
) -> SandboxProfileView {
    let state = if !inputs.enabled {
        disabled_state(t)
    } else if !inputs.index_enabled {
        // Document extraction has its own switch, on another page: no index, no extraction.
        SandboxStateView {
            label: t.tr("sandbox.state_index_off").to_string(),
            class: "badge-gray",
            reason: None,
            action: Some(SandboxActionView {
                href: "/sysadmin/settings/maintenance/",
                label_key: "sandbox.action_index",
            }),
        }
    } else {
        match answer {
            worker::PageAnswer::Measured {
                host_runs: true, ..
            } => running_state(t),
            worker::PageAnswer::Measured {
                host_runs: false,
                host_reason,
                ..
            } => unavailable_state(t, host_reason.clone().unwrap_or_default()),
            worker::PageAnswer::Unavailable(why) => unavailable_state(t, why.clone()),
        }
    };

    profile_view(t, Profile::Documents, answer, state)
}

/// The media profile's band: the one child that may start the configured helper.
///
/// Its state is its own question: a host can confine it perfectly and the helper
/// still not decode.
fn media_profile(t: &I18n, answer: &worker::PageAnswer, enabled: bool) -> SandboxProfileView {
    let state = if !enabled {
        disabled_state(t)
    } else {
        match answer {
            worker::PageAnswer::Measured {
                report,
                host_runs,
                host_reason,
            } => {
                if report.job == Some(JobVerdict::Failed) {
                    SandboxStateView {
                        label: t.tr("sandbox.state_media_failed").to_string(),
                        class: "badge-red",
                        reason: Some(host_reason.clone().unwrap_or_else(|| report.detail.clone())),
                        action: None,
                    }
                } else if !host_runs {
                    unavailable_state(t, host_reason.clone().unwrap_or_default())
                } else if report.job == Some(JobVerdict::NoHelper) {
                    // A state an administrator chose, not a fault: the fix is the
                    // helper path on the storage page.
                    SandboxStateView {
                        label: t.tr("sandbox.state_no_helper").to_string(),
                        class: "badge-gray",
                        reason: None,
                        action: Some(SandboxActionView {
                            href: "/sysadmin/settings/storage/",
                            label_key: "sandbox.action_storage",
                        }),
                    }
                } else {
                    running_state(t)
                }
            }
            worker::PageAnswer::Unavailable(why) => unavailable_state(t, why.clone()),
        }
    };

    profile_view(t, Profile::Media, answer, state)
}

fn running_state(t: &I18n) -> SandboxStateView {
    SandboxStateView {
        label: t.tr("sandbox.state_running").to_string(),
        class: "badge-green",
        reason: None,
        action: None,
    }
}

/// A profile the master switch stopped.
///
/// No reason of its own: the switch's row carries the cost of switching off.
fn disabled_state(t: &I18n) -> SandboxStateView {
    SandboxStateView {
        label: t.tr("sandbox.state_disabled").to_string(),
        class: "badge-gray",
        reason: None,
        action: None,
    }
}

fn unavailable_state(t: &I18n, why: String) -> SandboxStateView {
    SandboxStateView {
        label: t.tr("sandbox.state_unavailable").to_string(),
        class: "badge-red",
        reason: Some(why),
        action: None,
    }
}

/// The raw line one profile printed, or why it printed nothing.
fn report_detail(answer: &worker::PageAnswer) -> String {
    match answer {
        worker::PageAnswer::Measured { report, .. } => report.detail.clone(),
        worker::PageAnswer::Unavailable(why) => why.clone(),
    }
}

/// `badge-*` for one item: red only when a critical one is missing.
fn item_class(item: &str, present: bool, applicable: bool) -> &'static str {
    match (applicable, item, present) {
        (false, _, _) => "badge-gray",
        (_, _, true) => "badge-green",
        (_, "limits" | "files", false) => "badge-red",
        _ => "badge-gray",
    }
}

/// Whether one item is a question at all for one profile: the media profile
/// starts the helper by definition, so its process item cannot exist.
fn item_applicable(profile: Profile, item: &str) -> bool {
    !(profile.runs_helper() && item == "process")
}

/// Which band a profile belongs to, and what it serves.
fn profile_title(profile: Profile) -> &'static str {
    match profile {
        Profile::Media => "sandbox.profile_media",
        _ => "sandbox.profile_documents",
    }
}

fn profile_uses(profile: Profile) -> &'static str {
    match profile {
        Profile::Media => "sandbox.uses_media",
        _ => "sandbox.uses_documents",
    }
}

fn profile_icon(profile: Profile) -> &'static str {
    match profile {
        Profile::Media => "camera",
        _ => "file_text",
    }
}

/// How much one item is worth, which is what tells an admin where to look first.
///
/// The files layer is critical: it keeps a parser out of the user's data. The
/// network is important: it is how what a parser read would leave. Process is advice.
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

/// The mark beside one protection: a name, since the template owns the SVG.
fn item_icon(item: &str) -> &'static str {
    match item {
        "limits" => "clock",
        "files" => "lock",
        "network" => "share",
        _ => "cog",
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
        "fork" | "helper" | "media_process" | "token_unrestricted" => item == "process",
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
        "token_unrestricted" => "sandbox.note_token_unrestricted",
        _ => "sandbox.note_ll_gaps",
    }
}

/// The note key for the media row, where `fork` means something else.
///
/// On the document profile it is a platform concession that leaves the bound on
/// starting programs; on the media profile the fork *is* how the helper is started.
fn media_note_label(note: &str) -> &'static str {
    match note {
        "fork" => "sandbox.note_media_fork",
        other => note_label(other),
    }
}

/// Assemble a page's rows into its groups, in render order.
///
/// A catalog key that `GROUPS` does not name would silently vanish, so leftovers
/// go into a trailing "Other" group; `the_groups_partition_the_catalog` covers it.
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
    // Catalog order, not map order, so the leftovers read like a page; anything
    // else follows by key, so the order stays deterministic.
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

    // The config file's value matters only when a saved value supersedes it.
    let saved_supersedes = matches!(
        origin,
        Some(Origin::Database { .. }) | Some(Origin::Environment { .. })
    ) && (def.get)(service.base())
        != (def.get)(&infra::config::Config::default())
        && (def.get)(service.base()) != stored_value;
    let config_value = saved_supersedes.then(|| (def.get)(service.base()));

    // An enum's choices are named in the locale rather than shown as the wire
    // value: `starttls` and `lazy` are the server's vocabulary, not the operator's.
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
    // The environment's variable is the only provenance a row prints as text.
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

    // One state, by precedence: a value waiting for the next start, then a change
    // that needs a start, then a row that cannot be edited here.
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

    // `tr` borrows the key it is given, so the strings are owned first.
    let label = t.tr(&def.label_key()).to_string();
    let help = t.tr(&def.help_key()).to_string();
    // The variable name joins the haystack, so `NANOFILE_` finds the row too.
    let env_name = def.env.map(str::to_string);
    let search_text = match &env_name {
        Some(var) => format!("{} {label} {help} {var}", def.key),
        None => format!("{} {label} {help}", def.key),
    };
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
        env_name,
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
/// field win: a checkbox submits its hidden `false` before the `true`, so an
/// unchecked box means `false` rather than "absent".
fn fold_form(pairs: &[(String, String)]) -> HashMap<String, String> {
    pairs.iter().cloned().collect()
}

/// Build the service's form from the submitted body.
///
/// A secret is three-way: filled in = replace, the clear box = erase, and an
/// empty rendered field = leave what is stored alone.
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
        // value (a bad port, an invalid sender) has to be visible on its form.
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

/// POST /sysadmin/settings/reset/ — drop the saved override for one key.
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
/// The signal is raised *after* this handler has returned, so the run loop tears
/// the server down through the normal graceful shutdown and an in-flight response
/// is always delivered. The submitted values are deliberately ignored: restarting
/// is not a save.
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

    /// The media row reads `fork` differently: the fork is how the helper is
    /// started, so it gets its own sentence; every other note keeps the shared one.
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

    /// The notes belong to the item they weaken: each is shown once, under it.
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

    /// Every note key the report can produce has a label.
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

    /// Each band's state answers its own question: a profile that runs, one the
    /// switch stopped, one whose feature switch is off, one the host cannot run,
    /// and one that failed at its own job.
    #[test]
    fn a_band_state_says_whether_that_profile_runs_and_why_not() {
        let t = I18n::get(Some("en"));
        let every_item = Protections {
            limits: true,
            files: true,
            network: true,
            process: true,
        };
        let inputs = |enabled: bool, index_enabled: bool| SandboxInputs {
            enabled,
            index_enabled,
        };
        let documents = |answer: &worker::PageAnswer, inputs: &SandboxInputs| {
            documents_profile(t, answer, inputs).state
        };
        let media =
            |answer: &worker::PageAnswer, enabled: bool| media_profile(t, answer, enabled).state;

        let running = documents(&measured(every_item, true, None), &inputs(true, true));
        assert_eq!(running.label, t.tr("sandbox.state_running"));
        assert_eq!(running.class, "badge-green");
        assert!(running.reason.is_none() && running.action.is_none());

        // The switch is its own reason, not repeated under the band: the switch's
        // own row carries the cost.
        let off = documents(&measured(every_item, true, None), &inputs(false, true));
        assert_eq!(off.label, t.tr("sandbox.state_disabled"));
        assert_eq!(off.class, "badge-gray");
        assert!(off.reason.is_none());

        // The feature this profile serves has a switch of its own, on another
        // page: the band says so and points at it.
        let no_index = documents(&measured(every_item, true, None), &inputs(true, false));
        assert_eq!(no_index.label, t.tr("sandbox.state_index_off"));
        assert_eq!(
            no_index.action.as_ref().map(|action| action.href),
            Some("/sysadmin/settings/maintenance/")
        );

        // A host that cannot establish the one required item: the reason is the
        // refusal, not a sentence of the page's own.
        let unbounded = documents(
            &measured(
                Protections::default(),
                false,
                Some("this host could not bound the worker's memory and CPU"),
            ),
            &inputs(true, true),
        );
        assert_eq!(unbounded.label, t.tr("sandbox.state_unavailable"));
        assert_eq!(unbounded.class, "badge-red");
        assert_eq!(
            unbounded.reason.as_deref(),
            Some("this host could not bound the worker's memory and CPU")
        );

        // A probe that could not run at all says so rather than blaming an item.
        let unavailable = documents(
            &worker::PageAnswer::Unavailable("no worker binary".to_string()),
            &inputs(true, true),
        );
        assert_eq!(unavailable.label, t.tr("sandbox.state_unavailable"));
        assert_eq!(unavailable.reason.as_deref(), Some("no worker binary"));

        // The media profile has a job of its own: a helper that cannot decode is
        // neither "running" nor "the host cannot bound a child".
        let failed = media(
            &media_report(
                every_item,
                Some(crate::sandbox::JobVerdict::Failed),
                false,
                Some("the media profile cannot decode a frame"),
            ),
            true,
        );
        assert_eq!(failed.label, t.tr("sandbox.state_media_failed"));
        assert_eq!(failed.class, "badge-red");
        assert_eq!(
            failed.reason.as_deref(),
            Some("the media profile cannot decode a frame")
        );

        let no_helper = media(
            &media_report(
                every_item,
                Some(crate::sandbox::JobVerdict::NoHelper),
                true,
                None,
            ),
            true,
        );
        assert_eq!(no_helper.label, t.tr("sandbox.state_no_helper"));
        assert_eq!(no_helper.class, "badge-gray");
        assert_eq!(
            no_helper.action.as_ref().map(|action| action.href),
            Some("/sysadmin/settings/storage/")
        );

        let media_off = media(&media_report(every_item, None, true, None), false);
        assert_eq!(media_off.label, t.tr("sandbox.state_disabled"));
    }

    /// A report whose items are `protections`, for the state and item tests.
    fn measured(
        protections: Protections,
        host_runs: bool,
        host_reason: Option<&str>,
    ) -> worker::PageAnswer {
        worker::PageAnswer::Measured {
            report: crate::sandbox::Report {
                profile: Profile::Documents,
                protections,
                job: None,
                detail: "landlock_abi=6".to_string(),
            },
            host_runs,
            host_reason: host_reason.map(|why| why.to_string()),
        }
    }

    /// The same, for the media profile, whose job verdict is part of its state.
    fn media_report(
        protections: Protections,
        job: Option<crate::sandbox::JobVerdict>,
        host_runs: bool,
        host_reason: Option<&str>,
    ) -> worker::PageAnswer {
        worker::PageAnswer::Measured {
            report: crate::sandbox::Report {
                profile: Profile::Media,
                protections,
                job,
                detail: "parse=media-ok(1)".to_string(),
            },
            host_runs,
            host_reason: host_reason.map(|why| why.to_string()),
        }
    }

    /// The item rows say which task is a requirement, how much a missing one
    /// matters, and what does not apply at all.
    #[test]
    fn an_item_row_marks_the_required_one_and_explains_only_what_is_missing() {
        assert_eq!(Protections::REQUIRED, "limits");
        assert!(Protections::required("limits"));
        for item in ["files", "network", "process"] {
            assert!(!Protections::required(item), "{item} is best-effort");
        }

        assert_eq!(item_class("limits", true, true), "badge-green");
        assert_eq!(item_class("limits", false, true), "badge-red");
        assert_eq!(item_class("files", false, true), "badge-red");
        assert_eq!(item_class("network", false, true), "badge-gray");
        assert_eq!(item_class("process", false, true), "badge-gray");

        // The media profile starts the helper by definition, so its process item
        // is not a gap.
        assert!(item_applicable(Profile::Documents, "process"));
        assert!(item_applicable(Profile::Images, "process"));
        assert!(!item_applicable(Profile::Media, "process"));
        assert!(item_applicable(Profile::Media, "files"));
        assert_eq!(item_class("process", false, false), "badge-gray");

        assert_eq!(item_severity("limits"), "sandbox.severity_critical");
        assert_eq!(item_severity("files"), "sandbox.severity_critical");
        assert_eq!(item_severity("network"), "sandbox.severity_important");
        assert_eq!(item_severity("process"), "sandbox.severity_advisory");
    }

    /// A band's four rows exist exactly when the host answered for it.
    ///
    /// A profile the host refuses is still a *measured* band — the items are how
    /// an admin tells "cannot confine the profile" from "cannot decode".
    #[test]
    fn a_band_has_four_rows_exactly_when_the_host_answered_for_it() {
        let t = I18n::get(Some("en"));
        let every_item = Protections {
            limits: true,
            files: true,
            network: true,
            process: true,
        };

        // Measured, and not running: the rows are the answer, so they stay.
        let refused = media_report(
            every_item,
            Some(crate::sandbox::JobVerdict::Failed),
            false,
            Some("the media profile cannot decode a frame"),
        );
        let band = media_profile(t, &refused, true);
        assert!(band.measured);
        assert_eq!(band.items.len(), 4);
        assert_eq!(
            band.items.iter().map(|item| item.key).collect::<Vec<_>>(),
            ["limits", "files", "network", "process"]
        );

        // Never answered: a state with a reason, and no rows to misread.
        let unmeasured = documents_profile(
            t,
            &worker::PageAnswer::Unavailable("no worker binary".to_string()),
            &SandboxInputs {
                enabled: true,
                index_enabled: true,
            },
        );
        assert!(!unmeasured.measured);
        assert!(unmeasured.items.is_empty());

        // The switch does not change either half: a switched-off profile was still
        // measured, with the switch out of the picture.
        let off = documents_profile(
            t,
            &measured(every_item, true, None),
            &SandboxInputs {
                enabled: false,
                index_enabled: true,
            },
        );
        assert!(off.measured);
        assert_eq!(off.items.len(), 4);
    }

    #[test]
    fn an_unchecked_box_submits_the_hidden_false() {
        // An unchecked box sends only its hidden field; last-wins makes both alike.
        let off = fold_form(&[
            (
                "server.share_download_enabled".to_string(),
                "false".to_string(),
            ),
            ("csrf_token".to_string(), "x".to_string()),
        ]);
        assert_eq!(off.get("server.share_download_enabled").unwrap(), "false");

        let on = fold_form(&[
            (
                "server.share_download_enabled".to_string(),
                "false".to_string(),
            ),
            (
                "server.share_download_enabled".to_string(),
                "true".to_string(),
            ),
        ]);
        assert_eq!(on.get("server.share_download_enabled").unwrap(), "true");
    }

    #[test]
    fn a_secret_is_three_way_and_never_read_back() {
        let parsed = parse_form(
            Section::Email,
            &form(&[("secret:email.password", ""), ("csrf_token", "x")]),
        )
        .unwrap();
        assert_eq!(parsed.secrets.get("email.password"), Some(&None));

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
                ("server.share_download_enabled", "false"),
            ]),
        )
        .unwrap();
        assert_eq!(parsed.values.get("server.port").unwrap(), "8082");
        assert!(
            !parsed.values.contains_key("server.share_download_enabled"),
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
            env_name: None,
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

    /// A key with no group still has to reach the page; dropping it is worse.
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
