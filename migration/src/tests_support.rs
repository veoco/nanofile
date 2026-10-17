//! What every test in this crate needs to open a database.
//!
//! Tests build the schema by running the whole chain, and SQLite's default rollback
//! journal with `synchronous=FULL` makes each statement fsync repeatedly (several
//! times slower on Windows). Nothing here asserts durability — the database lives for
//! one test — so the journal is in memory and synchronous is off, without changing a
//! statement the migrations run. Production is configured in `infra::db`.

use sea_orm_migration::sea_orm::{ConnectOptions, Database, DatabaseConnection};
use std::path::Path;

/// A URL for a fresh database file inside `directory`.
pub fn file_uri(directory: &Path, name: &str) -> String {
    format!("sqlite://{}?mode=rwc", directory.join(name).display())
}

/// Open the database at `url`, with the syncs taken out of the path.
pub async fn connect(url: &str) -> DatabaseConnection {
    let mut options = ConnectOptions::new(url);
    options.sqlx_logging(false).map_sqlx_sqlite_opts(|options| {
        use sea_orm_migration::sea_orm::sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
        options
            .journal_mode(SqliteJournalMode::Memory)
            .synchronous(SqliteSynchronous::Off)
    });
    Database::connect(options).await.expect("connect sqlite")
}
