use sea_orm::{ConnectOptions, Database, DatabaseBackend, DatabaseConnection};

use crate::config::DatabaseConfig;

/// Extract the file path from a `sqlite:` URL such as
/// `sqlite:data/nanofile.db?mode=rwc`. Returns `None` for non-file databases
/// (e.g. `sqlite::memory:`).
#[cfg(unix)]
fn sqlite_file_path(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("sqlite:")?;
    let path = rest.split('?').next()?;
    if path.is_empty() || path == ":memory:" || path.starts_with("file::memory") {
        return None;
    }
    Some(path)
}

pub async fn establish_connection(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    // Build the pool with the per-connection SQLite options attached via
    // `map_sqlx_sqlite_opts` so the PRAGMAs below apply to EVERY connection the
    // pool opens (not just the first). `journal_mode`/`synchronous` are
    // persistent, but `busy_timeout`, `cache_size`, `temp_store` and `mmap_size`
    // are per-connection and must be re-applied on each new connection.
    let mut opts = ConnectOptions::new(config.url.as_str());
    opts.max_connections(config.max_connections)
        .sqlx_logging(false)
        .map_sqlx_sqlite_opts(|o| {
            use sea_orm::sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
            o.journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(std::time::Duration::from_secs(5))
                .pragma("cache_size", "-8000")
                .pragma("temp_store", "MEMORY")
                .pragma("mmap_size", "268435456")
        });
    let db = Database::connect(opts).await?;

    // Restrict the SQLite database file to the owning user so other local
    // users cannot read it (default umask may leave it world-readable).
    #[cfg(unix)]
    if db.get_database_backend() == DatabaseBackend::Sqlite {
        use std::os::unix::fs::PermissionsExt;
        if let Some(path) = sqlite_file_path(&config.url) {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }

    Ok(db)
}
