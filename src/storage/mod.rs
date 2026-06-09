pub mod block_store;
pub mod cdc;
pub mod download;
pub mod file_ops;
pub mod gc;
pub mod path_cache;
pub mod versioning;

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::{commit, file_lock_timestamp, fs_object, locked_file, repo, repo_member};
use crate::error::AppError;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::{
    FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};
use crate::storage::path_cache::PathCache;

/// SHA1 sentinel for empty directories (seafile convention).
/// seafile-server's seaf_dir_new() forces this when entries are NULL;
/// seaf_dir_save() skips persistence for this value.
pub const EMPTY_SHA1: &str = "0000000000000000000000000000000000000000";

/// Abstract backend for content-addressed block storage.
///
/// Blocks are identified by their SHA-1 hash (40-char hex string) and stored
/// in a two-level directory tree: `{base}/{prefix[..2]}/{block_id}`.
#[async_trait::async_trait]
pub trait BlockStorageBackend: Send + Sync {
    /// Check if a block exists on disk.
    async fn has_block(&self, block_id: &str) -> bool;

    /// Read raw block data by its SHA-1 ID.
    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error>;

    /// Write raw block data, computing and returning its SHA-1 ID.
    async fn write_block(&self, data: &[u8]) -> Result<String, std::io::Error>;

    /// Delete a block file from disk.
    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error>;

    /// List all block IDs stored on disk.
    async fn list_blocks(&self) -> Result<Vec<String>, std::io::Error>;
}

/// Convenience alias for an Arc-wrapped block storage backend.
pub type DynBlockStorage = Arc<dyn BlockStorageBackend>;

/// Create a new filesystem-backed block store at the given directory.
pub fn new_block_store(base_dir: &std::path::Path) -> DynBlockStorage {
    Arc::new(block_store::BlockStorage::new(base_dir.to_path_buf()))
}

// ====================================================================
// Compute-and-store methods on FS data structs
// ====================================================================

impl FsFileData {
    /// Serialize to compact JSON, compute SHA1, check DB, insert if new.
    /// Returns the computed fs_id.
    ///
    /// This is the async FS ID computation API matching the original design.
    /// Callers should use this method instead of manually repeating the
    /// serialize→hash→check→insert pattern.
    ///
    /// Matches seafile's write_seafile() flow:
    ///   seafile_to_json() → calculate SHA1 → check exists → write.
    pub async fn compute_and_store(
        self,
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, AppError> {
        let json = self.to_compact_json();
        let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

        let exists = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?
            .is_some();

        if !exists {
            fs_object::Entity::insert(fs_object::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                fs_id: Set(fs_id.clone()),
                obj_type: Set(SEAF_METADATA_TYPE_FILE as i8),
                data: Set(json),
            })
            .exec(db)
            .await?;
        }

        Ok(fs_id)
    }
}

impl FsDirData {
    /// Serialize to compact JSON, compute SHA1, check DB, insert if new.
    /// Returns the computed fs_id.
    ///
    /// Empty directories use the EMPTY_SHA1 sentinel and are never stored,
    /// matching seafile's seaf_dir_save() / seaf_dir_new() behavior:
    ///   seaf_dir_new: entries==NULL → dir_id = EMPTY_SHA1
    ///   seaf_dir_save: dir_id == EMPTY_SHA1 → skip
    pub async fn compute_and_store(
        self,
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, AppError> {
        // Empty dirs use the EMPTY_SHA1 sentinel per seafile convention.
        if self.dirents.is_empty() {
            return Ok(EMPTY_SHA1.to_string());
        }

        let json = self.to_compact_json();
        let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

        let exists = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?
            .is_some();

        if !exists {
            fs_object::Entity::insert(fs_object::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                fs_id: Set(fs_id.clone()),
                obj_type: Set(SEAF_METADATA_TYPE_DIR as i8),
                data: Set(json),
            })
            .exec(db)
            .await?;
        }

        Ok(fs_id)
    }
}

// ====================================================================
// Wrapper free functions (backward compat, delegate to methods)
// ====================================================================

/// Serialize an FsFileData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
///
/// Prefer calling `file_data.compute_and_store(db, repo_id).await`
/// directly instead.
pub async fn store_fs_file_object(
    db: &DatabaseConnection,
    repo_id: &str,
    file_data: FsFileData,
) -> Result<String, AppError> {
    file_data.compute_and_store(db, repo_id).await
}

/// Serialize an FsDirData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
///
/// Prefer calling `dir_data.compute_and_store(db, repo_id).await`
/// directly instead.
pub async fn store_fs_dir_object(
    db: &DatabaseConnection,
    repo_id: &str,
    dir_data: FsDirData,
) -> Result<String, AppError> {
    dir_data.compute_and_store(db, repo_id).await
}

// ====================================================================
// Permission and lock helpers
// ====================================================================

/// Check if `user_id` has write (`rw`) permission on the repo.
///
/// The repo owner always has full access. Members are checked against
/// `repo_member.permission`. Non-members and read-only members are
/// rejected with `AppError::Forbidden`.
///
/// Matches seafile-server's `check_permission()` in repo-perm.c.
pub async fn check_repo_write_permission(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let repo_model = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    // Owner always has write access
    if repo_model.owner_id == user_id {
        return Ok(());
    }

    // Check repo_member permission
    let member = repo_member::Entity::find()
        .filter(repo_member::Column::RepoId.eq(repo_id))
        .filter(repo_member::Column::UserId.eq(user_id))
        .one(db)
        .await?;

    match member {
        Some(m) if m.permission == "rw" => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

/// Check if `user_id` has read permission on the repo.
///
/// The repo owner always has access. Any member (r or rw) has access.
/// Non-members are rejected with `AppError::Forbidden`.
pub async fn check_repo_read_permission(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let repo_model = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    // Owner always has access
    if repo_model.owner_id == user_id {
        return Ok(());
    }

    let member = repo_member::Entity::find()
        .filter(repo_member::Column::RepoId.eq(repo_id))
        .filter(repo_member::Column::UserId.eq(user_id))
        .one(db)
        .await?;

    match member {
        Some(_) => Ok(()),
        None => Err(AppError::Forbidden),
    }
}

/// Walk the FS tree of a commit and check whether any file in the tree
/// is locked by a different user. Returns `AppError::Locked(path)` with
/// 403 if a lock conflict is found.
///
/// The seafile-daemon parses the 403 body with regex `"File (.+) is locked"`
/// and emits `SYNC_ERROR_ID_FILE_LOCKED`.
pub async fn check_commit_file_locks(
    db: &DatabaseConnection,
    repo_id: &str,
    root_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    if root_id == EMPTY_SHA1 {
        return Ok(());
    }

    let mut stack: Vec<(String, String)> = vec![(root_id.to_string(), String::new())];

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let obj = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?;

        let Some(obj) = obj else {
            continue;
        };

        match obj.obj_type {
            1 => {
                // File — check if locked by another user
                let lock = locked_file::Entity::find()
                    .filter(locked_file::Column::RepoId.eq(repo_id))
                    .filter(locked_file::Column::Path.eq(&path))
                    .one(db)
                    .await?;
                if let Some(lock) = lock
                    && lock.user_id != user_id
                {
                    return Err(AppError::Locked(path));
                }
            }
            3 => {
                let dir_data: FsDirData = serde_json::from_str(&obj.data)
                    .map_err(|e| AppError::Internal(format!("invalid dir object: {e}")))?;
                for entry in &dir_data.dirents {
                    let child_path = if path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", path, entry.name)
                    };
                    stack.push((entry.id.clone(), child_path));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Upsert the file lock timestamp for a repo (used by clients for cache invalidation).
/// Creates a new record if one doesn't exist, updates the timestamp if it does.
pub async fn upsert_lock_timestamp(db: &DatabaseConnection, repo_id: &str) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    let existing = file_lock_timestamp::Entity::find()
        .filter(file_lock_timestamp::Column::RepoId.eq(repo_id))
        .one(db)
        .await?;

    match existing {
        Some(record) => {
            let mut active: file_lock_timestamp::ActiveModel = record.into();
            active.update_time = Set(now);
            active.update(db).await?;
        }
        None => {
            file_lock_timestamp::Entity::insert(file_lock_timestamp::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                update_time: Set(now),
            })
            .exec(db)
            .await?;
        }
    }

    Ok(())
}

// ====================================================================
// Unified FS tree access functions
// ====================================================================

/// Read and parse a directory fs_object (FsDirData) from the database.
pub async fn read_fs_dir_data(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, Box<dyn std::error::Error>> {
    // The zero hash (all zeros) is a sentinel in seafile's protocol,
    // used for empty/incomplete directories or when an fs_object
    // hasn't been fully committed yet. Treat it as an empty directory.
    if fs_id == EMPTY_SHA1 {
        return Ok(FsDirData {
            dirents: vec![],
            obj_type: crate::serialization::fs_json::SEAF_METADATA_TYPE_DIR,
            version: 1,
        });
    }
    let obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?
        .ok_or_else(|| format!("fs_object not found: {fs_id}"))?;
    let data: FsDirData = serde_json::from_str(&obj.data)?;
    Ok(data)
}

/// Read and parse a file fs_object (FsFileData) from the database.
pub async fn read_fs_file_data(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsFileData, Box<dyn std::error::Error>> {
    let obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?
        .ok_or_else(|| format!("fs_object not found: {fs_id}"))?;
    let data: FsFileData = serde_json::from_str(&obj.data)?;
    Ok(data)
}

/// Traverse the FS tree from root_fs_id following path segments,
/// returning the fs_id of the final segment.
///
/// Path should be absolute (e.g. `/dir/subdir/file.txt`).
/// Returns the root_fs_id itself if path is "/" or empty.
///
/// When `cache` is provided, intermediate directories are cached so
/// subsequent traversals of sibling paths skip earlier SQL queries.
pub async fn resolve_fs_id(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
    path: &str,
    cache: Option<&PathCache>,
) -> Result<String, Box<dyn std::error::Error>> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(root_fs_id.to_string());
    }

    // Check cache for the full path first.
    if let Some(cache) = cache
        && let Some((_, fs_id, _)) = cache.get(repo_id, root_fs_id, path)
    {
        return Ok(fs_id);
    }

    let mut current_fs_id = root_fs_id.to_string();
    let mut accumulated = String::new();

    for (i, segment) in segments.iter().enumerate() {
        let dir_data = read_fs_dir_data(db, repo_id, &current_fs_id).await?;

        // Build the path for this segment.
        if i == 0 {
            accumulated = format!("/{}", segment);
        } else {
            accumulated = format!("{}/{}", accumulated, segment);
        }

        // Find the entry FIRST and update current_fs_id to the child's fs_id,
        // then cache using the CHILD's (correct) fs_id and data.
        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == *segment)
            .ok_or_else(|| format!("path segment not found: {segment}"))?;

        if i + 1 < segments.len()
            && let Some(cache) = cache
        {
            // Read the child directory's FsDirData so the cache stores the
            // correct content, NOT the parent's data.
            let child_fs_id = entry.id.clone();
            match read_fs_dir_data(db, repo_id, &child_fs_id).await {
                Ok(child_data) => {
                    let child_json = child_data.to_compact_json();
                    cache.set_dir(repo_id, root_fs_id, &accumulated, &child_fs_id, &child_json);
                }
                Err(_) => {
                    // Child may be EMPTY_SHA1 sentinel or a file — cache
                    // a minimal empty dir so callers can still find it.
                    cache.set_dir(
                        repo_id,
                        root_fs_id,
                        &accumulated,
                        &child_fs_id,
                        r#"{"dirents":[],"type":3,"version":1}"#,
                    );
                }
            }
        } else {
            // Even for the final segment, cache the resolved fs_id so
            // subsequent resolve_fs_id calls for the same path hit quickly.
            if let Some(cache) = cache {
                let child_fs_id = entry.id.clone();
                cache.set_dir(
                    repo_id,
                    root_fs_id,
                    &accumulated,
                    &child_fs_id,
                    r#"{"dirents":[],"type":3,"version":1}"#,
                );
            }
        }

        current_fs_id = entry.id.clone();
    }

    // Cache the final resolution for the full path (already cached
    // segment-by-segment above for intermediate paths, but the full
    // path may differ on the final segment).
    if let Some(cache) = cache {
        let fake_json = r#"{"dirents":[],"type":3,"version":1}"#.to_string();
        cache.set_dir(repo_id, root_fs_id, path, &current_fs_id, &fake_json);
    }

    Ok(current_fs_id)
}

/// Read and parse a directory fs_object, consulting the path cache first.
///
/// Prefer this over `read_fs_dir_data` when you have the `path` and
/// `root_fs_id` available — it avoids the SQL query on a cache hit.
pub async fn read_fs_dir_data_cached(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    cache: &PathCache,
    root_fs_id: &str,
    path: &str,
) -> Result<FsDirData, Box<dyn std::error::Error>> {
    // Check cache first.
    if let Some((3, cached_fs_id, json)) = cache.get(repo_id, root_fs_id, path)
        && cached_fs_id == fs_id
    {
        let data: FsDirData = serde_json::from_str(&json)?;
        return Ok(data);
    }

    // Fall back to DB.
    let data = read_fs_dir_data(db, repo_id, fs_id).await?;
    let json = data.to_compact_json();
    cache.set_dir(repo_id, root_fs_id, path, fs_id, &json);
    Ok(data)
}

/// Read and parse a file fs_object, consulting the path cache first.
pub async fn read_fs_file_data_cached(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    cache: &PathCache,
    root_fs_id: &str,
    path: &str,
) -> Result<FsFileData, Box<dyn std::error::Error>> {
    // Check cache first.
    if let Some((1, cached_fs_id, json)) = cache.get(repo_id, root_fs_id, path)
        && cached_fs_id == fs_id
    {
        let data: FsFileData = serde_json::from_str(&json)?;
        return Ok(data);
    }

    // Fall back to DB.
    let data = read_fs_file_data(db, repo_id, fs_id).await?;
    let json = crate::serialization::fs_json::FsFileData::to_compact_json(&data);
    cache.set_file(repo_id, root_fs_id, path, fs_id, &json);
    Ok(data)
}

// ====================================================================
// Repo size helpers
// ====================================================================

/// Compute the total size of all files in a repository by recursively
/// traversing the FS tree from the repo's head commit.
///
/// Returns 0 if the repo has no commits yet or the tree is empty.
pub async fn compute_repo_size(db: &DatabaseConnection, repo_id: &str) -> Result<i64, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok(0),
    };

    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    compute_tree_size(db, repo_id, &head.root_id).await
}

/// Recursively walk a directory tree (starting at root_fs_id) and sum
/// up all file sizes.
pub async fn compute_tree_size(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
) -> Result<i64, AppError> {
    use std::collections::VecDeque;

    if root_fs_id == EMPTY_SHA1 {
        return Ok(0);
    }

    let mut total: i64 = 0;
    let mut queue = VecDeque::new();
    queue.push_back(root_fs_id.to_string());

    while let Some(fs_id) = queue.pop_front() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let dir_data = match read_fs_dir_data(db, repo_id, &fs_id).await {
            Ok(d) => d,
            Err(_) => continue,
        };

        for entry in &dir_data.dirents {
            if entry.mode & S_IFDIR != 0 {
                queue.push_back(entry.id.clone());
            } else {
                total += entry.size;
            }
        }
    }

    Ok(total)
}

/// Adjust a repo's stored size by `delta` bytes.
///
/// If `repo.size` is 0 (never computed), falls back to a full traversal
/// via `compute_repo_size()` to get a correct baseline.  Otherwise
/// applies the delta incrementally.
pub async fn adjust_repo_size(
    db: &DatabaseConnection,
    repo_id: &str,
    delta: i64,
) -> Result<(), AppError> {
    let r = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let current_size = r.size;
    if current_size == 0 {
        let size = compute_repo_size(db, repo_id).await?;
        let mut active: repo::ActiveModel = r.into();
        active.size = Set(size);
        active.update(db).await?;
    } else {
        let new_size = (current_size + delta).max(0);
        let mut active: repo::ActiveModel = r.into();
        active.size = Set(new_size);
        active.update(db).await?;
    }
    Ok(())
}

/// Look up a single file/dir entry in the FS tree and return its total
/// size.  For a file this is the `DirEntryData.size` field; for a
/// directory the subtree is walked recursively via `compute_tree_size`.
pub async fn get_entry_total_size(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let parent_path = match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("no commits yet".into()))?;
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    let parent_fs_id = resolve_fs_id(db, repo_id, &head.root_id, parent_path, None)
        .await
        .map_err(|e| AppError::internal(format!("resolve parent failed: {e}")))?;

    let dir_data = read_fs_dir_data(db, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read dir failed: {e}")))?;

    let entry = dir_data
        .dirents
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| AppError::NotFound("entry not found".into()))?;

    if entry.mode & S_IFDIR != 0 {
        compute_tree_size(db, repo_id, &entry.id).await
    } else {
        Ok(entry.size)
    }
}
