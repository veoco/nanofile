//! Web UI for managing unified API keys.
//!
//! Server-rendered so the page works without JavaScript; `frontend/pages/api-keys.js`
//! only copies the freshly minted secret and pre-ticks a preset.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;
use crate::domain::capability::{Capability, CapabilityDomain};
use crate::i18n::I18n;
use crate::service::api_key::{
    ApiKeyService, ApiKeyUpdate, ApiKeyView, KeyExpiry, NewApiKey, catalog,
};
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Deserialize, Default)]
pub struct ApiKeysQuery {
    /// Set by the redirect that follows a successful creation.
    pub created: Option<i32>,
}

/// How long a freshly minted secret stays available for its one-time reveal.
const REVEAL_TTL: Duration = Duration::from_secs(600);

struct PendingReveal {
    user_id: i32,
    name: String,
    secret: String,
    created: Instant,
}

/// Secrets minted but not yet shown: they survive exactly the one redirect that
/// follows creation, are handed out once, and expire after [`REVEAL_TTL`] if the
/// browser never follows it.
static PENDING_REVEALS: LazyLock<Mutex<HashMap<i32, PendingReveal>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn stash_reveal(user_id: i32, key_id: i32, name: String, secret: String) {
    let mut pending = PENDING_REVEALS.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    pending.retain(|_, entry| now.duration_since(entry.created) < REVEAL_TTL);
    pending.insert(
        key_id,
        PendingReveal {
            user_id,
            name,
            secret,
            created: now,
        },
    );
}

/// Take the pending reveal for `key_id`, if it belongs to `user_id` and is fresh.
fn take_reveal(user_id: i32, key_id: i32) -> Option<NewKeyView> {
    let mut pending = PENDING_REVEALS.lock().unwrap_or_else(|e| e.into_inner());
    let entry = pending.remove(&key_id)?;
    if entry.user_id != user_id || entry.created.elapsed() >= REVEAL_TTL {
        return None;
    }
    Some(NewKeyView {
        name: entry.name,
        secret: entry.secret,
    })
}

pub struct CapabilityView {
    pub id: &'static str,
    pub write: bool,
    /// The enforcement points that consult this capability, pre-joined for the
    /// label's tooltip. Language-neutral (methods and paths), so no i18n key.
    pub targets: String,
}

pub struct DomainGroup {
    pub label: String,
    pub capabilities: Vec<CapabilityView>,
}

pub struct PresetView {
    pub id: &'static str,
    pub label: String,
    pub capabilities: Vec<&'static str>,
    /// How many capabilities the preset expands to, so an over-broad preset is visible.
    pub count: usize,
}

pub struct TtlOption {
    pub days: u64,
    pub label: String,
}

pub struct RepoOption {
    pub id: String,
    pub name: String,
}

pub struct ApiKeyCard {
    pub id: i32,
    pub name: String,
    pub key_prefix: String,
    pub capabilities: Vec<String>,
    pub all_repos: bool,
    /// `(repo_id, repo_name, permission)` for each bound library.
    pub repos: Vec<(String, String, String)>,
    /// Unix seconds; the page renders them in the viewer's timezone.
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub last_used_at_ts: Option<i64>,
    pub read_only: bool,
    /// Which `<option>` value the edit form marks selected.
    pub expiry_option: String,
    /// Whole days left, used to prefill the custom lifetime input.
    pub expiry_days: u64,
}

impl ApiKeyCard {
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    pub fn permission_for(&self, repo_id: &str) -> String {
        self.repos
            .iter()
            .find(|(id, _, _)| id == repo_id)
            .map(|(_, _, permission)| permission.clone())
            .unwrap_or_default()
    }
}

pub struct NewKeyView {
    pub name: String,
    pub secret: String,
}

#[derive(Template)]
#[template(path = "settings/api_keys.html")]
pub struct ApiKeysTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub sharing_enabled: bool,
    pub current_repo_id: Option<String>,
    pub csrf_token: String,
    pub domains: Vec<DomainGroup>,
    pub presets: Vec<PresetView>,
    pub ttl_options: Vec<TtlOption>,
    pub max_ttl_days: u64,
    pub repo_options: Vec<RepoOption>,
    pub keys: Vec<ApiKeyCard>,
    pub new_key: Option<NewKeyView>,
    pub error: Option<String>,
    pub success: Option<String>,
    pub nav: crate::ui::settings::SettingsNav,
}

/// Render the page, optionally with a freshly created (shown-once) secret.
async fn render(
    state: &AppState,
    user: &WebUser,
    new_key: Option<NewKeyView>,
    error: Option<String>,
    success: Option<String>,
) -> Result<Html<String>, AppError> {
    let user_record = state
        .repos
        .user
        .find_by_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
    let t = I18n::get(user.language.as_deref());

    let (entries, presets) = catalog(user_record.is_admin);
    let mut domains: Vec<DomainGroup> = Vec::new();
    for domain in CapabilityDomain::ALL {
        let capabilities: Vec<CapabilityView> = entries
            .iter()
            .filter(|entry| entry.domain == domain.id())
            .map(|entry| CapabilityView {
                id: entry.id,
                write: entry.write,
                targets: entry.enforced_by.join(", "),
            })
            .collect();
        if !capabilities.is_empty() {
            domains.push(DomainGroup {
                label: t.tr(&format!("apikey.domain.{}", domain.id())).to_string(),
                capabilities,
            });
        }
    }

    let repo_infos = crate::service::repo::service::RepoService::list_repos(
        &state.repos,
        user.user_id,
        &user.email,
    )
    .await?;
    let repo_options: Vec<RepoOption> = repo_infos
        .iter()
        .map(|repo| RepoOption {
            id: repo.id.clone(),
            name: repo.name.clone(),
        })
        .collect();
    let repo_names: HashMap<String, String> = repo_infos
        .iter()
        .map(|repo| (repo.id.clone(), repo.name.clone()))
        .collect();

    let keys = ApiKeyService::list(&state.repos, user.user_id).await?;
    let max_ttl_days = state.config().auth.api_key_max_ttl_days;
    let ttl_presets_days = state.config().auth.api_key_ttl_presets_days.clone();
    let cards = keys
        .into_iter()
        .map(|key| card_of(key, &repo_names, &ttl_presets_days, max_ttl_days))
        .collect();

    let left_panel_repos = state
        .left_panel_cache
        .get_for_user(&state.repos, user.user_id)
        .await?;
    let nav =
        crate::ui::settings::build_nav(state, user, crate::ui::settings::SettingsNav::API_KEYS)
            .await?;

    let tpl = ApiKeysTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        active_page: crate::ui::settings::SettingsNav::API_KEYS,
        nav,
        left_panel_repos,
        sharing_enabled: crate::ui::ctx::sharing_enabled(state),
        current_repo_id: None,
        csrf_token: crate::service::auth::csrf::generate_csrf_token(
            &state.csrf_secret,
            &user.session_token,
        ),
        domains,
        presets: presets
            .into_iter()
            .map(|preset| PresetView {
                id: preset.id,
                label: t.tr(&format!("apikey.preset.{}", preset.id)).to_string(),
                count: preset.capabilities.len(),
                capabilities: preset.capabilities,
            })
            .collect(),
        ttl_options: state
            .config()
            .auth
            .api_key_ttl_presets_days
            .iter()
            .map(|days| TtlOption {
                days: *days,
                label: t.trf("apikey.expiry_days", &[("days", days.to_string())]),
            })
            .collect(),
        max_ttl_days: state.config().auth.api_key_max_ttl_days,
        repo_options,
        keys: cards,
        new_key,
        error,
        success,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Which `<option>` value the edit form should mark selected.
///
/// `never` is a choice only while the server allows keys without an expiry;
/// otherwise an expiry-less key falls back to `custom`, asking for a lifetime
/// rather than quietly granting one.
fn expiry_option(has_expiry: bool, expiry_days: u64, presets: &[u64], max_ttl_days: u64) -> String {
    if !has_expiry {
        return if max_ttl_days == 0 {
            "never".to_string()
        } else {
            "custom".to_string()
        };
    }
    if presets.contains(&expiry_days) {
        expiry_days.to_string()
    } else {
        "custom".to_string()
    }
}

fn card_of(
    key: ApiKeyView,
    repo_names: &HashMap<String, String>,
    ttl_presets_days: &[u64],
    max_ttl_days: u64,
) -> ApiKeyCard {
    let repos = key
        .repos
        .iter()
        .map(|(repo_id, permission)| {
            let name = repo_names
                .get(repo_id)
                .cloned()
                .unwrap_or_else(|| repo_id.clone());
            (repo_id.clone(), name, permission.clone())
        })
        .collect();
    // A key is "read-only" when none of its capabilities can change state; the
    // badge is presentation only, enforcement is per route.
    let read_only = key
        .capabilities
        .iter()
        .all(|id| Capability::from_id(id).is_some_and(|c| !c.is_write()));
    let has_expiry = key.expires_at.is_some();
    let expiry_days = key.expires_at.map_or(0, |ts| {
        let remaining = (ts - chrono::Utc::now().timestamp()).max(0) as u64;
        remaining.div_ceil(86_400)
    });
    ApiKeyCard {
        id: key.id,
        name: key.name,
        key_prefix: key.key_prefix.unwrap_or_else(|| "—".to_string()),
        capabilities: key.capabilities,
        all_repos: key.all_repos,
        repos,
        created_at_ts: key.created_at,
        expires_at_ts: key.expires_at,
        last_used_at_ts: key.last_used_at,
        read_only,
        expiry_option: expiry_option(has_expiry, expiry_days, ttl_presets_days, max_ttl_days),
        expiry_days,
    }
}

/// Collect the capability checkboxes (`cap__<id>`).
fn capabilities_from(form: &HashMap<String, String>) -> Vec<String> {
    form.keys()
        .filter_map(|field| field.strip_prefix("cap__"))
        .map(str::to_string)
        .collect()
}

/// Collect the per-library permission selectors (`repo__<repo_id>`).
fn repo_permissions_from(form: &HashMap<String, String>) -> Vec<(String, String)> {
    form.iter()
        .filter_map(|(field, value)| {
            field
                .strip_prefix("repo__")
                .map(|repo_id| (repo_id.to_string(), value.clone()))
        })
        .filter(|(_, permission)| permission == "r" || permission == "rw")
        .collect()
}

fn all_repos_from(form: &HashMap<String, String>) -> bool {
    form.get("all_repos")
        .is_some_and(|value| value == "1" || value == "on" || value == "true")
}

/// Read the requested lifetime, rejecting what the service would reject, in the
/// reader's language; the JSON API keeps its English messages.
fn expiry_from(
    form: &HashMap<String, String>,
    t: &I18n,
    max_ttl_days: u64,
) -> Result<KeyExpiry, AppError> {
    let requested = form.get("expiry").map(String::as_str).unwrap_or("never");
    if requested == "never" {
        if max_ttl_days > 0 {
            return Err(AppError::BadRequest(t.trf(
                "apikey.err_expiry_never",
                &[("days", max_ttl_days.to_string())],
            )));
        }
        return Ok(KeyExpiry::Never);
    }
    let days = if requested == "custom" {
        form.get("expiry_days")
            .map(|value| value.trim())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|days| *days > 0)
            .ok_or_else(|| AppError::BadRequest(t.tr("apikey.err_expiry_days").to_string()))?
    } else {
        requested
            .parse::<u64>()
            .ok()
            .filter(|days| *days > 0)
            .ok_or_else(|| AppError::BadRequest(t.tr("apikey.err_expiry_invalid").to_string()))?
    };
    if max_ttl_days > 0 && days > max_ttl_days {
        return Err(AppError::BadRequest(t.trf(
            "apikey.err_expiry_days_max",
            &[("days", max_ttl_days.to_string())],
        )));
    }
    Ok(KeyExpiry::InDays(days))
}

fn message_of(error: &AppError, t: &I18n) -> String {
    match error {
        AppError::BadRequest(message) => message.clone(),
        AppError::Forbidden => t.tr("apikey.not_allowed").to_string(),
        AppError::NotFound(message) => message.clone(),
        _ => t.tr("apikey.create_failed").to_string(),
    }
}

/// Render an error page (HTTP 200, like the other settings forms).
async fn render_error(
    state: &AppState,
    user: &WebUser,
    error: AppError,
) -> Result<Response, AppError> {
    let t = I18n::get(user.language.as_deref());
    let message = message_of(&error, t);
    Ok(render(state, user, None, Some(message), None)
        .await?
        .into_response())
}

/// GET /settings/api-keys/
pub async fn list_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ApiKeysQuery>,
) -> Result<Html<String>, AppError> {
    // `?created=<id>` reveals the secret once; a second visit finds nothing.
    let new_key = query
        .created
        .and_then(|key_id| take_reveal(user.user_id, key_id));
    render(&state, &user, new_key, None, None).await
}

/// POST /settings/api-keys/create/
///
/// Answers with the re-rendered page so the plaintext secret can be shown once.
pub async fn create(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(String::as_str),
    )?;

    let t = I18n::get(user.language.as_deref());
    let expires = match expiry_from(&form, t, state.config().auth.api_key_max_ttl_days) {
        Ok(expires) => expires,
        Err(error) => return render_error(&state, &user, error).await,
    };
    let input = NewApiKey {
        name: form.get("name").cloned(),
        capabilities: capabilities_from(&form),
        all_repos: all_repos_from(&form),
        repo_permissions: repo_permissions_from(&form),
        expires,
    };

    match ApiKeyService::create(&state.repos, &state.config().auth, user.user_id, input).await {
        Ok(created) => {
            // The secret is only ever shown here, so this mail records that the key was created.
            state
                .mail
                .notify_api_key_created(user.user_id, &created.view.name)
                .await;
            stash_reveal(
                user.user_id,
                created.view.id,
                created.view.name.clone(),
                created.secret,
            );
            // POST/Redirect/GET: refreshing the page must not mint another key.
            Ok(
                Redirect::to(&format!("/settings/api-keys/?created={}", created.view.id))
                    .into_response(),
            )
        }
        Err(error) => render_error(&state, &user, error).await,
    }
}

/// POST /settings/api-keys/{id}/update/
pub async fn update(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(key_id): Path<i32>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(String::as_str),
    )?;

    let t = I18n::get(user.language.as_deref());
    let expires = match expiry_from(&form, t, state.config().auth.api_key_max_ttl_days) {
        Ok(expires) => expires,
        Err(error) => return render_error(&state, &user, error).await,
    };
    let input = ApiKeyUpdate {
        name: form.get("name").cloned(),
        capabilities: Some(capabilities_from(&form)),
        all_repos: Some(all_repos_from(&form)),
        repo_permissions: Some(repo_permissions_from(&form)),
        expires: Some(expires),
    };

    match ApiKeyService::update(
        &state.repos,
        &state.config().auth,
        user.user_id,
        key_id,
        input,
    )
    .await
    {
        Ok(_) => Ok((StatusCode::FOUND, [("Location", "/settings/api-keys/")]).into_response()),
        Err(error) => render_error(&state, &user, error).await,
    }
}

/// POST /settings/api-keys/{id}/revoke/
pub async fn revoke(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(key_id): Path<i32>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(String::as_str),
    )?;

    match ApiKeyService::revoke(&state.repos, user.user_id, key_id).await {
        Ok(()) => Ok((StatusCode::FOUND, [("Location", "/settings/api-keys/")]).into_response()),
        Err(error) => render_error(&state, &user, error).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESETS: &[u64] = &[7, 30, 90, 180, 365];

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn an_unlimited_server_selects_never_for_a_key_without_an_expiry() {
        assert_eq!(expiry_option(false, 0, PRESETS, 0), "never");
    }

    /// The create form hides `never` on a bounded server, so the edit form must too.
    #[test]
    fn a_bounded_server_offers_neither_never_nor_a_silent_lifetime() {
        assert_eq!(expiry_option(false, 0, PRESETS, 30), "custom");
    }

    #[test]
    fn a_remaining_lifetime_selects_its_own_preset() {
        assert_eq!(expiry_option(true, 30, PRESETS, 0), "30");
        assert_eq!(expiry_option(true, 15, PRESETS, 0), "custom");
    }

    #[test]
    fn a_key_page_rejection_reads_in_the_readers_language() {
        let zh = I18n::get(Some("zh"));
        let en = I18n::get(None);

        let error = expiry_from(&form(&[("expiry", "never")]), zh, 30).unwrap_err();
        assert_eq!(
            message_of(&error, zh),
            "本服务器要求密钥必须有有效期，最长 30 天。"
        );
        let error = expiry_from(&form(&[("expiry", "never")]), en, 30).unwrap_err();
        assert_eq!(
            message_of(&error, en),
            "This server requires an expiry; the longest is 30 days."
        );

        let error =
            expiry_from(&form(&[("expiry", "custom"), ("expiry_days", "")]), zh, 0).unwrap_err();
        assert_eq!(message_of(&error, zh), "请填写天数。");

        let error = expiry_from(&form(&[("expiry", "365")]), zh, 30).unwrap_err();
        assert_eq!(message_of(&error, zh), "最多 30 天。");
    }

    #[test]
    fn a_lifetime_the_server_allows_is_accepted() {
        assert_eq!(
            expiry_from(&form(&[("expiry", "30")]), I18n::get(None), 30).unwrap(),
            KeyExpiry::InDays(30)
        );
        assert_eq!(
            expiry_from(&form(&[("expiry", "never")]), I18n::get(None), 0).unwrap(),
            KeyExpiry::Never
        );
    }
}
