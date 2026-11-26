//! Trash use cases behind the v2.1 trash and deleted-library endpoints.
//!
//! The handlers only parse their request shape. Wrapping the `fs::core::trash`
//! operations here keeps the last direct filesystem dependency out of `handler`,
//! and it puts the activity entry for a cleanup next to the cleanup it describes.
//! The web pages in `ui` still read the same rows through the same functions.

use sea_orm::DatabaseConnection;
use std::collections::{HashMap, HashSet};

use crate::domain::permission::RepoScope;
use crate::fs::core::trash as core;
use crate::repository::Repositories;
use base::error::AppError;
use infra::activity_log;
use infra::entity::deleted_repo;
use infra::storage::DynBlockStorage;

/// One page of trashed entries for a library (`/api/v2.1/repos/{id}/trash/`).
pub(crate) async fn list_trash2(
    repos: &Repositories,
    repo_id: &str,
    page: u32,
    per_page: u32,
) -> Result<core::TrashListResult, AppError> {
    core::list_trash2(repos, repo_id, page, per_page).await
}

/// Search inside one library's trash.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn search_trash(
    db: &DatabaseConnection,
    repo_id: &str,
    query: &str,
    page: u32,
    per_page: u32,
    op_users: Option<&str>,
    time_from: Option<i64>,
    time_to: Option<i64>,
    suffixes: Option<&str>,
) -> Result<core::TrashListResult, AppError> {
    core::search_trash(
        db, repo_id, query, page, per_page, op_users, time_from, time_to, suffixes,
    )
    .await
}

/// Cursor-based trash listing used by the web trash page.
pub(crate) async fn list_trash_cursor(
    repos: &Repositories,
    repo_id: &str,
    cursor: Option<i64>,
    limit: u32,
) -> Result<core::CursorTrashResult, AppError> {
    core::list_trash_cursor(repos, repo_id, cursor, limit).await
}

/// Restore a map of `commit_id -> paths` from one library's trash.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn restore_trash_items(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    modifier: &str,
    user_id: i32,
    restore_map: HashMap<String, Vec<String>>,
    default_quota: u64,
) -> Result<core::RevertResult, AppError> {
    core::restore_trash_items(
        db,
        repos,
        repo_id,
        modifier,
        user_id,
        restore_map,
        default_quota,
    )
    .await
}

/// Restore the named entries of one commit from the trash.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn restore_dirents(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    modifier: &str,
    user_id: i32,
    commit_id: &str,
    paths: Vec<String>,
    default_quota: u64,
) -> Result<core::RevertResult, AppError> {
    core::restore_dirents(
        db,
        repos,
        repo_id,
        modifier,
        user_id,
        commit_id,
        paths,
        default_quota,
    )
    .await
}

/// Empty the entries older than `keep_days` and record the cleanup in the
/// activity log. Logging happens here so a caller cannot clean without it.
pub(crate) async fn clean_trash(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
    keep_days: Option<i64>,
) -> Result<u64, AppError> {
    let cleaned = core::clean_trash(repos, repo_id, keep_days).await?;
    activity_log::log_activity(
        db,
        repo_id,
        "clean-up-trash",
        "repo",
        "/",
        user_id,
        None,
        None,
        None,
        None,
        keep_days,
    )
    .await;
    Ok(cleaned)
}

/// The libraries in the caller's trash.
pub(crate) async fn list_deleted_repos(
    repos: &Repositories,
    user_id: i32,
    scope: &RepoScope,
) -> Result<Vec<deleted_repo::Model>, AppError> {
    core::list_deleted_repos(repos, user_id, scope).await
}

/// Restore one library from the trash.
pub(crate) async fn restore_deleted_repo(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
    default_quota: u64,
) -> Result<(), AppError> {
    core::restore_deleted_repo(db, repos, repo_id, user_id, default_quota).await
}

/// Permanently delete one library from the trash and reclaim its blocks.
pub(crate) async fn purge_deleted_repo(
    db: &DatabaseConnection,
    repos: &Repositories,
    block_store: &DynBlockStorage,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    core::purge_deleted_repo(db, repos, block_store, repo_id, user_id).await
}

/// Empty every library in the caller's trash that this credential may write to.
pub(crate) async fn purge_deleted_repos_of_owner(
    db: &DatabaseConnection,
    repos: &Repositories,
    block_store: &DynBlockStorage,
    user_id: i32,
    scope: &RepoScope,
    writable: &HashSet<String>,
) -> Result<u64, AppError> {
    core::purge_deleted_repos_of_owner(db, repos, block_store, user_id, scope, writable).await
}
