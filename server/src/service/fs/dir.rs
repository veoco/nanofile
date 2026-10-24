use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::fs::core::file_ops::FileOps;
use crate::repository::Repositories;
use base::common::{DirEntryData, EMPTY_SHA1, FsDirData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::activity_log;
use infra::common::DirEntry;
use infra::common::util::{basename, get_head_root_id, get_head_root_id_opt, parent_path_from};
use infra::serialization::S_IFDIR;

/// Seahub's reserved internal directory.
pub(crate) const RESERVED_INTERNAL_DIR: &str = "_Internal";

/// Whether an entry belongs to seahub's reserved `_Internal` tree.
///
/// nanofile has no wiki/seadoc tree, so `_Internal` is never part of what a user should see;
/// it is hidden from the plain listing, the recursive one, WebDAV and the `reloaddir`
/// responses. Upstream only skips it in `get_recursive_file_list` (the zip/index walk, and
/// there only at the repo root, together with `images`), so this is deliberately stricter
/// than seahub — the one place a client could tell is a library migrated from seahub.
/// Both listing paths share this predicate rather than each spelling the comparison, which
/// is how the recursive one drifted and leaked it.
pub(crate) fn is_reserved_internal(name: &str) -> bool {
    name == RESERVED_INTERNAL_DIR
}

/// Fill the display fields seahub computes for file entries of a listing: the modifier's
/// nickname and contact email (`get_dir_entrys_by_id` / `get_dir_file_info_list` in
/// `seahub/api2/views.py` and `seahub/api2/endpoints/dir.py`).
///
/// One batched lookup per listing; directories carry no modifier. Callers that already
/// ran it (the recursive walk) must not call it twice.
pub(crate) async fn fill_modifier_names(
    repos: &Repositories,
    entries: &mut [DirEntry],
) -> Result<(), AppError> {
    let emails: Vec<String> = entries
        .iter()
        .filter(|e| e.entry_type == "file" && !e.modifier.is_empty())
        .map(|e| e.modifier.clone())
        .collect();
    if emails.is_empty() {
        return Ok(());
    }

    let nickname_by_email: HashMap<String, String> = repos
        .user
        .find_by_emails(&emails)
        .await?
        .into_iter()
        .map(|u| (u.email.clone(), u.nickname()))
        .collect();

    for entry in entries
        .iter_mut()
        .filter(|e| e.entry_type == "file" && !e.modifier.is_empty())
    {
        let fallback = entry.modifier.split('@').next().unwrap_or("").to_string();
        entry.modifier_name = Some(
            nickname_by_email
                .get(&entry.modifier)
                .cloned()
                .unwrap_or(fallback),
        );
        entry.modifier_contact_email = Some(entry.modifier.clone());
    }
    Ok(())
}

/// List directory entries by traversing the FS object tree from the head commit.
pub(crate) async fn list_dir_from_fs_tree(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    list_dir_from_root(repos, repo_id, &head.root_id, path).await
}

/// Like [`list_dir_from_fs_tree`], but reuses an already-resolved root fs_id (skips the repo and
/// head commit lookups).
pub(crate) async fn list_dir_from_root(
    repos: &Repositories,
    repo_id: &str,
    root_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let dir_id = match crate::fs::core::resolve_fs_id(repos, repo_id, root_id, path).await {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            if msg.starts_with("path segment not found") {
                return Err(AppError::NotFound(msg.to_string()));
            }
            return Err(AppError::internal(format!("resolve_fs_id failed: {e}")));
        }
    };

    let dir_data = read_fs_dir_data(repos, repo_id, &dir_id)
        .await
        .map_err(|e| AppError::NotFound(format!("not a directory: {e}")))?;

    Ok((
        dir_id,
        dir_data
            .dirents
            .into_iter()
            .filter(|d| !is_reserved_internal(&d.name))
            .map(|d| DirEntry {
                id: d.id,
                entry_type: if d.mode & S_IFDIR != 0 {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: d.name,
                size: d.size,
                mtime: d.mtime,
                permission: "rw".to_string(),
                modifier: d.modifier,
                parent_dir: None,
                modifier_name: None,
                modifier_contact_email: None,
            })
            .collect(),
    ))
}

/// Recursively list all directory entries from the FS object tree.
pub(crate) async fn list_dir_recursive_from_fs_tree(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let (dir_id, entries, _truncated) =
        list_dir_recursive_from_root(repos, repo_id, &head.root_id, path, true, None).await?;
    Ok((dir_id, entries))
}

/// Like [`list_dir_recursive_from_fs_tree`], but reuses an already-resolved root fs_id.
///
/// `include_modifier_names` controls whether nicknames are batch-fetched for file entries; WebDAV
/// PROPFIND does not use those fields and passes `false` to skip the `find_by_emails` query.
///
/// `max_entries` stops the walk once it has collected more than that many entries and reports the
/// truncation in the returned flag. A caller with an entry cap needs this: checking the collected
/// vec afterwards still materialises the whole tree first, which is exactly the work the cap exists
/// to refuse.
pub(crate) async fn list_dir_recursive_from_root(
    repos: &Repositories,
    repo_id: &str,
    root_id: &str,
    path: &str,
    include_modifier_names: bool,
    max_entries: Option<usize>,
) -> Result<(String, Vec<DirEntry>, bool), AppError> {
    let dir_id = match crate::fs::core::resolve_fs_id(repos, repo_id, root_id, path).await {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            if msg.starts_with("path segment not found") {
                return Err(AppError::NotFound(msg.to_string()));
            }
            return Err(AppError::internal(format!("resolve_fs_id failed: {e}")));
        }
    };

    // Level frontier: each level reads all its directories with one batched `IN` query.
    let mut frontier: Vec<(String, String)> = vec![(dir_id.clone(), path.to_string())];
    let mut entries: Vec<DirEntry> = Vec::new();

    let mut guard = crate::fs::core::traversal::TreeGuard::new();
    while !frontier.is_empty() {
        guard.enter_level()?;
        guard.visit(frontier.len())?;
        let ids: Vec<String> = frontier
            .iter()
            .map(|(fs_id, _)| fs_id.clone())
            .filter(|id| id != EMPTY_SHA1)
            .collect();
        let dir_map = match crate::fs::core::read_fs_dir_data_batch(repos, repo_id, &ids).await {
            Ok(m) => m,
            Err(_) => break,
        };
        let mut next: Vec<(String, String)> = Vec::new();

        for (fs_id, parent_path) in &frontier {
            // Missing/EMPTY dirs are absent from the batch map → skip.
            let Some(dir_data) = dir_map.get(fs_id) else {
                continue;
            };

            for dirent in &dir_data.dirents {
                // The reserved tree is neither listed nor descended into: skipping only the
                // entry would still emit its children, each carrying `_Internal` as parent_dir.
                if is_reserved_internal(&dirent.name) {
                    continue;
                }
                let is_dir = dirent.mode & S_IFDIR != 0;
                let modifier_email = dirent.modifier.clone();

                entries.push(DirEntry {
                    id: dirent.id.clone(),
                    entry_type: if is_dir {
                        "dir".to_string()
                    } else {
                        "file".to_string()
                    },
                    name: dirent.name.clone(),
                    size: dirent.size,
                    mtime: dirent.mtime,
                    permission: "rw".to_string(),
                    modifier: modifier_email.clone(),
                    parent_dir: Some(parent_path.clone()),
                    modifier_name: None,
                    modifier_contact_email: None,
                });
                // Stop as soon as the cap is exceeded; one entry of slack keeps the check out of
                // the hot path and still bounds memory to `max + 1`.
                if max_entries.is_some_and(|max| entries.len() > max) {
                    return Ok((dir_id, entries, true));
                }

                if is_dir {
                    let child_path = if parent_path == "/" {
                        format!("/{}", dirent.name)
                    } else {
                        format!("{}/{}", parent_path, dirent.name)
                    };
                    next.push((dirent.id.clone(), child_path));
                }
            }
        }

        frontier = next;
    }

    if include_modifier_names {
        fill_modifier_names(repos, &mut entries).await?;
    }

    Ok((dir_id, entries, false))
}

pub(crate) async fn create_dir_by_path(
    db: &DatabaseConnection,
    repos: &Repositories,
    email: &str,
    user_id: i32,
    repo_id: &str,
    path: &str,
) -> Result<(), AppError> {
    // Validate and canonicalize the path to prevent directory traversal attacks
    let path = base::sanitize::canonicalize_path(path).map_err(|e| {
        AppError::BadRequest(format!(
            "Invalid directory path: {}. Please ensure the path does not contain '..' components that would escape the repository.",
            e
        ))
    })?;

    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.is_empty() {
        return Err(AppError::BadRequest("invalid path".into()));
    }

    let dir_name = parts.last().unwrap();
    base::sanitize::validate_filename(dir_name)
        .map_err(|e| AppError::BadRequest(format!("invalid directory name: {e}")))?;
    let parent_path = if parts.len() > 1 {
        format!("/{}", parts[..parts.len() - 1].join("/"))
    } else {
        "/".to_string()
    };

    let dir_fs_id = EMPTY_SHA1.to_string();
    let now = chrono::Utc::now().timestamp();

    let parent_fs_id = if parent_path == "/" {
        // Only `Ok(None)` means "no commits yet". Treating a failed lookup as an
        // empty library would base this commit on an empty tree and replace
        // HEAD's whole tree with the new directory.
        match get_head_root_id_opt(db, repo_id).await? {
            Some(root_id) => root_id,
            None => {
                let empty_root = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };
                crate::fs::core::store_fs_dir_object(db, repo_id, &empty_root).await?
            }
        }
    } else {
        let head_root_id = get_head_root_id(db, repo_id).await?;
        crate::fs::core::resolve_dir_fs_id(repos, repo_id, &head_root_id, &parent_path)
            .await
            .map_err(|e| crate::service::fs::resolve_parent_error(e, "resolve parent failed"))?
    };

    let email_clone = email.to_string();
    let dir_name_clone = dir_name.to_string();
    FileOps::update_dir_tree_and_commit(
        db,
        repos,
        repo_id,
        &parent_path,
        &parent_fs_id,
        email,
        &format!("Created directory {dir_name}"),
        crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if !dirents.iter().any(|d| d.name == dir_name_clone) {
                dirents.push(DirEntryData {
                    id: dir_fs_id.clone(),
                    mode: S_IFDIR,
                    modifier: email_clone.clone(),
                    mtime: now,
                    name: dir_name_clone.clone(),
                    size: 0,
                });
            }
            Ok(())
        },
    )
    .await?;

    activity_log::log_activity(
        db, repo_id, "create", "dir", &path, user_id, None, None, None, None, None,
    )
    .await;

    Ok(())
}

async fn read_fs_dir_data(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, AppError> {
    crate::fs::core::read_fs_dir_data(repos, repo_id, fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read fs_dir_data failed: {e}")))
}

pub struct DirService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
    indexer: Option<crate::indexer::TextIndexer>,
    /// Where a path change submits the index work it implies.
    scheduler: crate::service::index::IndexScheduler,
    /// Needed by `create_sub_repo`: the new library's FS objects reference the parent library's
    /// block ids, so the blocks are copied into the new library's own block directory.
    block_store: infra::storage::DynBlockStorage,
    /// Server config: the caller's quota (their own, or the default space quota)
    /// charges a sub-repo copy.
    config: crate::settings::RuntimeConfig,
}

impl DirService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        indexer: Option<crate::indexer::TextIndexer>,
        scheduler: crate::service::index::IndexScheduler,
        block_store: infra::storage::DynBlockStorage,
        config: crate::settings::RuntimeConfig,
    ) -> Self {
        Self {
            repos,
            db,
            indexer,
            scheduler,
            block_store,
            config,
        }
    }

    pub fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Single-level directory listing.
    pub async fn list_dir(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<(String, Vec<DirEntry>), AppError> {
        list_dir_from_fs_tree(&self.repos, repo_id, path).await
    }

    pub async fn list_dir_recursive(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<(String, Vec<DirEntry>), AppError> {
        list_dir_recursive_from_fs_tree(&self.repos, repo_id, path).await
    }

    pub async fn create_dir(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        create_dir_by_path(self.db(), &self.repos, email, user_id, repo_id, path).await
    }

    pub async fn rename_dir_entry(
        &self,
        repo_id: &str,
        path: &str,
        new_name: &str,
        modifier: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        crate::service::fs::file::rename_entry(
            self.db(),
            &self.repos,
            repo_id,
            path,
            new_name,
            modifier,
            user_id,
            true,
            self.indexer.as_ref(),
            &self.scheduler,
        )
        .await
    }

    /// Delete a directory.
    pub async fn delete_dir(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();
        let name = basename(path);
        let parent_path = parent_path_from(path);

        let deleted_size = crate::fs::core::get_entry_total_size(&self.repos, repo_id, path)
            .await
            .ok()
            .unwrap_or(0);

        let head_root_id = get_head_root_id(db, repo_id).await?;
        let parent_fs_id =
            crate::fs::core::resolve_dir_fs_id(&self.repos, repo_id, &head_root_id, parent_path)
                .await
                .map_err(|e| {
                    crate::service::fs::resolve_parent_error(e, "resolve parent failed")
                })?;

        // Record the trash entry before the tree update.
        crate::fs::core::trash::record_deleted_entry(
            db,
            &self.repos,
            repo_id,
            path,
            name,
            email,
            &parent_fs_id,
        )
        .await;

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            email,
            &format!("Deleted directory {name}"),
            crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| d.name != name);
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        crate::fs::core::adjust_repo_size(&self.repos, repo_id, -deleted_size).await?;

        activity_log::log_activity(
            db, repo_id, "delete", "dir", path, user_id, None, None, None, None, None,
        )
        .await;

        if let Err(e) = self
            .repos
            .file_tag
            .delete_by_path_prefix(repo_id, path)
            .await
        {
            tracing::warn!("Failed to delete file tags for {path}: {e}");
        }

        Ok(())
    }

    pub async fn move_dir(
        &self,
        repo_id: &str,
        path: &str,
        new_parent_dir: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        crate::service::fs::file::move_entry(
            self.db(),
            &self.repos,
            repo_id,
            path,
            new_parent_dir,
            email,
            user_id,
            true,
            self.indexer.as_ref(),
            &self.scheduler,
        )
        .await
        .map(|_| ())
    }

    pub async fn get_dir_shared_items(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let links = self
            .repos
            .share_link
            .find_by_repo_and_path(repo_id, path)
            .await?;

        Ok(links
            .into_iter()
            .map(|l| {
                serde_json::json!({
                    "share_type": "download",
                    "token": l.token,
                    "path": l.path,
                    "repo_id": l.repo_id,
                    "creator_email": "",
                    "created_at": l.created_at,
                })
            })
            .collect())
    }

    /// Create a sub-repository from a directory.
    pub async fn create_sub_repo(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;
        let source_dir_fs_id =
            crate::fs::core::resolve_dir_fs_id(&self.repos, repo_id, &head_root_id, path)
                .await
                .map_err(|_| AppError::NotFound("directory not found".into()))?;

        let new_repo_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let dir_name = path
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("subrepo");

        self.repos
            .repo
            .create_repo(crate::repository::repo::CreateRepoParams {
                id: new_repo_id.clone(),
                name: dir_name.to_string(),
                description: String::new(),
                owner_id: user_id,
                encrypted: 0i8,
                enc_version: 0i8,
                magic: None,
                random_key: None,
                salt: String::new(),
                pwd_hash: None,
                pwd_hash_algo: None,
                pwd_hash_params: None,
                permission: "rw".to_string(),
                created_at: now,
                updated_at: now,
                r#type: "repo".to_string(),
            })
            .await?;

        self.repos
            .member
            .create_member(crate::repository::member::CreateMemberParams {
                repo_id: new_repo_id.clone(),
                user_id,
                permission: "rw".to_string(),
                created_at: now,
            })
            .await?;

        copy_fs_tree(&self.repos, repo_id, &new_repo_id, &source_dir_fs_id).await?;

        // `copy_fs_tree` copies FS objects that reference the parent library's block ids; blocks
        // are stored per library, so every download would 404 until they are copied too. Do that
        // **before** the commit, so a failure leaves no library that looks complete but is not.
        let copied_bytes = match copy_subtree_blocks(
            &self.repos,
            &self.block_store,
            repo_id,
            &new_repo_id,
            &source_dir_fs_id,
            user_id,
            self.config.get().storage.default_user_quota_bytes,
        )
        .await
        {
            Ok(bytes) => bytes,
            Err(e) => {
                // Roll the half-built library back rather than leaving it behind, empty or with
                // blocks already on disk (a quota refusal).
                let _ = self.repos.member.delete_by_repo(&new_repo_id).await;
                let _ = self.repos.repo.delete_by_id(&new_repo_id).await;
                return Err(e);
            }
        };

        FileOps::create_commit(
            &self.repos,
            &new_repo_id,
            &source_dir_fs_id,
            email,
            "Created sub-repo",
        )
        .await
        .map_err(|e| AppError::Internal(format!("create commit failed: {e}")))?;

        // Charge the duplicated blocks to the new library: otherwise it reports `size = 0`, so
        // `compute_user_usage` misses the bytes and repeated sub-repo creation is a free disk
        // amplifier.
        if copied_bytes > 0
            && let Err(e) =
                crate::fs::core::adjust_repo_size(&self.repos, &new_repo_id, copied_bytes).await
        {
            tracing::warn!("could not record the sub-repo size: {e}");
        }

        Ok(serde_json::json!({
            // The desktop client reads `sub_repo_id` to open the new library (`api2/views.py:4199`,
            // `desktop/src/api/requests.cpp:388`); without it the flow opened an empty id.
            "sub_repo_id": new_repo_id,
            "id": new_repo_id,
            "name": dir_name,
            "desc": "",
            "size": 0,
            "encrypted": 0,
            "enc_version": 0,
            "owner": email,
            "permission": "rw",
            "mtime": now,
        }))
    }

    /// Delete a directory entry (v21 API, used for both files and directories).
    pub async fn delete_dirent(
        &self,
        repo_id: &str,
        obj: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();
        let name = basename(path);
        let parent_path = parent_path_from(path);

        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("no commits".into()))?;
        let head_commit = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        let parent_fs_id =
            crate::fs::core::resolve_fs_id(&self.repos, repo_id, &head_commit.root_id, parent_path)
                .await
                .map_err(|e| {
                    crate::service::fs::resolve_parent_error(e, "resolve parent failed")
                })?;

        let parent_dir_data =
            crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;

        // Resolve the entry size from the parent dirents (files O(1); directories walk their subtree
        // once) instead of re-resolving the path.
        let deleted_size: i64 = match parent_dir_data.dirents.iter().find(|d| d.name == name) {
            Some(entry) if entry.mode & S_IFDIR != 0 => {
                crate::fs::core::compute_tree_size(&self.repos, repo_id, &entry.id)
                    .await
                    .unwrap_or_default()
            }
            Some(entry) => entry.size,
            None => 0,
        };

        // Trash first: the tree update below removes the dirent.
        if let Some(entry) = parent_dir_data.dirents.iter().find(|d| d.name == name) {
            let obj_type = if entry.mode & S_IFDIR != 0 {
                "dir"
            } else {
                "file"
            };
            if let Err(e) = crate::fs::core::trash::add_to_trash(
                &self.repos,
                repo_id,
                path,
                obj_type,
                &entry.id,
                &entry.name,
                entry.size,
                &head_commit_id,
                email,
            )
            .await
            {
                tracing::warn!("Failed to record trash for {path}: {e}");
            }
        }

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            email,
            &format!("Deleted {name}"),
            crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| d.name != name);
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        if let Some(indexer) = &self.indexer
            && let Err(e) = indexer.delete_file_async(repo_id, path).await
        {
            tracing::warn!("Failed to delete index for {path}: {e}");
        }

        crate::fs::core::adjust_repo_size(&self.repos, repo_id, -deleted_size).await?;

        activity_log::log_activity(
            db, repo_id, "delete", obj, path, user_id, None, None, None, None, None,
        )
        .await;

        Ok(())
    }

    /// Build the v21 directory listing response data (non-recursive), including starred info.
    ///
    /// `permission` is the caller's effective permission on the repo (`"rw"`/`"r"`), the same
    /// string seahub derives with `check_folder_permission`: it is the body's `user_perm` and
    /// every entry's `permission`, because nanofile has no per-folder shares. Both mobile
    /// clients use it to decide whether a folder is editable.
    pub async fn build_list_dir_v21_json(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
        permission: &'static str,
        with_thumbnail: bool,
        entries: Vec<DirEntry>,
        dir_id: String,
    ) -> Result<serde_json::Value, AppError> {
        let user_perm = permission;

        let mut dir_list: Vec<_> = Vec::new();
        let mut file_list: Vec<_> = Vec::new();
        for e in entries {
            if e.entry_type == "dir" {
                dir_list.push(e);
            } else {
                file_list.push(e);
            }
        }
        dir_list.sort_by_key(|a| a.name.to_lowercase());
        file_list.sort_by_key(|a| a.name.to_lowercase());

        let starred_set: HashSet<String> = self
            .repos
            .starred
            .find_by_user_and_repo(user_id, repo_id)
            .await?
            .into_iter()
            .map(|s| s.path.trim_end_matches('/').to_string())
            .collect();

        let parent_dir = if path == "/" {
            path.to_string()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };

        let mut dirent_list = Vec::with_capacity(dir_list.len() + file_list.len());

        for e in &dir_list {
            let entry_path = format!("{}{}", parent_dir, e.name);
            dirent_list.push(serde_json::json!({
                "type": "dir",
                "id": e.id,
                "name": e.name,
                "mtime": e.mtime,
                "permission": permission,
                "parent_dir": parent_dir,
                "starred": starred_set.contains(&entry_path),
            }));
        }

        let mut nickname_cache: HashMap<String, String> = HashMap::new();
        let modifier_emails: Vec<String> = file_list
            .iter()
            .map(|e| e.modifier.clone())
            .filter(|e| !e.is_empty())
            .collect();
        let users = self.repos.user.find_by_emails(&modifier_emails).await?;
        for u in &users {
            nickname_cache.insert(u.email.clone(), u.nickname());
        }
        for email in &modifier_emails {
            nickname_cache
                .entry(email.clone())
                .or_insert_with(|| email.split('@').next().unwrap_or("").to_string());
        }

        for e in &file_list {
            let modifier_email = e.modifier.as_str();
            let modifier_name = nickname_cache
                .get(modifier_email)
                .map(|s| s.as_str())
                .unwrap_or_else(|| modifier_email.split('@').next().unwrap_or(""));
            let modifier_contact_email = modifier_email;
            let entry_path = format!("{}{}", parent_dir, e.name);
            let mut entry = serde_json::json!({
                "type": "file",
                "id": e.id,
                "name": e.name,
                "size": e.size,
                "mtime": e.mtime,
                "permission": permission,
                "parent_dir": parent_dir,
                "starred": starred_set.contains(&entry_path),
                "modifier_email": modifier_email,
                "modifier_name": modifier_name,
                "modifier_contact_email": modifier_contact_email,
            });
            if with_thumbnail {
                entry["encoded_thumbnail_src"] = serde_json::Value::String(String::new());
            }
            dirent_list.push(entry);
        }

        Ok(serde_json::json!({
            "user_perm": user_perm,
            "dir_id": dir_id,
            "dirent_list": dirent_list,
        }))
    }

    /// Get directory detail (v21 API).
    ///
    /// `permission` is the caller's effective repo permission, already narrowed by the
    /// credential ceiling (see `RepoPathRead`).
    pub async fn dir_detail(
        &self,
        repo_id: &str,
        path: &str,
        permission: &'static str,
    ) -> Result<serde_json::Value, AppError> {
        let repo_record = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Library not found".into()))?;
        let head_commit_id = repo_record
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("no commits".into()))?;
        let head_commit = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        let dir_name = path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or("");

        // One tree walk to the parent's dirent yields the mtime, without re-resolving the parent
        // path separately.
        let (_, mtime) =
            crate::fs::core::resolve_file_entry(&self.repos, repo_id, &head_commit.root_id, path)
                .await
                .map_err(|_| AppError::NotFound("Folder not found.".into()))?;

        Ok(serde_json::json!({
            "repo_id": repo_id,
            "path": path,
            "name": dir_name,
            "mtime": mtime,
            "permission": permission,
        }))
    }
}

/// Copy every block referenced by the subtree rooted at `root_fs_id` from `src_repo_id` into
/// `dst_repo_id`. Blocks are stored per library, so the copied FS objects keep the parent's block
/// ids and every download would 404 until the bytes follow them into the new library.
async fn copy_subtree_blocks(
    repos: &Repositories,
    block_store: &infra::storage::DynBlockStorage,
    src_repo_id: &str,
    dst_repo_id: &str,
    root_fs_id: &str,
    owner_id: i32,
    default_quota: u64,
) -> Result<i64, AppError> {
    if root_fs_id == EMPTY_SHA1 {
        return Ok(0);
    }

    // Level-frontier walk of the source subtree, collecting every file object's block ids.
    // `TreeGuard` bounds depth and visits so a corrupt tree cannot spin here.
    let mut frontier = vec![root_fs_id.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    let mut block_ids: HashSet<String> = HashSet::new();
    // Sum of the logical file sizes, so the copy is charged to the owner's quota before any block
    // is written; the walk already parses each file object, so this costs nothing extra.
    let mut total_size: i64 = 0;
    let mut guard = crate::fs::core::traversal::TreeGuard::new();
    while !frontier.is_empty() {
        guard.enter_level()?;
        frontier.retain(|id| *id != EMPTY_SHA1 && seen.insert(id.clone()));
        guard.visit(frontier.len())?;
        if frontier.is_empty() {
            break;
        }

        let objs =
            crate::fs::core::tree::fetch_fs_object_map(repos, src_repo_id, &frontier).await?;
        let mut next = Vec::new();
        for fs_id in &frontier {
            let Some(obj) = objs.get(fs_id) else {
                return Err(AppError::NotFound(format!("fs_object not found: {fs_id}")));
            };
            if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
                let dir_data: FsDirData = serde_json::from_str(&obj.data)
                    .map_err(|e| AppError::Internal(format!("deserialize failed: {e}")))?;
                for entry in &dir_data.dirents {
                    next.push(entry.id.clone());
                }
            } else {
                let file_data: base::common::FsFileData = serde_json::from_str(&obj.data)
                    .map_err(|e| AppError::Internal(format!("deserialize failed: {e}")))?;
                total_size = total_size.saturating_add(file_data.size.max(0));
                for id in file_data.block_ids {
                    block_ids.insert(id);
                }
            }
        }
        frontier = next;
    }

    // Refuse before writing anything: duplicating a folder consumes real disk,
    // and the caller may be over quota already.
    crate::service::fs::quota::check_upload_quota(repos, owner_id, total_size, default_quota)
        .await?;

    for id in block_ids {
        if block_store.has_block(dst_repo_id, &id).await {
            continue;
        }
        let data = block_store
            .read_block(src_repo_id, &id)
            .await
            .map_err(|e| {
                AppError::NotFound(format!(
                    "block {id} referenced by the folder is missing in the source library: {e}"
                ))
            })?;
        block_store
            .write_block_with_id(dst_repo_id, &id, &data)
            .await
            .map_err(|e| AppError::Internal(format!("copy block {id}: {e}")))?;
    }

    Ok(total_size)
}

/// Copy all reachable fs_objects from one repo to another.
async fn copy_fs_tree(
    repos: &Repositories,
    src_repo_id: &str,
    dst_repo_id: &str,
    root_fs_id: &str,
) -> Result<(), AppError> {
    // Level-frontier walk: batch-read each level, batch-skip existing objects, then batch-insert
    // the rest instead of three round-trips per object.
    let mut frontier = vec![root_fs_id.to_string()];
    let mut guard = crate::fs::core::traversal::TreeGuard::new();
    while !frontier.is_empty() {
        guard.enter_level()?;
        guard.visit(frontier.len())?;
        // Content-addressed storage lets one fs_id be referenced from several directories; dedup
        // so each object is fetched and copied once.
        let mut seen = HashSet::new();
        frontier.retain(|id| seen.insert(id.clone()));

        let objs =
            crate::fs::core::tree::fetch_fs_object_map(repos, src_repo_id, &frontier).await?;

        let mut next = Vec::new();
        let mut to_copy: Vec<infra::entity::fs_object::Model> = Vec::new();
        for fs_id in &frontier {
            if *fs_id == EMPTY_SHA1 {
                continue;
            }
            let Some(obj) = objs.get(fs_id) else {
                return Err(AppError::NotFound(format!("fs_object not found: {fs_id}")));
            };
            if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
                let dir_data: FsDirData = serde_json::from_str(&obj.data)
                    .map_err(|e| AppError::Internal(format!("deserialize failed: {e}")))?;
                for entry in &dir_data.dirents {
                    next.push(entry.id.clone());
                }
            }
            to_copy.push(obj.clone());
        }

        let fs_ids: Vec<String> = to_copy.iter().map(|o| o.fs_id.clone()).collect();
        let existing = repos
            .fs_object
            .find_existing_fs_ids(dst_repo_id, &fs_ids)
            .await?;
        let models: Vec<infra::entity::fs_object::ActiveModel> = to_copy
            .into_iter()
            .filter(|o| !existing.contains(&o.fs_id))
            .map(|o| infra::entity::fs_object::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: sea_orm::Set(dst_repo_id.to_string()),
                fs_id: sea_orm::Set(o.fs_id),
                obj_type: sea_orm::Set(o.obj_type),
                data: sea_orm::Set(o.data),
            })
            .collect();
        repos.fs_object.insert_many(models).await?;

        frontier = next;
    }
    Ok(())
}
