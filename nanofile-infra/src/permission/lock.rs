use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};

use crate::common::EMPTY_SHA1;
use crate::entity::{file_lock_timestamp, fs_object, locked_file};
use crate::serialization::fs_json::FsDirData;
use nanofile_domain::AppError;

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
