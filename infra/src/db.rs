use std::path::Path;

use anyhow::Context as _;
use sea_orm::{ConnectOptions, Database, DatabaseBackend, DatabaseConnection};

use crate::config::DatabaseConfig;

/// File name of the SQLite database in `url` (see [`crate::config::sqlite_url_file`]).
fn sqlite_file_path(url: &str) -> Option<&str> {
    crate::config::sqlite_url_file(url).map(|(file, _)| file)
}

/// Create the SQLite parent directory if it does not exist yet.
///
/// SQLite creates the database file but never its parent, so the shipped
/// default (`sqlite:data/nanofile.db?mode=rwc`) fails in an installation with
/// no `data/` directory. An existing directory keeps its mode: the database may
/// deliberately live in a shared location, and tightening it would lock other
/// services out.
fn create_sqlite_parent_dir(url: &str) {
    let Some(parent) = sqlite_file_path(url)
        .map(Path::new)
        .and_then(Path::parent)
        .filter(|dir| !dir.as_os_str().is_empty())
    else {
        return;
    };
    if let Err(e) = crate::common::util::ensure_private_dir_only_if_created(parent) {
        // Not fatal: the connection attempt produces the real error.
        tracing::warn!(
            dir = %parent.display(),
            "could not create the database directory: {e}"
        );
    }
}

/// Restrict the SQLite file and its `-wal` / `-shm` sidecars to the owner.
///
/// The sidecars are created lazily on the first write, after a permission
/// change that only touched the main file, so all three are restricted again
/// afterwards. The WAL can contain password hashes, session-token hashes and
/// plaintext share tokens, so none may be group- or world-readable.
#[cfg(unix)]
fn restrict_sqlite_files(url: &str) {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = sqlite_file_path(url) else {
        return;
    };
    if !std::path::Path::new(path).exists() {
        // Best effort: SQLite reports the real error on connect.
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
    // Before connecting, so the database file and sidecars inherit a private mode
    // and their directory exists.
    create_sqlite_parent_dir(&config.url);
    restrict_sqlite_files(&config.url);

    // `map_sqlx_sqlite_opts` applies these PRAGMAs to every pool connection:
    // `journal_mode`/`synchronous` persist, but `busy_timeout`, `cache_size`,
    // `temp_store` and `mmap_size` are per-connection.
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
    // Name the file that could not be opened: SQLite's code 14 says nothing about
    // which path was tried. The configured URL is never printed — it can carry
    // credentials for a non-SQLite backend.
    let db =
        Database::connect(opts)
            .await
            .with_context(|| match sqlite_file_path(&config.url) {
                Some(file) => format!(
                    "failed to open the SQLite database {}",
                    Path::new(file).display()
                ),
                None => "failed to connect to the database".to_string(),
            })?;

    // The sidecars may have been created by the connection above.
    if db.get_database_backend() == DatabaseBackend::Sqlite {
        restrict_sqlite_files(&config.url);
    }

    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(url: String) -> DatabaseConfig {
        DatabaseConfig {
            url,
            max_connections: 1,
        }
    }

    #[test]
    fn recognizes_only_urls_that_name_a_database_file() {
        for (url, expected) in [
            ("sqlite:data/nanofile.db?mode=rwc", Some("data/nanofile.db")),
            (
                "sqlite://data/nanofile.db?mode=rwc",
                Some("data/nanofile.db"),
            ),
            ("sqlite:/var/lib/nanofile.db", Some("/var/lib/nanofile.db")),
            (
                "sqlite:///var/lib/nanofile.db",
                Some("/var/lib/nanofile.db"),
            ),
            ("sqlite::memory:", None),
            ("sqlite://?mode=memory", None),
            ("sqlite:file::memory:?cache=shared", None),
            ("postgres://user@localhost/nanofile", None),
        ] {
            assert_eq!(sqlite_file_path(url), expected, "url {url}");
        }
    }

    /// Regression: SQLite never creates the directory above the database file, so
    /// the shipped relative default failed in a fresh install without `data/`.
    #[tokio::test]
    async fn connects_to_a_database_in_a_directory_that_does_not_exist_yet() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data").join("nanofile.db");
        let url = format!("sqlite:{}?mode=rwc", db_path.display());

        let db = establish_connection(&config_with(url))
            .await
            .expect("a missing database directory must be created, not reported as an error");

        assert!(db_path.is_file(), "the database file must exist");
        db.close().await.unwrap();
    }

    /// The error must name the file: SQLite's code 14 leaves the operator guessing
    /// which path a relative configuration resolved to.
    #[tokio::test]
    async fn reports_which_database_file_could_not_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        // A directory can never be opened as a database file.
        let url = format!("sqlite:{}?mode=ro", dir.path().display());

        let error = establish_connection(&config_with(url)).await.unwrap_err();

        let message = format!("{error:#}");
        assert!(
            message.contains("failed to open the SQLite database"),
            "{message}"
        );
        assert!(
            message.contains(&dir.path().display().to_string()),
            "the error must name the file: {message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn creates_the_missing_database_directory_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        create_sqlite_parent_dir(&format!(
            "sqlite:{}?mode=rwc",
            data.join("nanofile.db").display()
        ));

        assert!(data.is_dir());
        let mode = std::fs::metadata(&data).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a created state directory must be owner-only");
    }

    /// Creating the parent must not tighten a directory someone else owns.
    #[cfg(unix)]
    #[test]
    fn leaves_an_existing_directory_mode_alone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();

        create_sqlite_parent_dir(&format!("sqlite:{}/nanofile.db", shared.display()));

        let mode = std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }
}
