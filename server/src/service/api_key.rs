//! Management of unified API keys.
//!
//! A key is described by capabilities (what it may do), library bindings (where
//! it may do it, with a per-library read/write ceiling) and an expiry. The
//! service owns every validation rule so the JSON API and the server-rendered
//! settings page cannot drift apart.

use std::collections::HashSet;

use base::error::AppError;
use infra::config::AuthConfig;
use infra::entity::api_key;

use crate::domain::capability::{Capability, CapabilitySet, PRESETS, Preset};
use crate::repository::Repositories;
use crate::repository::api_key::{ApiKeyBinding, CreateApiKeyParams, UpdateApiKeyParams};
use crate::service::auth::token::generate_api_token;

/// Longest accepted key name.
const MAX_NAME_LEN: usize = 255;
/// Number of leading secret characters kept for display.
const PREFIX_LEN: usize = 8;
const SECONDS_PER_DAY: i64 = 86_400;

/// How long a new key should live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyExpiry {
    Never,
    InDays(u64),
    At(i64),
}

/// Everything needed to mint a key.
pub struct NewApiKey {
    pub name: Option<String>,
    pub capabilities: Vec<String>,
    pub all_repos: bool,
    pub repo_permissions: Vec<(String, String)>,
    pub expires: KeyExpiry,
}

/// A partial update. `None` leaves the corresponding property alone.
pub struct ApiKeyUpdate {
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub all_repos: Option<bool>,
    pub repo_permissions: Option<Vec<(String, String)>>,
    pub expires: Option<KeyExpiry>,
}

/// A key as exposed to its owner. The secret is never part of this.
#[derive(Clone, Debug)]
pub struct ApiKeyView {
    pub id: i32,
    pub name: String,
    pub key_prefix: Option<String>,
    pub capabilities: Vec<String>,
    pub all_repos: bool,
    pub repos: Vec<(String, String)>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

/// A freshly created key: the only time the plaintext is available.
pub struct CreatedApiKey {
    pub view: ApiKeyView,
    pub secret: String,
}

/// One grantable capability, for rendering a picker.
pub struct CatalogEntry {
    pub id: &'static str,
    pub domain: &'static str,
    pub write: bool,
}

/// A named starting point offered by the UI.
pub struct CatalogPreset {
    pub id: &'static str,
    pub capabilities: Vec<&'static str>,
}

pub struct ApiKeyService;

impl ApiKeyService {
    /// Mint a key. The returned secret is shown to the caller exactly once.
    pub async fn create(
        repos: &Repositories,
        config: &AuthConfig,
        user_id: i32,
        input: NewApiKey,
    ) -> Result<CreatedApiKey, AppError> {
        let is_admin = load_user(repos, user_id).await?.is_admin;
        let capabilities = validate_capabilities(&input.capabilities, is_admin)?;
        let bindings = validate_bindings(
            repos,
            user_id,
            input.all_repos,
            &input.repo_permissions,
            capabilities,
        )
        .await?;
        let expires_at = resolve_expiry(config, input.expires)?;
        let repo_view = bindings
            .iter()
            .map(|binding| (binding.repo_id.clone(), binding.permission.clone()))
            .collect();

        let secret = generate_api_token();
        let model = repos
            .api_key
            .create(CreateApiKeyParams {
                user_id,
                name: normalize_name(input.name),
                key_hash: crate::service::auth::token::hash_token(&secret),
                key_prefix: secret[..PREFIX_LEN].to_string(),
                capabilities: capabilities.to_canonical(),
                all_repos: input.all_repos,
                created_at: now(),
                expires_at,
                bindings,
            })
            .await?;

        Ok(CreatedApiKey {
            view: view_of(model, repo_view),
            secret,
        })
    }

    pub async fn list(repos: &Repositories, user_id: i32) -> Result<Vec<ApiKeyView>, AppError> {
        let keys = repos.api_key.find_by_user(user_id).await?;
        let ids: Vec<i32> = keys.iter().map(|key| key.id).collect();
        let bindings = repos.api_key.list_bindings_for_keys(&ids).await?;
        let mut views = Vec::with_capacity(keys.len());
        for key in keys {
            let owned = bindings
                .iter()
                .filter(|binding| binding.key_id == key.id)
                .map(|binding| (binding.repo_id.clone(), binding.permission.clone()))
                .collect();
            views.push(view_of(key, owned));
        }
        Ok(views)
    }

    pub async fn get(
        repos: &Repositories,
        user_id: i32,
        key_id: i32,
    ) -> Result<ApiKeyView, AppError> {
        let key = repos
            .api_key
            .find_by_id_and_user(key_id, user_id)
            .await?
            .ok_or_else(not_found)?;
        let bindings = repos.api_key.list_bindings(key_id).await?;
        let owned = bindings
            .into_iter()
            .map(|binding| (binding.repo_id, binding.permission))
            .collect();
        Ok(view_of(key, owned))
    }

    /// Change a key's metadata. The secret itself is immutable; rotate by
    /// creating a new key and revoking the old one.
    pub async fn update(
        repos: &Repositories,
        config: &AuthConfig,
        user_id: i32,
        key_id: i32,
        input: ApiKeyUpdate,
    ) -> Result<ApiKeyView, AppError> {
        let ApiKeyUpdate {
            name,
            capabilities: requested_capabilities,
            all_repos: requested_all_repos,
            repo_permissions: requested_repos,
            expires,
        } = input;

        let existing = repos
            .api_key
            .find_by_id_and_user(key_id, user_id)
            .await?
            .ok_or_else(not_found)?;
        let is_admin = load_user(repos, user_id).await?.is_admin;

        let capabilities = match &requested_capabilities {
            Some(ids) => validate_capabilities(ids, is_admin)?,
            None => parse_stored_capabilities(&existing)?,
        };
        let final_all_repos = requested_all_repos.unwrap_or(existing.all_repos);
        let current_repos: Vec<(String, String)> = repos
            .api_key
            .list_bindings(key_id)
            .await?
            .into_iter()
            .map(|binding| (binding.repo_id, binding.permission))
            .collect();
        // Widening to every library drops the (now meaningless) bindings, so a
        // key that keeps them does not fail the "all_repos + bindings" check.
        let widening = final_all_repos && requested_repos.is_none();
        let final_repos = requested_repos.unwrap_or(current_repos);

        // Validate the state the key will end up in, so a partial update cannot
        // combine a new capability set with a stale library scope.
        let to_validate = if widening { Vec::new() } else { final_repos };
        let bindings =
            validate_bindings(repos, user_id, final_all_repos, &to_validate, capabilities).await?;

        let expires_at = match expires {
            Some(expiry) => Some(resolve_expiry(config, expiry)?),
            None => None,
        };

        let updated = repos
            .api_key
            .update_metadata(
                key_id,
                user_id,
                UpdateApiKeyParams {
                    name: name.map(|name| normalize_name(Some(name))),
                    capabilities: requested_capabilities
                        .as_ref()
                        .map(|_| capabilities.to_canonical()),
                    all_repos: requested_all_repos,
                    expires_at,
                },
            )
            .await?;
        if !updated {
            return Err(not_found());
        }
        // Bindings are rewritten from the validated final state, which keeps the
        // stored scope and the capability set consistent in one step.
        repos.api_key.replace_bindings(key_id, &bindings).await?;
        Self::get(repos, user_id, key_id).await
    }

    pub async fn revoke(repos: &Repositories, user_id: i32, key_id: i32) -> Result<(), AppError> {
        if repos.api_key.delete_by_id_and_user(key_id, user_id).await? {
            Ok(())
        } else {
            Err(not_found())
        }
    }
}

/// The capability catalog and presets, for rendering a picker.
///
/// Entries a non-admin may not grant are filtered out: `key.*` is never
/// grantable (a key that can mint keys could widen its own access), and
/// `admin.*` only appears for admins.
pub fn catalog(is_admin: bool) -> (Vec<CatalogEntry>, Vec<CatalogPreset>) {
    let entries = Capability::ALL
        .iter()
        .filter(|capability| !capability.is_reserved_to_sessions())
        .filter(|capability| is_admin || !is_admin_capability(**capability))
        .map(|capability| CatalogEntry {
            id: capability.id(),
            domain: capability.domain().id(),
            write: capability.is_write(),
        })
        .collect();
    let presets = PRESETS
        .iter()
        .map(|preset| CatalogPreset {
            id: preset.id,
            capabilities: expanded_ids(preset),
        })
        .collect();
    (entries, presets)
}

fn expanded_ids(preset: &Preset) -> Vec<&'static str> {
    let mut set = CapabilitySet::EMPTY;
    for capability in preset.capabilities {
        set.insert(*capability);
    }
    set.expand();
    set.to_ids()
}

fn is_admin_capability(capability: Capability) -> bool {
    matches!(
        capability,
        Capability::AdminUserRead | Capability::AdminUserWrite
    )
}

async fn load_user(
    repos: &Repositories,
    user_id: i32,
) -> Result<infra::entity::user::Model, AppError> {
    repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))
}

fn not_found() -> AppError {
    AppError::NotFound("API key not found".into())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn normalize_name(name: Option<String>) -> String {
    let trimmed = name.unwrap_or_default();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        "API key".to_string()
    } else {
        trimmed.chars().take(MAX_NAME_LEN).collect()
    }
}

/// Parse and validate a requested capability set.
fn validate_capabilities(ids: &[String], is_admin: bool) -> Result<CapabilitySet, AppError> {
    if ids.is_empty() {
        return Err(AppError::BadRequest(
            "at least one capability is required".into(),
        ));
    }
    let set = CapabilitySet::parse(ids).map_err(|error| AppError::BadRequest(error.to_string()))?;
    for capability in set.iter() {
        if capability.is_reserved_to_sessions() {
            return Err(AppError::BadRequest(format!(
                "{} cannot be granted to an API key",
                capability.id()
            )));
        }
        if is_admin_capability(capability) && !is_admin {
            return Err(AppError::Forbidden);
        }
    }
    Ok(set)
}

/// Parse the capability set already stored on a key.
fn parse_stored_capabilities(key: &api_key::Model) -> Result<CapabilitySet, AppError> {
    CapabilitySet::from_canonical(&key.capabilities)
        .map_err(|error| AppError::Internal(format!("api key {} has {error}", key.id)))
}

/// Validate the library scope, returning the bindings to persist.
async fn validate_bindings(
    repos: &Repositories,
    user_id: i32,
    all_repos: bool,
    repo_permissions: &[(String, String)],
    capabilities: CapabilitySet,
) -> Result<Vec<ApiKeyBinding>, AppError> {
    if all_repos {
        if !repo_permissions.is_empty() {
            return Err(AppError::BadRequest(
                "all_repos cannot be combined with repo_permissions".into(),
            ));
        }
        return Ok(Vec::new());
    }
    if repo_permissions.is_empty() {
        return Err(AppError::BadRequest(
            "a key restricted to specific libraries needs at least one library".into(),
        ));
    }

    let mut seen = HashSet::new();
    let mut bindings = Vec::with_capacity(repo_permissions.len());
    for (repo_id, permission) in repo_permissions {
        if !seen.insert(repo_id.clone()) {
            return Err(AppError::BadRequest(format!("duplicate library {repo_id}")));
        }
        if permission != "r" && permission != "rw" {
            return Err(AppError::BadRequest(format!(
                "permission for {repo_id} must be \"r\" or \"rw\""
            )));
        }
        // Read access is enough to bind: the ceiling limits the key, it does not
        // grant anything, and the effective permission is recomputed on every
        // request against live membership.
        crate::domain::permission::check_repo_read_permission(
            repos.member.as_ref(),
            repo_id,
            user_id,
        )
        .await?;

        // WebDAV has no way to supply an encrypted library's password, so a key
        // carrying WebDAV access must not be bound to one. This mirrors the
        // rejection the legacy per-library WebDAV key endpoint performed.
        if (capabilities.contains(Capability::WebdavRead)
            || capabilities.contains(Capability::WebdavWrite))
            && let Some(repo) = repos.repo.find_by_id(repo_id).await?
            && repo.encrypted != 0
        {
            return Err(AppError::BadRequest(
                "WebDAV is not supported for encrypted libraries".into(),
            ));
        }

        bindings.push(ApiKeyBinding {
            repo_id: repo_id.clone(),
            permission: permission.clone(),
        });
    }
    Ok(bindings)
}

/// Turn the requested lifetime into an absolute expiry.
fn resolve_expiry(config: &AuthConfig, expiry: KeyExpiry) -> Result<Option<i64>, AppError> {
    let now = now();
    let max_days = config.api_key_max_ttl_days;
    match expiry {
        KeyExpiry::Never => {
            if max_days > 0 {
                return Err(AppError::BadRequest(format!(
                    "keys may not be created without an expiry; the maximum lifetime is {max_days} days"
                )));
            }
            Ok(None)
        }
        KeyExpiry::InDays(0) => Err(AppError::BadRequest(
            "expires_in_days must be positive".into(),
        )),
        KeyExpiry::InDays(days) => {
            if max_days > 0 && days > max_days {
                return Err(AppError::BadRequest(format!(
                    "expires_in_days exceeds the maximum of {max_days} days"
                )));
            }
            Ok(Some(now + (days as i64) * SECONDS_PER_DAY))
        }
        KeyExpiry::At(timestamp) => {
            if timestamp <= now {
                return Err(AppError::BadRequest(
                    "expires_at must be in the future".into(),
                ));
            }
            if max_days > 0 && timestamp > now + (max_days as i64) * SECONDS_PER_DAY {
                return Err(AppError::BadRequest(format!(
                    "expires_at exceeds the maximum lifetime of {max_days} days"
                )));
            }
            Ok(Some(timestamp))
        }
    }
}

fn view_of(key: api_key::Model, repos: Vec<(String, String)>) -> ApiKeyView {
    // The stored list is already canonical; splitting it back is lenient on
    // purpose, so a row carrying an id this build does not know still lists
    // instead of failing the whole page.
    let capabilities = key
        .capabilities
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    ApiKeyView {
        id: key.id,
        name: key.name,
        key_prefix: key.key_prefix,
        capabilities,
        all_repos: key.all_repos,
        repos,
        created_at: key.created_at,
        expires_at: key.expires_at,
        last_used_at: key.last_used_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(max_days: u64) -> AuthConfig {
        AuthConfig {
            api_key_max_ttl_days: max_days,
            ..AuthConfig::default()
        }
    }

    #[test]
    fn capability_sets_expand_and_reject_reserved_ids() {
        let set = validate_capabilities(&["file.write".to_string()], false).expect("valid");
        assert!(set.contains(Capability::FileRead));
        assert!(set.contains(Capability::FileWrite));

        for reservation in ["key.read", "key.write"] {
            assert!(
                validate_capabilities(&[reservation.to_string()], false).is_err(),
                "{reservation} must never be grantable"
            );
        }
        assert!(validate_capabilities(&[], false).is_err());
        assert!(validate_capabilities(&["nope".to_string()], false).is_err());
    }

    #[test]
    fn admin_capabilities_require_an_admin() {
        assert!(validate_capabilities(&["admin.user.read".to_string()], false).is_err());
        assert!(validate_capabilities(&["admin.user.read".to_string()], true).is_ok());
    }

    #[test]
    fn expiry_respects_the_configured_bound() {
        let unbounded = config(0);
        assert_eq!(resolve_expiry(&unbounded, KeyExpiry::Never).unwrap(), None);
        assert!(
            resolve_expiry(&unbounded, KeyExpiry::InDays(7))
                .unwrap()
                .is_some()
        );
        assert!(resolve_expiry(&unbounded, KeyExpiry::InDays(0)).is_err());
        assert!(resolve_expiry(&unbounded, KeyExpiry::At(now() - 1)).is_err());

        let bounded = config(30);
        assert!(resolve_expiry(&bounded, KeyExpiry::Never).is_err());
        assert!(resolve_expiry(&bounded, KeyExpiry::InDays(30)).is_ok());
        assert!(resolve_expiry(&bounded, KeyExpiry::InDays(31)).is_err());
        assert!(resolve_expiry(&bounded, KeyExpiry::At(now() + 31 * SECONDS_PER_DAY)).is_err());
    }

    #[test]
    fn catalog_hides_ungrantable_capabilities() {
        let (entries, presets) = catalog(false);
        assert!(
            !entries.iter().any(|entry| entry.id.starts_with("key.")),
            "key management must not appear in the grantable catalog"
        );
        assert!(
            !entries.iter().any(|entry| entry.id.starts_with("admin.")),
            "admin capabilities are hidden from non-admins"
        );
        let (admin_entries, _) = catalog(true);
        assert!(
            admin_entries
                .iter()
                .any(|entry| entry.id == "admin.user.read")
        );
        assert!(presets.iter().any(|preset| preset.id == "webdav_ro"));
        // Presets are expanded, so a client can post them back verbatim.
        let webdav_ro = presets
            .iter()
            .find(|preset| preset.id == "webdav_ro")
            .expect("preset");
        assert!(webdav_ro.capabilities.contains(&"library.read"));
        assert!(webdav_ro.capabilities.contains(&"webdav.read"));
    }

    #[test]
    fn names_are_trimmed_and_defaulted() {
        assert_eq!(normalize_name(None), "API key");
        assert_eq!(normalize_name(Some("   ".to_string())), "API key");
        assert_eq!(normalize_name(Some("  ci  ".to_string())), "ci");
        assert_eq!(normalize_name(Some("x".repeat(400))).len(), MAX_NAME_LEN);
    }
}
