use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

use crate::AppState;
use crate::handler::json_or_form::JsonOrForm;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use crate::service::sharing::{link, share};
use base::error::AppError;

/// Map JSON `null` to `Some(None)` and a value to `Some(Some(v))`; a missing
/// field stays `None` (via `#[serde(default)]`) — "don't update" vs "set null".
fn deserialize_nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(Some(Option::<T>::deserialize(d)?))
}

#[derive(Deserialize)]
pub struct CreateLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expire_days: Option<i64>,
    /// ISO 8601 datetime (desktop client sends this instead of `expire_days`).
    #[serde(default)]
    pub expiration_time: Option<String>,
    pub description: Option<String>,
}

fn parse_link_request(v: &serde_json::Value) -> Result<CreateLinkRequest, AppError> {
    let get_str = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.to_string());
    let get_i64 = |k: &str| {
        v.get(k).and_then(|x| match x {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        })
    };

    let repo_id = get_str("repo_id")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("repo_id invalid".into()))?;
    let path = get_str("path")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("path invalid".into()))?;

    Ok(CreateLinkRequest {
        repo_id,
        path,
        password: get_str("password"),
        expire_days: get_i64("expire_days"),
        expiration_time: get_str("expiration_time"),
        description: get_str("description"),
    })
}

/// Normalize `expiration_time` into `expire_days` when absent (seahub parity).
fn resolve_expire_days(req: &CreateLinkRequest) -> Result<Option<i64>, AppError> {
    if req.expire_days.is_some() {
        return Ok(req.expire_days);
    }
    match &req.expiration_time {
        Some(et) => {
            let dt = chrono::DateTime::parse_from_rfc3339(et).map_err(|_| {
                AppError::BadRequest(
                    "expiration_time invalid, should be ISO format e.g. 2020-05-17T10:26:22+08:00"
                        .into(),
                )
            })?;
            let now = chrono::Utc::now();
            let days = dt.timestamp() - now.timestamp();
            Ok(Some((days + 86399).max(1) / 86400))
        }
        None => Ok(None),
    }
}

/// Enforce the caller's per-library write ceiling for a link token.
///
/// A link token names its library only in the database, so the route-table
/// ceiling check never sees it: creating/updating/deleting a share or upload
/// link must be a write on that library (`r` ceilings stay read-only). A token
/// outside the caller's library scope is deliberately left to the service,
/// which answers `NotFound`; returning `Forbidden` there would confirm that the
/// token exists, so this only narrows a scope that already allows the library.
fn ensure_link_write_ceiling(auth: &AuthUser, repo_id: &str) -> Result<(), AppError> {
    if auth.repo_scope().allows(repo_id) {
        auth.ensure_repo_allowed(repo_id, true)?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ListShareLinksQuery {
    pub repo_id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct ListUploadLinksQuery {
    pub repo_id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateLinkRequest {
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub password: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub expire_days: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub description: Option<Option<String>>,
}

/// GET /api/v2.1/share-links/
///
/// Optional filters: `repo_id`, and `path` (exact path, used with `repo_id`).
pub async fn list_share_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListShareLinksQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let base_url = &state.config().server.site_url;
    let scope = auth.repo_scope();
    let infos = if let (Some(repo_id), Some(path)) = (&query.repo_id, &query.path) {
        // Filtering by a repo requires membership of that repo, and — for a
        // key — that the repo is inside its library scope.
        if !scope.allows(repo_id) {
            return Err(AppError::Forbidden);
        }
        crate::domain::permission::check_repo_read_permission(
            state.repos.member.as_ref(),
            repo_id,
            auth.user_id,
        )
        .await?;
        share::list_share_links_for_path(&state.repos, base_url, repo_id, path, auth.user_id)
            .await?
    } else {
        let mut links = share::list_share_links(&state.repos, base_url, auth.user_id).await?;
        // A share token is a capability URL, so listing one for a library
        // outside the key's scope would hand the key that library.
        links.retain(|link| scope.allows(&link.repo_id));
        links
    };
    let items: Vec<serde_json::Value> = infos.iter().map(share::share_link_json).collect();

    Ok(Json(serde_json::Value::Array(items)))
}

/// POST /api/v2.1/share-links/
pub async fn create_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    JsonOrForm(v): JsonOrForm,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::middleware::ensure_share_download_enabled(&state)?;
    let req = parse_link_request(&v)?;
    let expire_days = resolve_expire_days(&req)?;
    // A share link is a capability URL granting anonymous read access, so a
    // read-ceiling key must not mint one for its library.
    auth.ensure_repo_allowed(&req.repo_id, true)?;
    let info = share::create_share_link_v21(
        &state.repos,
        &state.config(),
        &req.repo_id,
        &req.path,
        req.password.as_deref(),
        expire_days,
        req.description.as_deref(),
        auth.user_id,
        &auth.repo_scope(),
    )
    .await?;

    Ok(Json(share::share_link_created_json(&info)))
}

/// GET /api/v2.1/share-links/{token}/
pub async fn get_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = share::get_share_link_v21(
        &state.repos,
        &state.config().server.site_url,
        &token,
        auth.user_id,
        &auth.repo_scope(),
    )
    .await?;

    Ok(Json(share::share_link_detail_json(&info)))
}

/// POST /api/v2.1/multi-share-links/
///
/// Identical to POST /api/v2.1/share-links/; used by the Android client.
pub async fn create_multi_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    JsonOrForm(v): JsonOrForm,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::middleware::ensure_share_download_enabled(&state)?;
    let req = parse_link_request(&v)?;
    let expire_days = resolve_expire_days(&req)?;
    // Same capability URL as POST /share-links/: the caller needs write access
    // to the library, not merely its read ceiling.
    auth.ensure_repo_allowed(&req.repo_id, true)?;
    let info = share::create_share_link_v21(
        &state.repos,
        &state.config(),
        &req.repo_id,
        &req.path,
        req.password.as_deref(),
        expire_days,
        req.description.as_deref(),
        auth.user_id,
        &auth.repo_scope(),
    )
    .await?;

    Ok(Json(share::share_link_created_json(&info)))
}

/// DELETE /api/v2.1/share-links/{token}/
pub async fn delete_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // The token names its library only in the database; revoking a capability
    // URL is a write on that library.
    if let Some(link) = state.repos.share_link.find_by_token(&token).await? {
        ensure_link_write_ceiling(&auth, &link.repo_id)?;
    }
    let found =
        share::delete_share_link_v21(&state.repos, &token, auth.user_id, &auth.repo_scope())
            .await?;
    if !found {
        return Err(AppError::NotFound("share link not found".into()));
    }
    Ok(ok_json())
}

/// PUT /api/v2.1/share-links/{token}/
pub async fn update_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(req): Json<UpdateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(link) = state.repos.share_link.find_by_token(&token).await? {
        ensure_link_write_ceiling(&auth, &link.repo_id)?;
    }
    let info = share::update_share_link_v21(
        &state.config(),
        &state.repos,
        &token,
        auth.user_id,
        req.password,
        req.expire_days,
        req.description,
        &auth.repo_scope(),
    )
    .await?;

    Ok(Json(share::share_link_json(&info)))
}

/// GET /api/v2.1/upload-links/
///
/// Optional filters: `repo_id`, and `path` (exact path, used with `repo_id`).
pub async fn list_upload_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListUploadLinksQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scope = auth.repo_scope();
    let items = if let (Some(repo_id), Some(path)) = (&query.repo_id, &query.path) {
        // Filtering by a repo requires membership of that repo, and — for a
        // key — that the repo is inside its library scope.
        if !scope.allows(repo_id) {
            return Err(AppError::Forbidden);
        }
        crate::domain::permission::check_repo_read_permission(
            state.repos.member.as_ref(),
            repo_id,
            auth.user_id,
        )
        .await?;
        link::list_upload_links_for_path(&state.repos, repo_id, path, auth.user_id).await?
    } else {
        let mut items = link::list_upload_links_v21(&state.repos, auth.user_id).await?;
        items.retain(|item| {
            item.get("repo_id")
                .and_then(|value| value.as_str())
                .is_some_and(|repo_id| scope.allows(repo_id))
        });
        items
    };
    Ok(Json(serde_json::Value::Array(items)))
}

/// POST /api/v2.1/upload-links/
pub async fn create_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    JsonOrForm(v): JsonOrForm,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::middleware::ensure_share_upload_enabled(&state)?;
    let req = parse_link_request(&v)?;
    let expire_days = resolve_expire_days(&req)?;
    let has_password = req.password.is_some();
    // An upload link is anonymous write access: a read-ceiling key must not
    // mint one even though it is bound to the library.
    auth.ensure_repo_allowed(&req.repo_id, true)?;

    let info = link::create_upload_link_v21(
        &state.repos,
        &state.config(),
        &req.repo_id,
        &req.path,
        req.password,
        expire_days,
        req.description,
        auth.user_id,
        &auth.repo_scope(),
    )
    .await?;

    Ok(Json(link::upload_link_created_json(&info, has_password)))
}

/// DELETE /api/v2.1/upload-links/clean-invalid/
pub async fn clean_invalid_upload_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let scope = auth.repo_scope();
    // Cleaning is a write on the links' libraries, so a read-ceiling key must
    // not delete them — the service skips links the closure refuses.
    let count =
        link::clean_invalid_upload_links_v21(&state.repos, auth.user_id, &scope, |repo_id| {
            auth.ensure_repo_allowed(repo_id, true).is_ok()
        })
        .await?;
    Ok(Json(serde_json::json!({"success": true, "deleted": count})))
}

/// GET /api/v2.1/upload-links/{token}/
pub async fn get_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = link::get_upload_link_v21(
        &state.repos,
        &state.config().server.site_url,
        &token,
        auth.user_id,
        &auth.repo_scope(),
    )
    .await?;
    Ok(Json(info))
}

/// PUT /api/v2.1/upload-links/{token}/
pub async fn update_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(req): Json<UpdateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let existing = state.repos.upload_link.find_by_token(&token).await?;
    if let Some(link) = &existing {
        ensure_link_write_ceiling(&auth, &link.repo_id)?;
    }
    let updated = link::update_upload_link_v21(
        &state.repos,
        &state.config(),
        &token,
        auth.user_id,
        req.expire_days,
        req.password,
        req.description,
        &auth.repo_scope(),
    )
    .await?;

    if !updated {
        return Err(AppError::NotFound("upload link not found".into()));
    }
    // The link's password and expiry gate the anonymous upload tokens already
    // minted from it; a successful update must invalidate them, or a visitor
    // holding a one-hour token keeps uploading under the terms just changed.
    if let Some(link) = existing {
        let revoked = state.token_manager.revoke_upload_link(link.id);
        if revoked > 0 {
            tracing::debug!(
                link_id = link.id,
                revoked,
                "revoked upload-link tokens after link update"
            );
        }
    }
    Ok(ok_json())
}

/// DELETE /api/v2.1/upload-links/{token}/
///
/// Returns bare `true` (not an object): the Android client parses this as `Single<Boolean>`.
pub async fn delete_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Revoking an anonymous write capability is a write on its library.
    if let Some(link) = state.repos.upload_link.find_by_token(&token).await? {
        ensure_link_write_ceiling(&auth, &link.repo_id)?;
    }
    let found = link::delete_upload_link_v21_by_token(
        &state.repos,
        &token,
        auth.user_id,
        Some(&state.token_manager),
        &auth.repo_scope(),
    )
    .await?;
    if !found {
        return Err(AppError::NotFound("upload link not found".into()));
    }
    Ok(Json(serde_json::Value::Bool(true)))
}

/// GET /api/v2.1/upload-links/{token}/upload/
///
/// Returns a short-lived upload URL for anyone holding the token.
pub async fn get_upload_link_upload_url_v21(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // The server-wide kill switch must gate this token mint too: otherwise a
    // shared token still yields an upload URL while anonymous links are disabled.
    crate::middleware::ensure_share_upload_enabled(&state)?;

    // No CSRF guard on purpose: the capability is the link token itself, which a
    // holder can already exchange, and `public-upload.js` legitimately calls this
    // with `credentials: same-origin` and no CSRF header.

    let link = state
        .repos
        .upload_link
        .find_by_token(&token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    if let Some(exp) = link.expires_at
        && chrono::Utc::now().timestamp() > exp
    {
        return Err(AppError::NotFound("Upload link has expired".into()));
    }

    // Password-protected links need the `visited_ufs_{token}` session flag set
    // by the web password form (matching seahub), or the password is bypassed.
    // The signature covers the link's current password version, so a cookie
    // issued before a password change no longer mints an upload token.
    if link.password.is_some()
        && !crate::service::auth::csrf::has_valid_upload_link_cookie(
            headers.get("cookie").and_then(|v| v.to_str().ok()),
            &token,
            link.password.as_deref().unwrap_or(""),
            &state.csrf_secret,
        )
    {
        return Err(AppError::Forbidden);
    }

    let repo = state
        .repos
        .repo
        .find_by_id(&link.repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    // An anonymous upload link cannot write ciphertext into an encrypted library
    // (no per-user key cache entry for the visitor), so refuse to mint a token;
    // official clients never offer this action for encrypted libraries.
    if repo.encrypted != 0 {
        return Err(AppError::BadRequest(
            "upload links are not supported for encrypted libraries".into(),
        ));
    }
    let _repo = repo;

    // The upload token acts as the link's creator, so minting it requires that
    // the creator still has write access (the token endpoint re-checks this too).
    if !crate::service::sharing::share::link_creator_may_access(
        &state.repos,
        link.creator_id,
        &link.repo_id,
        true,
    )
    .await
    {
        return Err(AppError::Forbidden);
    }

    let creator = state
        .repos
        .user
        .find_by_id(link.creator_id)
        .await?
        .ok_or_else(|| AppError::Internal("Creator not found".into()))?;

    let upload_token = state.token_manager.generate(
        &link.repo_id,
        link.creator_id,
        &creator.email,
        "upload",
        &link.path,
    );

    // Link the upload token back to the upload link so we can count uploads, and
    // carry the link's own expiry: the token's one-hour lifetime must not outlive
    // a link that expires in five minutes.
    state
        .token_manager
        .set_upload_link(&upload_token, link.id, link.expires_at);

    let upload_url = format!("/upload-aj/{}", upload_token);

    Ok(Json(serde_json::json!({"upload_link": upload_url})))
}

/// GET /api/v2.1/repos/{repo_id}/upload-links/
pub async fn list_repo_upload_links_v21(
    path: crate::middleware::repo_extractor::RepoPathRead,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let items = link::list_upload_links_for_repo_v21(
        &state.repos,
        &state.config().server.site_url,
        &repo_id,
        path.user.user_id,
    )
    .await?;
    Ok(Json(serde_json::Value::Array(items)))
}
