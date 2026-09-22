//! `email_settings` is folded into the generic `settings` table.
//!
//! The single typed row becomes one row per key, and its `password_enc` becomes
//! `email.password` verbatim: the value is already ciphertext under the same AEAD
//! key, so no secret is re-encrypted (or, worse, written in the clear) and the
//! administrator never has to retype it.

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};

/// The migration under test.
const MOVE: &str = "m20260922_000001_create_settings";

/// How many migrations run before `name`, so adding a later one cannot silently
/// turn this into "after the move".
fn steps_before(name: &str) -> u32 {
    Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == name)
        .expect("the named migration must exist") as u32
}

async fn value(db: &DatabaseConnection, key: &str) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT value FROM settings WHERE key = ?",
        [key.into()],
    ))
    .await
    .expect("query")
    .map(|row| row.try_get("", "value").expect("value column"))
}

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
async fn the_email_settings_row_becomes_settings_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("migration.db");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", db_path.display()).as_str())
        .await
        .expect("connect");

    Migrator::up(&db, Some(steps_before(MOVE)))
        .await
        .expect("migrate to the pre-move schema");
    assert!(
        table_exists(&db, "email_settings").await,
        "the typed row must exist before the move"
    );

    db.execute_unprepared(
        "INSERT INTO email_settings \
         (id, paused, host, port, tls, username, password_enc, from_address, from_name, \
          timeout_secs, max_attempts, notify_new_device, notify_api_key_created, \
          notify_new_login, updated_at, updated_by) \
         VALUES (1, 0, 'smtp.example.com', 2525, 'tls', 'bot', 'enc1:ciphertext', \
                 'nanofile@example.com', 'Nanofile', 7, 3, 0, 1, 1, 1700000000, 9)",
    )
    .await
    .expect("seed the settings row");

    Migrator::up(&db, None).await.expect("run the move");

    assert!(!table_exists(&db, "email_settings").await);
    assert_eq!(
        value(&db, "email.host").await.as_deref(),
        Some("smtp.example.com")
    );
    assert_eq!(value(&db, "email.port").await.as_deref(), Some("2525"));
    assert_eq!(value(&db, "email.tls").await.as_deref(), Some("tls"));
    assert_eq!(value(&db, "email.username").await.as_deref(), Some("bot"));
    assert_eq!(value(&db, "email.timeout_secs").await.as_deref(), Some("7"));
    assert_eq!(value(&db, "email.max_attempts").await.as_deref(), Some("3"));
    // Booleans become the catalog's canonical spelling.
    assert_eq!(value(&db, "email.paused").await.as_deref(), Some("false"));
    assert_eq!(
        value(&db, "email.notify_new_device").await.as_deref(),
        Some("false")
    );
    assert_eq!(
        value(&db, "email.notify_api_key_created").await.as_deref(),
        Some("true")
    );
    assert_eq!(
        value(&db, "email.notify_new_login").await.as_deref(),
        Some("true")
    );
    // The ciphertext is copied exactly: same format, same AEAD key, and the
    // administrator never has to enter the password again.
    assert_eq!(
        value(&db, "email.password").await.as_deref(),
        Some("enc1:ciphertext")
    );
    // The audit columns travel with every row.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) AS n FROM settings WHERE updated_at = 1700000000 AND updated_by = 9"
        )
        .await,
        13,
        "every copied row carries the original audit columns"
    );
}

#[tokio::test]
async fn a_database_without_an_email_row_just_gets_the_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("migration.db");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", db_path.display()).as_str())
        .await
        .expect("connect");

    Migrator::up(&db, None).await.expect("migrate everything");

    assert!(table_exists(&db, "settings").await);
    // A fresh install never saved an email row, so nothing is invented for it:
    // the config file keeps supplying every email setting.
    assert_eq!(count(&db, "SELECT COUNT(*) AS n FROM settings").await, 0);
}
