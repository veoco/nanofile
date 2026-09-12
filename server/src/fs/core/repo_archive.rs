//! Archive of a deleted library's content while it sits in the trash.
//!
//! Deleting a library removes its `repos` row, and `commits` /
//! `fs_objects` cascade with it. The block files survive on disk — garbage
//! collection deliberately keeps the block directory of a library that is still
//! listed in `deleted_repos` — but without the commit graph and the FS objects
//! nothing can enumerate them, so restoring a library used to rebuild an empty
//! one.
//!
//! [`copy_to`] copies that content into `deleted_repo_commits` /
//! `deleted_repo_fs_objects`, [`copy_from`] puts it back when the library is
//! restored, and [`drop_archive`] discards it when the trash entry is purged.
//! `copy_to` does **not** delete the live rows: the caller runs it inside the
//! same transaction as the delete, so the archive and the library cannot
//! diverge. `copy_from` likewise keeps the archive rows, which makes a
//! restored-then-crashed library resumable; the caller drops them once the
//! restore has fully succeeded.
//!
//! This mirrors the official server, which renames a deleted library's
//! `storage/commits/<repo_id>` and `storage/fs/<repo_id>` into
//! `seafile-data/deleted_store/` and keeps the library's blocks until its trash
//! entry is cleared.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};

use base::error::AppError;

/// Archived columns, in the order the `INSERT ... SELECT` statements use. They
/// mirror the `commit` and `fs_object` entities.
const COMMIT_COLUMNS: &str = "id, repo_id, commit_id, root_id, parent_id, second_parent_id, \
     creator_name, creator, description, ctime, version";
const FS_OBJECT_COLUMNS: &str = "id, repo_id, fs_id, obj_type, data";

/// Build the copy statement for one table.
///
/// Table names and column lists are compile-time constants; only the library id
/// is bound as a parameter.
fn copy_sql(from: &str, to: &str, columns: &str) -> String {
    format!(
        "INSERT OR REPLACE INTO {to} ({columns}) SELECT {columns} FROM {from} WHERE repo_id = $1"
    )
}

fn delete_sql(table: &str) -> String {
    format!("DELETE FROM {table} WHERE repo_id = $1")
}

async fn run<C: ConnectionTrait>(conn: &C, sql: String, repo_id: &str) -> Result<u64, AppError> {
    let result = conn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            sql,
            vec![repo_id.to_owned().into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// Copy the library's commits and FS objects into the archive.
///
/// Returns the number of rows copied. The library's previous archive, if any, is
/// discarded first: a library that is deleted again after being restored must not
/// keep archived objects that its current content no longer has, or restoring it
/// a second time would resurrect pruned commits.
pub async fn copy_to<C: ConnectionTrait>(conn: &C, repo_id: &str) -> Result<u64, AppError> {
    drop_archive(conn, repo_id).await?;
    let mut copied = run(
        conn,
        copy_sql("commits", "deleted_repo_commits", COMMIT_COLUMNS),
        repo_id,
    )
    .await?;
    copied += run(
        conn,
        copy_sql("fs_objects", "deleted_repo_fs_objects", FS_OBJECT_COLUMNS),
        repo_id,
    )
    .await?;
    Ok(copied)
}

/// Copy the archived commits and FS objects back into the live tables.
///
/// Returns the number of rows restored. Idempotent, and it leaves the archive
/// in place so a failed restore can be retried.
pub async fn copy_from<C: ConnectionTrait>(conn: &C, repo_id: &str) -> Result<u64, AppError> {
    let mut restored = run(
        conn,
        copy_sql("deleted_repo_commits", "commits", COMMIT_COLUMNS),
        repo_id,
    )
    .await?;
    restored += run(
        conn,
        copy_sql("deleted_repo_fs_objects", "fs_objects", FS_OBJECT_COLUMNS),
        repo_id,
    )
    .await?;
    Ok(restored)
}

/// Discard the archived content of a library that is being purged for good.
pub async fn drop_archive<C: ConnectionTrait>(conn: &C, repo_id: &str) -> Result<u64, AppError> {
    let mut removed = run(conn, delete_sql("deleted_repo_commits"), repo_id).await?;
    removed += run(conn, delete_sql("deleted_repo_fs_objects"), repo_id).await?;
    Ok(removed)
}

/// Archive a library's content and delete the library's rows, in one
/// transaction.
///
/// This is the delete half of the trash cycle. Copying the content and removing
/// the rows together means a crash can never leave a library deleted with its
/// content unarchived — which would strand its blocks with nothing to enumerate
/// them.
///
/// The rows are deleted here rather than through the repositories so that the
/// whole move is a single transaction; the caller only has to flush the cached
/// sync tokens afterwards. Deleting the `repos` row also cascades into the
/// library's shares, upload links, tags, metadata and WebDAV keys: a trashed
/// library is reachable by none of them, and only the content is archived.
pub async fn archive_and_delete(db: &DatabaseConnection, repo_id: &str) -> Result<u64, AppError> {
    let txn = db.begin().await?;
    let archived = copy_to(&txn, repo_id).await?;
    for sql in [
        "DELETE FROM commits WHERE repo_id = $1",
        "DELETE FROM fs_objects WHERE repo_id = $1",
        "DELETE FROM repo_members WHERE repo_id = $1",
        "DELETE FROM sync_tokens WHERE repo_id = $1",
        "DELETE FROM repos WHERE id = $1",
    ] {
        run(&txn, sql.to_owned(), repo_id).await?;
    }
    txn.commit().await?;
    Ok(archived)
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::MigratorTrait;
    use sea_orm::{Database, DatabaseConnection, TryGetable};

    /// A migrated in-memory database with one user and one library.
    async fn setup() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        for sql in [
            "INSERT INTO users (id, email, password_hash, created_at) \
             VALUES (1, 'owner@example.com', 'x', 0)",
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
             permission, created_at, updated_at, size, repo_version, history_limit, \
             history_ttl_days) \
             VALUES ('repo-1', 'library', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, 0, 0)",
            "INSERT INTO commits (repo_id, commit_id, root_id, creator_name, ctime, version) \
             VALUES ('repo-1', 'c1', 'd1', 'owner@example.com', 11, 1)",
            "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) \
             VALUES ('repo-1', 'd1', 3, '{\"dirents\":[]}')",
            "INSERT INTO repo_members (repo_id, user_id, permission, created_at) \
             VALUES ('repo-1', 1, 'rw', 0)",
            "INSERT INTO sync_tokens (token, repo_id, user_id, created_at) \
             VALUES ('token-1', 'repo-1', 1, 0)",
        ] {
            db.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, sql))
                .await
                .unwrap();
        }
        db
    }

    async fn count(db: &DatabaseConnection, table: &str) -> i64 {
        db.query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!("SELECT COUNT(*) FROM {table}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap()
    }

    /// Simulate the cascade: the library's live rows disappear, and the archive
    /// brings them back unchanged.
    #[tokio::test]
    async fn archived_content_survives_the_delete_and_comes_back() {
        let db = setup().await;

        assert_eq!(copy_to(&db, "repo-1").await.unwrap(), 2);
        // The copy must not touch the live rows: the caller deletes them in the
        // same transaction.
        assert_eq!(count(&db, "commits").await, 1);
        assert_eq!(count(&db, "fs_objects").await, 1);
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 1);

        // Re-archiving is idempotent (a retried delete).
        assert_eq!(copy_to(&db, "repo-1").await.unwrap(), 2);
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 1);

        for table in ["commits", "fs_objects"] {
            db.execute_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("DELETE FROM {table} WHERE repo_id = 'repo-1'"),
            ))
            .await
            .unwrap();
        }

        assert_eq!(copy_from(&db, "repo-1").await.unwrap(), 2);
        assert_eq!(count(&db, "commits").await, 1);
        assert_eq!(count(&db, "fs_objects").await, 1);
        // Restoring keeps the archive until the caller drops it.
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 1);

        // A retried restore must not duplicate anything.
        assert_eq!(copy_from(&db, "repo-1").await.unwrap(), 2);
        assert_eq!(count(&db, "commits").await, 1);

        let commit = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT commit_id, root_id, ctime FROM commits WHERE repo_id = 'repo-1'",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(String::try_get_by_index(&commit, 0).unwrap(), "c1");
        assert_eq!(String::try_get_by_index(&commit, 1).unwrap(), "d1");
        assert_eq!(i64::try_get_by_index(&commit, 2).unwrap(), 11);

        assert_eq!(drop_archive(&db, "repo-1").await.unwrap(), 2);
        assert_eq!(count(&db, "deleted_repo_commits").await, 0);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 0);
        assert_eq!(count(&db, "commits").await, 1);
    }

    #[tokio::test]
    async fn empty_library_copies_nothing() {
        let db = setup().await;
        for table in ["commits", "fs_objects"] {
            db.execute_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("DELETE FROM {table} WHERE repo_id = 'repo-1'"),
            ))
            .await
            .unwrap();
        }

        assert_eq!(copy_to(&db, "repo-1").await.unwrap(), 0);
        assert_eq!(copy_from(&db, "repo-1").await.unwrap(), 0);
        assert_eq!(drop_archive(&db, "repo-1").await.unwrap(), 0);
    }

    /// A library that is deleted, restored and deleted again must not keep an
    /// archived object its current content no longer has: restoring it a second
    /// time would otherwise resurrect a pruned commit whose blocks are gone.
    #[tokio::test]
    async fn re_archiving_drops_objects_the_library_no_longer_has() {
        let db = setup().await;
        assert_eq!(copy_to(&db, "repo-1").await.unwrap(), 2);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 1);

        // The restored library lost its FS object (e.g. history pruning) and is
        // deleted again.
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "DELETE FROM fs_objects WHERE repo_id = 'repo-1'",
        ))
        .await
        .unwrap();

        assert_eq!(copy_to(&db, "repo-1").await.unwrap(), 1);
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 0);
    }

    /// A library never deleted has no archive, and other libraries' archives are
    /// never touched.
    #[tokio::test]
    async fn archive_is_scoped_to_one_library() {
        let db = setup().await;
        for sql in [
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
             permission, created_at, updated_at, size, repo_version, history_limit, \
             history_ttl_days) \
             VALUES ('repo-2', 'other', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, 0, 0)",
            "INSERT INTO commits (repo_id, commit_id, root_id, creator_name, ctime, version) \
             VALUES ('repo-2', 'c2', 'd2', 'owner@example.com', 12, 1)",
        ] {
            db.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, sql))
                .await
                .unwrap();
        }

        assert_eq!(copy_to(&db, "repo-2").await.unwrap(), 1);
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);

        assert_eq!(copy_from(&db, "repo-1").await.unwrap(), 0);
        assert_eq!(drop_archive(&db, "repo-1").await.unwrap(), 0);
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);
    }

    /// The delete path: the content is archived and the library's rows are gone
    /// in the same transaction, so a restore can rebuild that library.
    #[tokio::test]
    async fn archive_and_delete_removes_the_library_and_keeps_its_content() {
        let db = setup().await;

        assert_eq!(archive_and_delete(&db, "repo-1").await.unwrap(), 2);

        // The library is gone, together with everything that pointed at it.
        assert_eq!(count(&db, "repos").await, 0);
        assert_eq!(count(&db, "commits").await, 0);
        assert_eq!(count(&db, "fs_objects").await, 0);
        assert_eq!(count(&db, "repo_members").await, 0);
        assert_eq!(count(&db, "sync_tokens").await, 0);

        // ... and its content is in the archive, ready for a restore.
        assert_eq!(count(&db, "deleted_repo_commits").await, 1);
        assert_eq!(count(&db, "deleted_repo_fs_objects").await, 1);

        // Restoring recreates the library row (as `restore_deleted_repo` does)
        // and copies the content back.
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
             permission, created_at, updated_at, size, repo_version, history_limit, \
             history_ttl_days) \
             VALUES ('repo-1', 'library', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, 0, 0)",
        ))
        .await
        .unwrap();
        assert_eq!(copy_from(&db, "repo-1").await.unwrap(), 2);
        assert_eq!(count(&db, "commits").await, 1);
        assert_eq!(count(&db, "fs_objects").await, 1);
    }
}
