//! What every test in this crate needs to open a database.
//!
//! The tests build a schema by running the whole migration chain, which is a few
//! hundred statements, and SQLite makes every one of them durable on its own when
//! the connection is left at its defaults: `sea-orm-migration` wraps a migration
//! in a transaction only on Postgres, sqlx sets neither `journal_mode` nor
//! `synchronous`, and SQLite's defaults are a rollback journal with
//! `synchronous=FULL`. Each statement then costs four `fsync`s — the journal
//! twice, the directory, the database — which is milliseconds on Linux and tens
//! of milliseconds on Windows, where the same suite measured ~25× slower.
//!
//! Nothing in these tests is about durability: a database that only exists for
//! the length of a test cannot survive a crash anyway, and none of them asserts
//! on-disk file behaviour. So the journal is kept in memory and the synchronous
//! setting is off, which removes the syncs without changing a single statement
//! the migrations run. The production connection is configured separately (see
//! `infra::db`), where durability is the point and the chain is not re-run.

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
