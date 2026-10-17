//! The legacy `webdav_keys` table is moved into the unified key store.
//!
//! The digest is copied verbatim, so existing WebDAV clients keep their secret, and
//! each key becomes a `webdav.*`-capable row bound to its library (still not valid
//! for `/api2`).

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

/// The migration that performs the move under test.
const MOVE: &str = "m20260912_000001_migrate_webdav_keys_into_api_keys";

/// The `steps` argument that leaves the database just before `name`.
fn steps_before(name: &str) -> u32 {
    Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == name)
        .expect("the named migration must exist") as u32
}

const REPO: &str = "11111111-2222-3333-4444-555555555555";
const HASH_RW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_RO: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn count(db: &DatabaseConnection, sql: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
        .await
        .expect("query")
        .expect("one row")
        .try_get("", "n")
        .expect("count column")
}

async fn table_exists(db: &DatabaseConnection, name: &str) -> bool {
    count(
        db,
        &format!("SELECT COUNT(*) AS n FROM sqlite_master WHERE type='table' AND name='{name}'"),
    )
    .await
        > 0
}

#[tokio::test]
async fn legacy_webdav_keys_become_unified_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = migration::tests_support::connect(&migration::tests_support::file_uri(
        dir.path(),
        "migration.db",
    ))
    .await;

    // Stop immediately before the move, looked up by name so a later migration cannot
    // silently turn this into "after the move".
    Migrator::up(&db, Some(steps_before(MOVE)))
        .await
        .expect("migrate to the pre-move schema");
    assert!(
        table_exists(&db, "webdav_keys").await,
        "the legacy table must exist before the move"
    );

    db.execute_unprepared(
        "INSERT INTO users (id, email, password_hash, created_at) \
         VALUES (1, 'owner@example.com', 'x', 1)",
    )
    .await
    .expect("seed user");
    db.execute_unprepared(&format!(
        "INSERT INTO repos (id, name, owner_id, created_at, updated_at) \
         VALUES ('{REPO}', 'library', 1, 1, 1)"
    ))
    .await
    .expect("seed repo");
    db.execute_unprepared(&format!(
        "INSERT INTO webdav_keys (repo_id, user_id, name, permission, key_hash, created_at, last_used_at) \
         VALUES ('{REPO}', 1, 'rclone', 'rw', '{HASH_RW}', 100, 200), \
                ('{REPO}', 1, 'viewer', 'r', '{HASH_RO}', 101, NULL)"
    ))
    .await
    .expect("seed legacy keys");

    Migrator::up(&db, None).await.expect("apply the move");

    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM api_keys").await,
        2,
        "both legacy keys must be migrated"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM api_keys WHERE key_hash IS NOT NULL"
        )
        .await,
        2,
        "the digest is copied, not recomputed"
    );

    let rw = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!(
                "SELECT capabilities, all_repos, key_prefix, expires_at \
                 FROM api_keys WHERE key_hash = '{HASH_RW}'"
            ),
        ))
        .await
        .expect("query")
        .expect("rw row");
    assert_eq!(
        rw.try_get::<String>("", "capabilities").unwrap(),
        "webdav.read,webdav.write"
    );
    assert!(!rw.try_get::<bool>("", "all_repos").unwrap());
    assert!(
        rw.try_get::<Option<String>>("", "key_prefix")
            .unwrap()
            .is_none(),
        "a hash cannot be reversed into a display prefix"
    );
    assert!(
        rw.try_get::<Option<i64>>("", "expires_at")
            .unwrap()
            .is_none(),
        "legacy keys never expired"
    );

    let ro = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("SELECT capabilities FROM api_keys WHERE key_hash = '{HASH_RO}'"),
        ))
        .await
        .expect("query")
        .expect("ro row");
    assert_eq!(
        ro.try_get::<String>("", "capabilities").unwrap(),
        "webdav.read",
        "a read-only key must not gain write access"
    );

    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) AS n FROM api_key_repos \
                 WHERE repo_id = '{REPO}' AND permission = 'rw'"
            )
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) AS n FROM api_key_repos \
                 WHERE repo_id = '{REPO}' AND permission = 'r'"
            )
        )
        .await,
        1
    );

    assert!(!table_exists(&db, "webdav_keys").await);
    assert!(
        table_exists(&db, "sync_tokens").await,
        "the sync-token table is not part of this move"
    );

    // Re-running must be a no-op rather than a failure.
    Migrator::up(&db, None).await.expect("idempotent");

    let _ = db.close().await;
}

#[tokio::test]
async fn orphaned_webdav_keys_are_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = migration::tests_support::connect(&migration::tests_support::file_uri(
        dir.path(),
        "migration.db",
    ))
    .await;

    Migrator::up(&db, Some(steps_before(MOVE)))
        .await
        .expect("migrate to the pre-move schema");

    db.execute_unprepared(
        "INSERT INTO users (id, email, password_hash, created_at) \
         VALUES (1, 'owner@example.com', 'x', 1)",
    )
    .await
    .expect("seed user");
    db.execute_unprepared(&format!(
        "INSERT INTO repos (id, name, owner_id, created_at, updated_at) \
         VALUES ('{REPO}', 'library', 1, 1, 1)"
    ))
    .await
    .expect("seed repo");

    // FKs off to insert the orphaned row: SQLite defaults to FKs off, so deleting a repo
    // or user left dangling `webdav_keys` rows in the wild.
    db.execute_unprepared("PRAGMA foreign_keys = OFF")
        .await
        .expect("disable FKs for seeding");

    let ghost_repo = "99999999-9999-9999-9999-999999999999";
    let ghost_hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    db.execute_unprepared(&format!(
        "INSERT INTO webdav_keys (repo_id, user_id, name, permission, key_hash, created_at, last_used_at) \
         VALUES ('{REPO}', 1, 'rclone', 'rw', '{HASH_RW}', 100, 200), \
                ('{REPO}', 1, 'viewer', 'r', '{HASH_RO}', 101, NULL), \
                ('{ghost_repo}', 1, 'orphan', 'rw', '{ghost_hash}', 102, NULL)"
    ))
    .await
    .expect("seed legacy keys");

    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .expect("re-enable FKs for the migration");

    Migrator::up(&db, None).await.expect("apply the move");

    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM api_keys").await,
        2,
        "only keys with a valid user and repo are migrated"
    );
    assert!(!table_exists(&db, "webdav_keys").await);

    let _ = db.close().await;
}

#[tokio::test]
async fn rerun_after_partial_failure_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = migration::tests_support::connect(&migration::tests_support::file_uri(
        dir.path(),
        "migration.db",
    ))
    .await;

    Migrator::up(&db, Some(steps_before(MOVE)))
        .await
        .expect("migrate to the pre-move schema");

    db.execute_unprepared(
        "INSERT INTO users (id, email, password_hash, created_at) \
         VALUES (1, 'owner@example.com', 'x', 1)",
    )
    .await
    .expect("seed user");
    db.execute_unprepared(&format!(
        "INSERT INTO repos (id, name, owner_id, created_at, updated_at) \
         VALUES ('{REPO}', 'library', 1, 1, 1)"
    ))
    .await
    .expect("seed repo");
    db.execute_unprepared(&format!(
        "INSERT INTO webdav_keys (repo_id, user_id, name, permission, key_hash, created_at, last_used_at) \
         VALUES ('{REPO}', 1, 'rclone', 'rw', '{HASH_RW}', 100, 200)"
    ))
    .await
    .expect("seed legacy key");

    // A previous failed run already copied the row into `api_keys` (`key_prefix IS NULL`
    // marks it) but aborted before dropping `webdav_keys`.
    db.execute_unprepared(&format!(
        "INSERT INTO api_keys (user_id, name, key_hash, key_prefix, capabilities, all_repos, created_at, expires_at, last_used_at) \
         VALUES (1, 'rclone', '{HASH_RW}', NULL, 'webdav.read,webdav.write', 0, 100, NULL, 200)"
    ))
    .await
    .expect("seed leftover api_key");
    db.execute_unprepared(&format!(
        "INSERT INTO api_key_repos (key_id, repo_id, permission) \
         SELECT id, '{REPO}', 'rw' FROM api_keys WHERE key_hash = '{HASH_RW}'"
    ))
    .await
    .expect("seed leftover binding");

    Migrator::up(&db, None).await.expect("apply the move");

    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM api_keys").await,
        1,
        "the leftover row is replaced, not duplicated"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM api_key_repos").await,
        1,
        "the leftover binding is replaced, not duplicated"
    );
    assert!(!table_exists(&db, "webdav_keys").await);

    let _ = db.close().await;
}

#[tokio::test]
async fn duplicate_digest_across_libraries_merges_into_one_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = migration::tests_support::connect(&migration::tests_support::file_uri(
        dir.path(),
        "migration.db",
    ))
    .await;

    Migrator::up(&db, Some(steps_before(MOVE)))
        .await
        .expect("migrate to the pre-move schema");

    db.execute_unprepared(
        "INSERT INTO users (id, email, password_hash, created_at) \
         VALUES (1, 'owner@example.com', 'x', 1)",
    )
    .await
    .expect("seed user");

    // Two libraries, same owner and same WebDAV secret: the old unique index was on
    // `(repo_id, user_id, key_hash)`, not `key_hash` alone.
    let repo_a = "11111111-2222-3333-4444-555555555555";
    let repo_b = "22222222-3333-4444-5555-666666666666";
    db.execute_unprepared(&format!(
        "INSERT INTO repos (id, name, owner_id, created_at, updated_at) \
         VALUES ('{repo_a}', 'library-a', 1, 1, 1), \
                ('{repo_b}', 'library-b', 1, 1, 1)"
    ))
    .await
    .expect("seed repos");

    db.execute_unprepared(&format!(
        "INSERT INTO webdav_keys (repo_id, user_id, name, permission, key_hash, created_at, last_used_at) \
         VALUES ('{repo_a}', 1, 'rclone', 'rw', '{HASH_RW}', 100, 200), \
                ('{repo_b}', 1, 'rclone', 'r', '{HASH_RW}', 101, NULL)"
    ))
    .await
    .expect("seed legacy keys");

    Migrator::up(&db, None).await.expect("apply the move");

    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM api_keys").await,
        1,
        "the same digest must produce a single api_key, not two"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) AS n FROM api_key_repos").await,
        2,
        "each legacy library gets its own binding"
    );

    let caps = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("SELECT capabilities FROM api_keys WHERE key_hash = '{HASH_RW}'"),
        ))
        .await
        .expect("query")
        .expect("one row");
    assert_eq!(
        caps.try_get::<String>("", "capabilities").unwrap(),
        "webdav.read,webdav.write",
        "the broader of the two permissions wins"
    );

    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) AS n FROM api_key_repos \
                 WHERE repo_id = '{repo_a}' AND permission = 'rw'"
            )
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) AS n FROM api_key_repos \
                 WHERE repo_id = '{repo_b}' AND permission = 'r'"
            )
        )
        .await,
        1
    );

    let _ = db.close().await;
}
