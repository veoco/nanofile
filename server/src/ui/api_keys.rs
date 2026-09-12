//! Web UI for managing unified API keys.
//!
//! Server-rendered: every capability is its own checkbox and every library its
//! own permission selector, so the page works without JavaScript. The only
//! enhancement (see `frontend/pages/api-keys.js`) is copying the freshly minted
//! secret and pre-ticking a preset.

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

/// Query parameters of the key page.
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

/// Secrets minted but not yet shown.
///
/// The create form redirects after a successful POST so that a browser refresh
/// re-reads the list instead of re-running the creation. The plaintext therefore
/// has to survive exactly one redirect: it waits here, is handed out once, and
/// expires after [`REVEAL_TTL`] if the browser never follows the redirect.
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

/// One capability row in the picker.
pub struct CapabilityView {
    pub id: &'static str,
    pub write: bool,
}

/// Capabilities grouped under a translated domain heading.
pub struct DomainGroup {
    pub label: String,
    pub capabilities: Vec<CapabilityView>,
}

/// A selectable starting point with its translated label.
pub struct PresetView {
    pub id: &'static str,
    pub label: String,
    pub capabilities: Vec<&'static str>,
}

/// A selectable lifetime preset value.
pub struct TtlOption {
    pub days: u64,
    pub label: String,
}

/// One library row in the scope picker.
pub struct RepoOption {
    pub id: String,
    pub name: String,
}

/// A key as rendered on the page.
pub struct ApiKeyCard {
    pub id: i32,
    pub name: String,
    pub key_prefix: String,
    pub capabilities: Vec<String>,
    pub all_repos: bool,
    /// `(repo_id, repo_name, permission)` for each bound library.
    pub repos: Vec<(String, String, String)>,
    pub created_at: String,
    pub expires_at: String,
    pub last_used_at: String,
    pub read_only: bool,
    /// Whether the key expires at all (drives the edit form's default).
    pub has_expiry: bool,
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
    let cards = keys
        .into_iter()
        .map(|key| card_of(key, &repo_names, t))
        .collect();

    let left_panel_repos = state
        .left_panel_cache
        .get_for_user(&state.repos, user.user_id)
        .await?;

    let tpl = ApiKeysTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        active_page: "settings",
        left_panel_repos,
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
                capabilities: preset.capabilities,
            })
            .collect(),
        ttl_options: state
            .config
            .auth
            .api_key_ttl_presets_days
            .iter()
            .map(|days| TtlOption {
                days: *days,
                label: t.trf("apikey.expiry_days", &[("days", days.to_string())]),
            })
            .collect(),
        max_ttl_days: state.config.auth.api_key_max_ttl_days,
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

fn card_of(key: ApiKeyView, repo_names: &HashMap<String, String>, t: &I18n) -> ApiKeyCard {
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
    let never = t.tr("common.never").to_string();
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
        created_at: super::format_ts(key.created_at),
        expires_at: key
            .expires_at
            .map(super::format_ts)
            .unwrap_or(never.clone()),
        last_used_at: key.last_used_at.map(super::format_ts).unwrap_or(never),
        read_only,
        has_expiry,
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

fn expiry_from(form: &HashMap<String, String>) -> Result<KeyExpiry, AppError> {
    match form.get("expiry").map(String::as_str).unwrap_or("never") {
        "never" => Ok(KeyExpiry::Never),
        "custom" => {
            let days: u64 = form
                .get("expiry_days")
                .map(|value| value.trim())
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| {
                    AppError::BadRequest("a custom lifetime needs a number of days".into())
                })?;
            Ok(KeyExpiry::InDays(days))
        }
        preset => preset
            .parse::<u64>()
            .map(KeyExpiry::InDays)
            .map_err(|_| AppError::BadRequest("invalid lifetime".into())),
    }
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
    // `?created=<id>` reveals the secret once; a second visit (or a reload of
    // this URL) finds nothing left to show.
    let new_key = query
        .created
        .and_then(|key_id| take_reveal(user.user_id, key_id));
    render(&state, &user, new_key, None, None).await
}

/// POST /settings/api-keys/create/
///
/// Answers with the re-rendered page so the plaintext secret can be shown
/// exactly once; a redirect would lose it.
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

    let expires = match expiry_from(&form) {
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

    match ApiKeyService::create(&state.repos, &state.config.auth, user.user_id, input).await {
        Ok(created) => {
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

    let expires = match expiry_from(&form) {
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
        &state.config.auth,
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
