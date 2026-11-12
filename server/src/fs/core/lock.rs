use std::collections::HashMap;

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::repository::Repositories;
use base::common::EMPTY_SHA1;
use base::error::AppError;
use infra::entity::file_lock_timestamp;

/// Longest lock path accepted, matching the server's general path bound.
const MAX_LOCK_PATH_LEN: usize = 4096;

/// Canonical key for a file lock: the repo-relative path the sync tree walk
/// and the clients' indexes use (`dir/file`), with no leading `/`.
///
/// The REST surface receives an absolute repo path (`/dir/file`) while the
/// sync protocol and older rows may carry either form. Normalizing here keeps
/// a lock created through one endpoint visible — and unlockable — through the
/// other, and lets the commit-time check match it.
pub fn canonical_lock_path(path: &str) -> Result<String, AppError> {
    let relative = path.trim_start_matches('/');
    if relative.is_empty() {
        return Err(AppError::BadRequest("cannot lock the library root".into()));
    }
    if relative.len() > MAX_LOCK_PATH_LEN {
        return Err(AppError::BadRequest("lock path too long".into()));
    }
    if relative.contains('\0')
        || relative
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(AppError::BadRequest("invalid lock path".into()));
    }
    Ok(relative.to_string())
}

/// Key for reading a stored lock. Rows written before lock paths were
/// canonicalized may still carry a leading `/`; they must address the same
/// file as new rows.
fn lock_path_key(path: &str) -> &str {
    path.trim_start_matches('/')
}

/// Reject a sync commit that changes a file locked by a different user,
/// returning `AppError::Locked(path)` (403) on conflict.
///
/// The paths considered are the diff against the commit's parent, not the whole
/// new tree: an unrelated or stale lock must not block work on other files, and
/// deleting or renaming a locked path has to be caught as well. The seafile
/// daemon parses the 403 body with regex `"File (.+) is locked"` and emits
/// `SYNC_ERROR_ID_FILE_LOCKED`.
pub async fn check_commit_file_locks(
    repos: &Repositories,
    repo_id: &str,
    base_root: Option<&str>,
    new_root: &str,
    user_id: i32,
) -> Result<(), AppError> {
    if new_root == EMPTY_SHA1 {
        return Ok(());
    }

    // Fast path: no lock in the library, no diff to walk.
    let locks = load_locks(repos, repo_id).await?;
    if locks.is_empty() {
        return Ok(());
    }

    let changes =
        crate::fs::core::tree_diff::diff_trees(repos, repo_id, base_root, new_root).await?;
    for change in changes {
        // A rename/move reports the new path and the old one; both belong to the
        // change, so a lock on either side rejects it.
        for path in std::iter::once(change.path.as_str()).chain(change.old_path.as_deref()) {
            let key = lock_path_key(path);
            if key.is_empty() {
                continue;
            }
            if let Some(owners) = locks.get(key)
                && owners.iter().any(|owner| *owner != user_id)
            {
                return Err(AppError::Locked(key.to_string()));
            }
        }
    }

    Ok(())
}

/// Load a library's locks keyed by canonical path. Rows written before locks
/// were canonicalized may still carry a leading `/`, and a file can briefly
/// have both forms (one per endpoint), so keep every owner per path.
async fn load_locks(
    repos: &Repositories,
    repo_id: &str,
) -> Result<HashMap<String, Vec<i32>>, AppError> {
    let mut locks: HashMap<String, Vec<i32>> = HashMap::new();
    for lock in repos.locked_file.find_by_repo(repo_id).await? {
        let key = lock_path_key(&lock.path);
        if !key.is_empty() {
            locks.entry(key.to_string()).or_default().push(lock.user_id);
        }
    }
    Ok(locks)
}

/// Delete lock rows whose path is absent from the library's current HEAD.
///
/// A commit that deletes or renames a file leaves its row behind; such a row
/// would block a later file created at the same path and show up in listings.
/// The sync commit path sweeps after advancing HEAD, and the expired-data
/// cleanup task sweeps every library that still has locks.
pub async fn prune_stale_file_locks(
    repos: &Repositories,
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<u64, AppError> {
    let locks = repos.locked_file.find_by_repo(repo_id).await?;
    if locks.is_empty() {
        return Ok(0);
    }

    let head_root = match repos.repo.find_by_id(repo_id).await? {
        // The library is gone; its rows go with the FK cascade.
        None => None,
        Some(repo) => match repo.head_commit_id.as_deref().filter(|h| *h != EMPTY_SHA1) {
            None => None,
            Some(head) => repos
                .commit
                .find_by_repo_and_commit_id(repo_id, head)
                .await?
                .map(|commit| commit.root_id),
        },
    };

    let mut removed = 0u64;
    for lock in locks {
        let key = lock_path_key(&lock.path);
        let exists = match &head_root {
            Some(root) if !key.is_empty() => {
                match crate::fs::core::resolve_fs_id(repos, repo_id, root, key).await {
                    Ok(_) => true,
                    Err(AppError::NotFound(_)) => false,
                    Err(e) => return Err(e),
                }
            }
            _ => false,
        };
        if !exists {
            repos
                .locked_file
                .delete_by_repo_and_path(repo_id, &lock.path)
                .await?;
            removed += 1;
        }
    }

    if removed > 0 {
        // Clients invalidate their cached lock lists from this timestamp.
        upsert_lock_timestamp(db, repo_id).await?;
    }

    Ok(removed)
}

/// Sweep every library that still has locks; the periodic cleanup's entry point.
pub async fn prune_all_stale_file_locks(
    repos: &Repositories,
    db: &DatabaseConnection,
) -> Result<u64, AppError> {
    let mut removed = 0u64;
    for repo_id in repos.locked_file.find_repo_ids().await? {
        removed += prune_stale_file_locks(repos, db, &repo_id).await?;
    }
    Ok(removed)
}

/// Upsert the repo's lock timestamp (clients use it for cache invalidation).
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

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

    /// In-memory SQLite database with the minimal schema for the checks below.
    async fn setup_lock_test_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        db.execute_raw(Statement::from_string(
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
                pwd_hash VARCHAR(255),
                pwd_hash_algo VARCHAR(255),
                pwd_hash_params VARCHAR(255),
                head_commit_id VARCHAR(40),
                permission VARCHAR(10) NOT NULL DEFAULT 'rw',
                created_at BIGINT NOT NULL DEFAULT 0,
                updated_at BIGINT NOT NULL DEFAULT 0,
                size BIGINT NOT NULL DEFAULT 0,
                repo_version INTEGER NOT NULL DEFAULT 1,
                history_limit INTEGER NOT NULL DEFAULT 0,
                history_ttl_days INTEGER NOT NULL DEFAULT 0,
                type VARCHAR(20) NOT NULL DEFAULT 'repo'
            );

            CREATE TABLE commits (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                commit_id VARCHAR(40) NOT NULL,
                root_id VARCHAR(40) NOT NULL,
                parent_id VARCHAR(40),
                second_parent_id VARCHAR(40),
                creator_name VARCHAR(255) NOT NULL,
                creator VARCHAR(40) NOT NULL DEFAULT '0000000000000000000000000000000000000000',
                description TEXT NOT NULL DEFAULT '',
                ctime BIGINT NOT NULL,
                version TINYINT NOT NULL DEFAULT 1,
                new_merge BOOLEAN,
                conflict BOOLEAN
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

            CREATE TABLE file_lock_timestamps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL UNIQUE,
                update_time BIGINT NOT NULL
            );
            ",
        ))
        .await
        .unwrap();

        // Insert a repo using raw SQL to avoid ORM issues with string PKs
        db.execute_raw(Statement::from_string(
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

    use std::sync::Arc;

    fn test_repos(db: &DatabaseConnection) -> Repositories {
        Repositories::new_for_tests(Arc::new(db.clone()))
    }

    /// Insert a directory object with flat `(name, file_id)` entries. The file
    /// ids need not resolve: a diff only reads directory objects.
    async fn insert_dir(db: &DatabaseConnection, fs_id: &str, entries: &[(&str, &str)]) {
        let dirents: Vec<String> = entries
            .iter()
            .map(|(name, id)| {
                format!(
                    r#"{{"id":"{id}","mode":33188,"modifier":"u1","mtime":1000,"name":"{name}","size":100}}"#
                )
            })
            .collect();
        let data = format!(
            r#"{{"dirents":[{}],"type":3,"version":1}}"#,
            dirents.join(",")
        );
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) \
                 VALUES ('test-repo', '{fs_id}', 3, '{data}')"
            ),
        ))
        .await
        .unwrap();
    }

    async fn insert_lock(db: &DatabaseConnection, path: &str, user_id: i32) {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO locked_files (repo_id, path, user_id, locked_at, lock_owner_name) \
                 VALUES ('test-repo', '{path}', {user_id}, 1000, 'user{user_id}')"
            ),
        ))
        .await
        .unwrap();
    }

    const F1: &str = "1111111111111111111111111111111111111111";
    const F2: &str = "2222222222222222222222222222222222222222";

    /// No lock rows means no diff and no error, even for a populated tree.
    #[tokio::test]
    async fn test_check_locks_skip_when_no_locks() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "root", &[("file.txt", F1)]).await;
        let repos = test_repos(&db);

        let result = check_commit_file_locks(&repos, "test-repo", None, "root", 1).await;
        assert!(result.is_ok());
    }

    /// A locked file the commit does not touch must not block work elsewhere.
    #[tokio::test]
    async fn test_unrelated_change_ignores_a_foreign_lock() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "base", &[("locked.txt", F1), ("other.txt", F1)]).await;
        insert_dir(&db, "new", &[("locked.txt", F1), ("other.txt", F2)]).await;
        insert_lock(&db, "locked.txt", 2).await;
        let repos = test_repos(&db);

        let result = check_commit_file_locks(&repos, "test-repo", Some("base"), "new", 1).await;
        assert!(
            result.is_ok(),
            "an unrelated edit must not be blocked by a foreign lock: {result:?}"
        );
    }

    /// A lock whose path exists in neither tree is stale, not a conflict.
    #[tokio::test]
    async fn test_stale_lock_ignores_an_unrelated_change() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "base", &[("other.txt", F1)]).await;
        insert_dir(&db, "new", &[("other.txt", F2)]).await;
        insert_lock(&db, "ghost.txt", 2).await;
        let repos = test_repos(&db);

        let result = check_commit_file_locks(&repos, "test-repo", Some("base"), "new", 1).await;
        assert!(result.is_ok());
    }

    /// Editing a file another user holds is still rejected, including the
    /// legacy `/`-prefixed row shape.
    #[tokio::test]
    async fn test_edit_of_a_foreign_locked_file_is_rejected() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "base", &[("locked.txt", F1)]).await;
        insert_dir(&db, "new", &[("locked.txt", F2)]).await;
        insert_lock(&db, "/locked.txt", 2).await;
        let repos = test_repos(&db);

        match check_commit_file_locks(&repos, "test-repo", Some("base"), "new", 1).await {
            Err(AppError::Locked(path)) => assert_eq!(path, "locked.txt"),
            other => panic!("expected AppError::Locked, got {other:?}"),
        }
        assert!(
            check_commit_file_locks(&repos, "test-repo", Some("base"), "new", 2)
                .await
                .is_ok(),
            "the lock owner may edit the file"
        );
    }

    /// Deleting a locked file is a change to it and must be rejected too.
    #[tokio::test]
    async fn test_delete_of_a_foreign_locked_file_is_rejected() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "base", &[("locked.txt", F1), ("keep.txt", F1)]).await;
        insert_dir(&db, "new", &[("keep.txt", F1)]).await;
        insert_lock(&db, "locked.txt", 2).await;
        let repos = test_repos(&db);

        assert!(matches!(
            check_commit_file_locks(&repos, "test-repo", Some("base"), "new", 1).await,
            Err(AppError::Locked(path)) if path == "locked.txt"
        ));
    }

    /// Renaming a locked file changes the locked path; the diff reports both
    /// the new name and the old one, so the lock still applies.
    #[tokio::test]
    async fn test_rename_of_a_foreign_locked_file_is_rejected() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "base", &[("locked.txt", F1)]).await;
        insert_dir(&db, "new", &[("renamed.txt", F1)]).await;
        insert_lock(&db, "locked.txt", 2).await;
        let repos = test_repos(&db);

        assert!(matches!(
            check_commit_file_locks(&repos, "test-repo", Some("base"), "new", 1).await,
            Err(AppError::Locked(path)) if path == "locked.txt"
        ));
    }

    /// Locks on paths missing from HEAD are swept; locks on live paths stay.
    #[tokio::test]
    async fn test_prune_stale_file_locks() {
        let db = setup_lock_test_db().await;
        insert_dir(&db, "root", &[("kept.txt", F1)]).await;
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            INSERT INTO commits (repo_id, commit_id, root_id, parent_id, creator_name, description, ctime, version)
            VALUES ('test-repo', 'cccccccccccccccccccccccccccccccccccccccc', 'root', NULL, 'u1', '', 1000, 1)
            ",
        ))
        .await
        .unwrap();
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            UPDATE repos SET head_commit_id = 'cccccccccccccccccccccccccccccccccccccccc'
            WHERE id = 'test-repo'
            ",
        ))
        .await
        .unwrap();
        insert_lock(&db, "kept.txt", 1).await;
        insert_lock(&db, "gone.txt", 1).await;
        insert_lock(&db, "/gone.txt", 2).await;
        let repos = test_repos(&db);

        let removed = prune_stale_file_locks(&repos, &db, "test-repo")
            .await
            .unwrap();
        assert_eq!(removed, 2, "both rows for the deleted path must go");
        let remaining = repos.locked_file.find_by_repo("test-repo").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, "kept.txt");
    }

    /// The REST surface sends `/dir/file`, the sync surface and the commit
    /// walk use `dir/file`; both must resolve to the same lock key.
    #[test]
    fn test_canonical_lock_path() {
        assert_eq!(
            canonical_lock_path("/dir/file.txt").unwrap(),
            "dir/file.txt"
        );
        assert_eq!(canonical_lock_path("dir/file.txt").unwrap(), "dir/file.txt");
        assert_eq!(canonical_lock_path("//file.txt").unwrap(), "file.txt");

        // The library root cannot be locked, and traversal is rejected.
        assert!(canonical_lock_path("/").is_err());
        assert!(canonical_lock_path("").is_err());
        assert!(canonical_lock_path("../etc/passwd").is_err());
        assert!(canonical_lock_path("/dir/../file.txt").is_err());
        assert!(canonical_lock_path("/dir//file.txt").is_err());
    }
}
