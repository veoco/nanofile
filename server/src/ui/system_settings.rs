//! Admin Web UI — system management: every setting that can be managed at
//! runtime, on one page per area.
//!
//! The page is *data-driven*: it renders the catalog (`infra::settings::CATALOG`)
//! rather than naming fields, so a setting cannot exist without appearing here,
//! and each row shows where its effective value came from — the environment, the
//! config file, the database, or the built-in default — plus whether a change
//! needs a restart.
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
    /// The badge's detail line (variable name, file key, or "saved …").
    /// For an environment-owned row this is the variable itself, and it is the
    /// only place the name is rendered.
    pub origin_detail: String,
    /// Unix seconds of the last save, for a database-sourced value.
    pub origin_at: Option<i64>,
    /// The value in the config file, when a saved value is superseding it —
    /// otherwise an operator editing the file sees no effect and no reason why.
    pub config_value: Option<String>,
    /// The server starts with this value locked in; a change needs a restart.
    pub restart: bool,
    /// A saved value that is waiting for the next start.
    pub pending_restart: bool,
    /// The value is read before the server loop exists (the log subscriber, the
    /// desktop tray), so only a full process restart applies it.
    pub process_restart: bool,
    /// Only the environment or the config file can set this.
    pub locked: bool,
    /// Read-only settings are shown for their origin, not for editing.
    pub read_only: bool,
    /// A secret is configured (the field renders a placeholder, not a value).
    pub secret_set: bool,
    /// A stored secret cannot be decrypted any more.
    pub secret_broken: bool,
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
    /// restart button below does cover.
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

    let options = match def.kind {
        Kind::Enum(options) => options
            .iter()
            .map(|option| SelectOption {
                value: (*option).to_string(),
                label: if option.is_empty() {
                    t.tr("setting.value_unset").to_string()
                } else {
                    (*option).to_string()
                },
                selected: value == *option,
            })
            .collect(),
        _ => Vec::new(),
    };

    let secret_state = service.secret_state(def.key);
    let (origin_id, origin_detail, origin_at) = match origin {
        Some(Origin::Environment { var }) => ("environment", var.to_string(), None),
        Some(Origin::ConfigFile { key }) => ("config", format!("config.toml: {key}"), None),
        Some(Origin::Database {
            updated_at,
            updated_by,
        }) => (
            "database",
            match updated_by {
                Some(id) => t.trf("setting.saved_by", &[("id", id.to_string())]),
                None => String::new(),
            },
            (updated_at > 0).then_some(updated_at),
        ),
        Some(Origin::Default) => ("default", String::new(), None),
        None => ("default", String::new(), None),
    };

    let read_only = !def.is_stored();
    let locked = read_only || matches!(origin, Some(Origin::Environment { .. }));

    SettingRow {
        key: def.key.to_string(),
        label_key: def.label_key(),
        help: t.tr(&def.help_key()).to_string(),
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
        origin_detail,
        origin_at,
        config_value,
        restart: def.apply.is_in_place_restart(),
        pending_restart: pending.contains(def.key),
        process_restart: def.apply == Apply::ProcessRestart,
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
            label_key: String::new(),
            help: String::new(),
            unit: None,
            control: "text",
            value: String::new(),
            checked: false,
            options: Vec::new(),
            field: String::new(),
            origin: "default",
            origin_detail: String::new(),
            origin_at: None,
            config_value: None,
            restart: false,
            pending_restart: false,
            process_restart: false,
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
