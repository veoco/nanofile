//! `api_tokens.source` is backfilled from the only hint the old schema had.
//!
//! Before this column, a token's origin had to be guessed from `platform`,
//! which is set only when a client reported device details. The backfill is
//! therefore an approximation, and the test pins which way it errs: rows that
//! look like client logins become `client`, everything else stays `web`. A
//! session filed under the wrong label is still visible and revocable, which is
//! the direction that matters.

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};

/// The migration under test.
const ADD_SOURCE: &str = "m20260913_000001_add_source_and_user_agent_to_api_tokens";

/// How many migrations run before `name`: the `steps` argument that leaves the
/// database in the state immediately preceding it.
fn steps_before(name: &str) -> u32 {
    Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == name)
        .expect("the named migration must exist") as u32
}

async fn source_of(db: &DatabaseConnection, id: i32) -> String {
    db.query_one_raw(Statement::from_string(
        DbBackend::Sqlite,
        format!("SELECT source FROM api_tokens WHERE id = {id}"),
    ))
    .await
    .expect("query")
    .expect("one row")
    .try_get("", "source")
    .expect("source column")
}

#[tokio::test]
async fn existing_tokens_are_classified_from_platform() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("migration.db");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", db_path.display()).as_str())
        .await
        .expect("connect");

    // Stop immediately before the column is added. Looked up by name so a
    // later migration cannot silently move this starting point.
    Migrator::up(&db, Some(steps_before(ADD_SOURCE)))
        .await
        .expect("migrate to the pre-change schema");

    db.execute_unprepared(
        "INSERT INTO users (id, email, password_hash, created_at) \
         VALUES (1, 'owner@example.com', 'x', 1)",
    )
    .await
    .expect("seed user");

    // 1: a client that reported its device. 2: a browser session. 3: a client
    // that sent no device details — indistinguishable from a browser session
    // before this change, and still filed as one afterwards.
    db.execute_unprepared(&format!(
        "INSERT INTO api_tokens \
           (id, user_id, token, created_at, platform, device_id, device_name, is_pending) \
         VALUES (1, 1, '{}', 100, 'linux', 'ccnet-1', 'laptop', 0), \
                (2, 1, '{}', 101, NULL, NULL, NULL, 0), \
                (3, 1, '{}', 102, NULL, NULL, NULL, 0)",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64),
    ))
    .await
    .expect("seed tokens");

    Migrator::up(&db, None).await.expect("apply the change");

    assert_eq!(source_of(&db, 1).await, "client", "platform means a client");
    assert_eq!(
        source_of(&db, 2).await,
        "web",
        "no platform means a browser"
    );
    assert_eq!(
        source_of(&db, 3).await,
        "web",
        "the backfill cannot recover a client that sent no device info"
    );

    // The new column is nullable and starts empty: nothing recorded a user
    // agent before this release, and a made-up one would be worse than none.
    let agents: i64 = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM api_tokens WHERE user_agent IS NOT NULL".to_string(),
        ))
        .await
        .expect("query")
        .expect("one row")
        .try_get("", "n")
        .expect("count");
    assert_eq!(agents, 0);
}
