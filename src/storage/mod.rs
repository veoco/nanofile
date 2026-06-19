pub mod block_store;
pub mod cdc;
pub mod download;
pub mod file_ops;
pub mod gc;
pub mod trash;

pub mod versioning;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, PaginatorTrait, QueryFilter, Set, Statement,
};
use std::sync::Arc;

use crate::entity::{commit, file_lock_timestamp, fs_object, locked_file, repo};
use crate::error::AppError;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::{
    FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};

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
    /// Serialize to compact JSON, compute SHA1, insert if new (skips
    /// duplicate with INSERT OR IGNORE).  Returns the computed fs_id.
    ///
    /// Matches seafile's write_seafile() flow:
    ///   seafile_to_json() → calculate SHA1 → check exists → write.
    ///
    /// Unlike the original implementation, we skip the separate existence-check
    /// SELECT and rely on the UNIQUE(repo_id, fs_id) constraint to silently
    /// ignore duplicates.  This halves the query count for this hot path.
    pub async fn compute_and_store(
        self,
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, AppError> {
        let json = self.to_compact_json();
        let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

        let _ = db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
                vec![
                    repo_id.to_owned().into(),
                    fs_id.clone().into(),
                    (SEAF_METADATA_TYPE_FILE as i8).into(),
                    json.into(),
                ],
            ))
            .await?;

        Ok(fs_id)
    }
}

impl FsDirData {
    /// Serialize to compact JSON, compute SHA1, insert if new (skips
    /// duplicate with INSERT OR IGNORE).  Returns the computed fs_id.
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

        let _ = db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
                vec![
                    repo_id.to_owned().into(),
                    fs_id.clone().into(),
                    (SEAF_METADATA_TYPE_DIR as i8).into(),
                    json.into(),
                ],
            ))
            .await?;

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
/// Uses a single LEFT JOIN query instead of two sequential lookups.
///
/// Matches seafile-server's `check_permission()` in repo-perm.c.
pub async fn check_repo_write_permission(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    use sea_orm::Statement;

    let row: Option<(i32, Option<String>)> = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT r.owner_id, m.permission FROM repos r \
             LEFT JOIN repo_members m ON r.id = m.repo_id AND m.user_id = $1 \
             WHERE r.id = $2",
            vec![user_id.into(), repo_id.to_owned().into()],
        ))
        .await?
        .map(|r| {
            let owner_id: i32 = r.try_get("", "owner_id").unwrap_or(0);
            let permission: Option<String> = r.try_get("", "permission").ok();
            (owner_id, permission)
        });

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _)) if owner_id == user_id => Ok(()),
        Some((_, Some(perm))) if perm == "rw" => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

/// Check if `user_id` has read permission on the repo.
///
/// The repo owner always has access. Any member (r or rw) has access.
/// Non-members are rejected with `AppError::Forbidden`.
///
/// Uses a single LEFT JOIN query instead of two sequential lookups.
pub async fn check_repo_read_permission(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    use sea_orm::Statement;

    let row: Option<(i32, Option<String>)> = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT r.owner_id, m.permission FROM repos r \
             LEFT JOIN repo_members m ON r.id = m.repo_id AND m.user_id = $1 \
             WHERE r.id = $2",
            vec![user_id.into(), repo_id.to_owned().into()],
        ))
        .await?
        .map(|r| {
            let owner_id: i32 = r.try_get("", "owner_id").unwrap_or(0);
            let permission: Option<String> = r.try_get("", "permission").ok();
            (owner_id, permission)
        });

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _)) if owner_id == user_id => Ok(()),
        Some((_, Some(_))) => Ok(()), // any membership grants read access
        _ => Err(AppError::Forbidden),
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

    // Quick skip: if no file locks exist for this repo, skip the expensive
    // full-tree BFS entirely.  The `idx_locked_files_repo_path` index makes
    // this count(*) O(log n) instead of O(files in tree).
    //
    // Most users never use file locking, so this eliminates the vast majority
    // of the traversal cost.
    let lock_count = locked_file::Entity::find()
        .filter(locked_file::Column::RepoId.eq(repo_id))
        .count(db)
        .await?;
    if lock_count == 0 {
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
pub async fn resolve_fs_id(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(root_fs_id.to_string());
    }

    let mut current_fs_id = root_fs_id.to_string();

    for segment in segments {
        let dir_data = read_fs_dir_data(db, repo_id, &current_fs_id).await?;

        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == segment)
            .ok_or_else(|| format!("path segment not found: {segment}"))?;

        current_fs_id = entry.id.clone();
    }

    Ok(current_fs_id)
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

    let parent_fs_id = resolve_fs_id(db, repo_id, &head.root_id, parent_path)
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

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

    /// Helper: create an in-memory SQLite database with the minimal schema
    /// needed for `check_commit_file_locks`.
    async fn setup_lock_test_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        // Create required tables
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            CREATE TABLE repos (
                id VARCHAR(36) PRIMARY KEY NOT NULL,
                name VARCHAR(255) NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                owner_id INTEGER NOT NULL DEFAULT 0,
                encrypted TINYINT NOT NULL DEFAULT 0,
                enc_version TINYINT NOT NULL DEFAULT 0,
                magic VARCHAR(255),
                random_key VARCHAR(255),
                salt VARCHAR(255) NOT NULL DEFAULT '',
                head_commit_id VARCHAR(40),
                permission VARCHAR(10) NOT NULL DEFAULT 'rw',
                created_at BIGINT NOT NULL DEFAULT 0,
                updated_at BIGINT NOT NULL DEFAULT 0,
                size BIGINT NOT NULL DEFAULT 0,
                repo_version INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE commits (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                commit_id VARCHAR(40) NOT NULL,
                root_id VARCHAR(40) NOT NULL,
                parent_id VARCHAR(40),
                second_parent_id VARCHAR(40),
                creator_name VARCHAR(255) NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                ctime BIGINT NOT NULL,
                version TINYINT NOT NULL DEFAULT 1
            );
            CREATE UNIQUE INDEX idx_commits_repo_commit
                ON commits(repo_id, commit_id);

            CREATE TABLE fs_objects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                fs_id VARCHAR(40) NOT NULL,
                obj_type TINYINT NOT NULL,
                data TEXT NOT NULL
            );
            CREATE UNIQUE INDEX idx_fs_objects_repo_fs
                ON fs_objects(repo_id, fs_id);

            CREATE TABLE locked_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                path TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                locked_at BIGINT NOT NULL,
                lock_owner_name VARCHAR(255) NOT NULL DEFAULT ''
            );
            CREATE UNIQUE INDEX idx_locked_files_repo_path
                ON locked_files(repo_id, path);
            ",
        ))
        .await
        .unwrap();

        // Insert a repo using raw SQL to avoid ORM issues with string PKs
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version,
                               salt, permission, created_at, updated_at, size, repo_version)
            VALUES ('test-repo', 'test', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1)
            ",
        ))
        .await
        .unwrap();

        db
    }

    /// check_commit_file_locks should return Ok(()) immediately when no
    /// locked_files records exist for the repo (no tree traversal).
    #[tokio::test]
    async fn test_check_locks_skip_when_no_locks() {
        let db = setup_lock_test_db().await;

        let result = check_commit_file_locks(
            &db,
            "test-repo",
            "0000000000000000000000000000000000000000", // EMPTY_SHA1
            1,
        )
        .await;
        assert!(result.is_ok(), "empty tree should always succeed");
    }

    /// check_commit_file_locks should skip full tree traversal when the
    /// locked_files table has no entries for this repo (even if the tree
    /// itself has many files).
    #[tokio::test]
    async fn test_check_locks_skips_tree_walk_for_unlocked_repo() {
        let db = setup_lock_test_db().await;

        // Seed a root dir with a file
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            r#"
            INSERT INTO fs_objects (repo_id, fs_id, obj_type, data)
            VALUES ('test-repo', 'root-fs-id', 3, '{"dirents":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","mode":33188,"modifier":"u1","mtime":1000,"name":"file.txt","size":100}],"type":3,"version":1}')
            "#,
        ))
        .await
        .unwrap();

        // Seed a commit that references the root dir
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            INSERT INTO commits (repo_id, commit_id, root_id, parent_id, creator_name, description, ctime, version)
            VALUES ('test-repo', 'cccccccccccccccccccccccccccccccccccccccc', 'root-fs-id', NULL, 'u1', '', 1000, 1)
            ",
        ))
        .await
        .unwrap();

        // No locked_files records → should skip tree walk and return Ok
        let result = check_commit_file_locks(&db, "test-repo", "root-fs-id", 1).await;
        assert!(result.is_ok(), "should skip tree walk when no locks exist");
    }

    /// check_commit_file_locks should find a lock conflict.
    #[tokio::test]
    async fn test_check_locks_detects_conflict() {
        let db = setup_lock_test_db().await;

        // Root dir with a file
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            r#"
            INSERT INTO fs_objects (repo_id, fs_id, obj_type, data)
            VALUES ('test-repo', 'root-fs-id', 3, '{"dirents":[{"id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","mode":33188,"modifier":"u1","mtime":1000,"name":"locked.txt","size":100}],"type":3,"version":1}')
            "#,
        ))
        .await
        .unwrap();

        // Insert the file fs_object that the directory entry references
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            r#"
            INSERT INTO fs_objects (repo_id, fs_id, obj_type, data)
            VALUES ('test-repo', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 1, '{"block_ids":["dddddddddddddddddddddddddddddddddddddddd"],"size":100,"type":1,"version":1}')
            "#,
        ))
        .await
        .unwrap();

        // Commit
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            INSERT INTO commits (repo_id, commit_id, root_id, parent_id, creator_name, description, ctime, version)
            VALUES ('test-repo', 'cccccccccccccccccccccccccccccccccccccccc', 'root-fs-id', NULL, 'u1', '', 1000, 1)
            ",
        ))
        .await
        .unwrap();

        // Insert a lock record for this file by user 2
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            INSERT INTO locked_files (repo_id, path, user_id, locked_at, lock_owner_name)
            VALUES ('test-repo', 'locked.txt', 2, 1000, 'user2')
            ",
        ))
        .await
        .unwrap();

        // User 1 tries to commit → should detect lock conflict (locked by user 2)
        let result = check_commit_file_locks(
            &db,
            "test-repo",
            "root-fs-id",
            1, // user_id = 1
        )
        .await;
        assert!(
            result.is_err(),
            "should detect lock conflict for file locked by another user"
        );
        match result {
            Err(AppError::Locked(path)) => assert_eq!(path, "locked.txt"),
            _ => panic!("expected AppError::Locked"),
        }

        // Same user who locked it should pass
        let result = check_commit_file_locks(
            &db,
            "test-repo",
            "root-fs-id",
            2, // user_id = 2 (lock owner)
        )
        .await;
        assert!(result.is_ok(), "lock owner should be allowed to commit");
    }
}
