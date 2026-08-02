use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

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
    let db = Database::connect(&config.url).await?;

    // Restrict the SQLite database file to the owning user so other local
    // users cannot read it (default umask may leave it world-readable).
    #[cfg(unix)]
    if db.get_database_backend() == DatabaseBackend::Sqlite {
        use std::os::unix::fs::PermissionsExt;
        if let Some(path) = sqlite_file_path(&config.url) {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }

    // ── SQLite performance PRAGMAs ──────────────────────────────────
    // These are essential for concurrent read/write throughput.
    // Without them SQLite defaults to journal_mode=DELETE which
    // serializes ALL write operations and blocks readers.
    if db.get_database_backend() == DatabaseBackend::Sqlite {
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            PRAGMA journal_mode = WAL;           -- concurrent readers + writer
            PRAGMA synchronous   = NORMAL;       -- safe on modern SSD, ~10x faster than FULL
            PRAGMA cache_size    = -8000;         -- 8 MiB page cache
            PRAGMA busy_timeout  = 5000;          -- wait 5 s instead of SQLITE_BUSY
            PRAGMA temp_store    = MEMORY;        -- temp tables / indexes in RAM
            PRAGMA mmap_size     = 268435456;     -- 256 MiB memory-mapped I/O
            "
            .to_owned(),
        ))
        .await?;
    }

    Ok(db)
}
