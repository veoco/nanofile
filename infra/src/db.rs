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

/// Restrict the SQLite database file and its `-wal` / `-shm` sidecars to the
/// owning user.
///
/// The main file is created first (with `0600`) so SQLite derives the sidecar
/// modes from it, and all three are restricted again afterwards because the
/// sidecars are created lazily on the first write — after any permission change
/// that only touched the main file. The WAL can contain password hashes,
/// session-token hashes and plaintext share tokens, so none of them may be
/// group- or world-readable.
#[cfg(unix)]
fn restrict_sqlite_files(url: &str) {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = sqlite_file_path(url) else {
        return;
    };
    if !std::path::Path::new(path).exists() {
        // Best effort: if this fails (read-only mount, `mode=ro`) SQLite will
        // report the real error when connecting.
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path);
    }
    for candidate in [
        path.to_string(),
        format!("{path}-wal"),
        format!("{path}-shm"),
    ] {
        let _ = std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(not(unix))]
fn restrict_sqlite_files(_url: &str) {}

pub async fn establish_connection(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    // Before connecting, so the `-wal` / `-shm` files SQLite creates inherit a
    // private mode rather than the process umask.
    restrict_sqlite_files(&config.url);

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

    // Restrict the database files again: the sidecars may have been created by
    // the connection above.
    if db.get_database_backend() == DatabaseBackend::Sqlite {
        restrict_sqlite_files(&config.url);
    }

    Ok(db)
}
