//! Ownership checks shared by share links and upload links.
//!
//! The two link kinds had their own copies of "find the row, compare the creator
//! and the key's library scope": one copy hid a foreign share link as 404 while
//! the upload-link copy answered 403, and a missing token was 404 in some paths
//! and success in others. Reads and updates hide a foreign link as 404; deletes
//! follow seahub, where a missing token is already gone (success) and a foreign
//! owner is a permission error (403).

pub mod link;
pub mod share;

use crate::domain::permission::RepoScope;
use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::{share_link, upload_link};

/// The share link named by `token`, visible to `user_id` under `scope`.
///
/// A missing token and a link owned by somebody else both answer 404: the token
/// must not confirm its own existence to an unrelated account.
pub(crate) async fn resolve_owned_share_link(
    repos: &Repositories,
    token: &str,
    user_id: i32,
    scope: &RepoScope,
) -> Result<share_link::Model, AppError> {
    let link = repos
        .share_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Share link not found".into()))?;
    if link.creator_id != user_id || !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Share link not found".into()));
    }
    Ok(link)
}

/// [`resolve_owned_share_link`] for upload links.
pub(crate) async fn resolve_owned_upload_link(
    repos: &Repositories,
    token: &str,
    user_id: i32,
    scope: &RepoScope,
) -> Result<upload_link::Model, AppError> {
    let link = repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;
    if link.creator_id != user_id || !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Upload link not found".into()));
    }
    Ok(link)
}

/// The share link named by `token` when the caller may delete it.
///
/// `Ok(None)` means the token no longer resolves: seahub's delete is idempotent,
/// so the caller reports success. A different owner is 403, as in
/// `ShareLink.delete`; a link outside the presented key's library scope is hidden
/// as 404, because the key must not learn that the token exists.
pub(crate) async fn owned_share_link_for_delete(
    repos: &Repositories,
    token: &str,
    user_id: i32,
    scope: &RepoScope,
) -> Result<Option<share_link::Model>, AppError> {
    let Some(link) = repos.share_link.find_by_token(token).await? else {
        return Ok(None);
    };
    if link.creator_id != user_id {
        return Err(AppError::Forbidden);
    }
    if !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Share link not found".into()));
    }
    Ok(Some(link))
}

/// [`owned_share_link_for_delete`] for upload links.
pub(crate) async fn owned_upload_link_for_delete(
    repos: &Repositories,
    token: &str,
    user_id: i32,
    scope: &RepoScope,
) -> Result<Option<upload_link::Model>, AppError> {
    let Some(link) = repos.upload_link.find_by_token(token).await? else {
        return Ok(None);
    };
    if link.creator_id != user_id {
        return Err(AppError::Forbidden);
    }
    if !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Upload link not found".into()));
    }
    Ok(Some(link))
}
