use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

use crate::config::DatabaseConfig;

pub async fn establish_connection(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    let db = Database::connect(&config.url).await?;

    // ── SQLite performance PRAGMAs ──────────────────────────────────
    // These are essential for concurrent read/write throughput.
    // Without them SQLite defaults to journal_mode=DELETE which
    // serializes ALL write operations and blocks readers.

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

    Ok(db)
}
