//! The legacy `webdav_keys` table is moved into the unified key store.
//!
//! The move is what lets existing WebDAV clients keep the secret they already
//! hold: the digest is copied verbatim rather than recomputed, and each key
//! becomes a `webdav.*`-capable row bound to its library, so a migrated key
//! still cannot reach `/api2`.

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};

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
    let db_path = dir.path().join("migration.db");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", db_path.display()).as_str())
        .await
        .expect("connect");

    // Stop immediately before the move.
    let total = Migrator::migrations().len();
    Migrator::up(&db, Some((total - 1) as u32))
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

    // Run the move.
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

    // Each key keeps its library and the same ceiling.
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

    // The legacy table is gone; the sync-token table is untouched.
    assert!(!table_exists(&db, "webdav_keys").await);
    assert!(
        table_exists(&db, "sync_tokens").await,
        "the sync-token table is not part of this move"
    );

    // Re-running must be a no-op rather than a failure.
    Migrator::up(&db, None).await.expect("idempotent");

    let _ = db.close().await;
}
