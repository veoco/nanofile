//! Config migration: in-place upgrade of `config.toml` on product version bumps.
//!
//! `Config::load()` first parses the file with `toml_edit` (preserving all
//! comments), then applies every registered migration whose version falls in
//! `(recorded_version, current_version]`. After a successful deserialize the
//! upgraded document is written back atomically behind a `config.toml.bak`
//! backup. A read-only mount degrades to a warning: the in-memory config is
//! already migrated, so the current run is unaffected.
//!
//! Environment variables (`NANOFILE_*`) are deliberately never written into
//! the file: `apply_env_overrides()` runs after migration and is the highest
//! priority, so anything set via env simply wins at runtime. Persisting env
//! values would corrupt the docker / multi-environment model and leave stale
//! values behind once the env var changes.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Top-level key that records the last product version this config was
/// migrated to. It lives only in the `toml_edit` layer — `Config` ignores it
/// (serde drops unknown keys), so it never surfaces in `Config` fields.
const CONFIG_VERSION_KEY: &str = "config_version";

/// Current product version, from the `infra` crate's Cargo version.
/// The workspace keeps every crate at the same version, so this doubles as
/// the config format anchor.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Signature of a single migration step.
type MigrationFn = fn(&mut toml_edit::DocumentMut) -> anyhow::Result<()>;

/// One registered config migration.
struct ConfigMigration {
    /// Product version this migration was introduced in, e.g. `"0.1.10"`.
    pub version: &'static str,
    /// Short name used in logs, e.g. `"0.1.10-inject-email-defaults"`.
    pub name: &'static str,
    /// The step itself. MUST be idempotent: applying it to a document that is
    /// already at the target format must be a no-op (the `changed` detection
    /// in `migrate_with` relies on this — same-value writes never touch disk).
    pub apply: MigrationFn,
}

/// All registered migrations, ascending by version.
///
/// # Adding a new migration
///
/// When a config structure change actually needs one (only when a new required
/// key without a `#[serde(default)]` is introduced, a key is renamed/moved, or
/// a value's format changes), append a new entry **at the end** (keep ascending
/// order), with `version` set to the product release that ships the change and
/// a guard-based, idempotent `apply`:
///
/// ```rust,ignore
/// ConfigMigration {
///     version: "0.1.10",
///     name: "0.1.10-inject-email-defaults",
///     apply: |doc| {
///         let tbl = doc.entry("email").or_insert(toml_edit::Table::new());
///         if tbl.as_table().is_some_and(|t| !t.contains_key("enabled")) {
///             tbl.as_table_mut()
///                 .expect("entry is a table")
///                 .insert("enabled", toml_edit::value(false));
///         }
///         Ok(())
///     },
/// }
/// ```
///
/// Add a unit test (old config → `migrate_with(..., current = new version)` →
/// assert new key / move / stamp / comments preserved) and, once a real
/// migration exists, a `load_from` end-to-end disk-write test.
const MIGRATIONS: &[ConfigMigration] = &[
    // Placeholder / scaffolding migration: this release has no real config
    // structure changes. It exercises the mechanism end-to-end and gives the
    // tests something concrete to assert against. Replace it with the first
    // real migration.
    ConfigMigration {
        version: "0.1.9",
        name: "scaffold-config-version",
        apply: |_doc| Ok(()),
    },
];

/// Result of running the migration chain over a document.
/// Returned by `migrate_config`; consumed by `Config::load_from`.
pub(crate) struct MigrationReport {
    /// Names of migrations actually executed this run.
    pub applied: Vec<&'static str>,
    /// Whether the document text changed (before stamping). `false` means
    /// nothing is written back to disk.
    pub changed: bool,
    /// Recorded version before this run (verbatim; `"0.0.0"` only when the
    /// key is absent — an unparseable value is kept as-is).
    pub from: String,
    /// Version after this run (`current` when changed, otherwise unchanged).
    pub to: String,
}

/// Parse a version string, degrading to `0.0.0` on failure so a garbage
/// `config_version` self-heals by re-running the full chain.
fn parse_or_zero(s: &str) -> semver::Version {
    match semver::Version::parse(s) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "config.toml {CONFIG_VERSION_KEY} \"{s}\" is not a valid version; \
                 treating it as 0.0.0"
            );
            semver::Version::new(0, 0, 0)
        }
    }
}

/// Read the recorded config version, defaulting to `"0.0.0"` when absent.
fn read_config_version(doc: &toml_edit::DocumentMut) -> String {
    doc.get(CONFIG_VERSION_KEY)
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "0.0.0".to_string())
}

/// Set (or stamp) the config version. First insertion carries a one-time
/// explanation comment; subsequent writes update the value in place and keep
/// any existing decor.
fn set_config_version(doc: &mut toml_edit::DocumentMut, version: &str) {
    if doc.get(CONFIG_VERSION_KEY).is_none() {
        let key = toml_edit::Key::new(CONFIG_VERSION_KEY).with_leaf_decor(toml_edit::Decor::new(
            "# Last product version this config.toml was migrated to.\n\
             # Auto-maintained by nanofile; do not edit by hand.\n",
            // Suffix sits between the key and `=`, so it must keep a space.
            " ",
        ));
        doc.insert_formatted(&key, toml_edit::value(version));
    } else {
        *doc.entry(CONFIG_VERSION_KEY)
            .or_insert(toml_edit::value(version)) = toml_edit::value(version);
    }
}

/// Apply all migrations in `(from_version, current]`. `current` is
/// parameterized so tests can inject arbitrary version windows.
fn migrate_with(
    doc: &mut toml_edit::DocumentMut,
    migrations: &[ConfigMigration],
    current: &str,
) -> anyhow::Result<MigrationReport> {
    let from = read_config_version(doc);
    let from_v = parse_or_zero(&from);
    let current_v = parse_or_zero(current);

    // Downgrade: the file records a newer version than this binary. Don't
    // touch it and don't rewrite; unknown new keys are ignored by serde, so
    // the server still starts.
    if from_v > current_v {
        tracing::warn!(
            "config.toml records {CONFIG_VERSION_KEY} = {from}, newer than this binary ({current}); \
             skipping migration (new keys will be ignored)"
        );
        return Ok(MigrationReport {
            applied: vec![],
            changed: false,
            to: from.clone(),
            from,
        });
    }

    // Already current: no migration can apply (`from < m.version` contradicts
    // `m.version <= current == from`). Short-circuit to skip the document
    // snapshot — this is the hot path every fresh install hits.
    if from_v == current_v {
        return Ok(MigrationReport {
            applied: vec![],
            changed: false,
            to: from.clone(),
            from,
        });
    }

    let before = doc.to_string();
    let mut applied = Vec::new();
    for m in migrations {
        let m_v = parse_or_zero(m.version);
        if from_v < m_v && m_v <= current_v {
            (m.apply)(doc).with_context(|| format!("config migration \"{}\" failed", m.name))?;
            applied.push(m.name);
        }
    }
    // No migration ran ⇒ nothing changed, no need to re-serialize.
    let changed = !applied.is_empty() && doc.to_string() != before;
    let to = if changed {
        current.to_string()
    } else {
        from.clone()
    };
    if changed {
        set_config_version(doc, current);
    }
    Ok(MigrationReport {
        applied,
        changed,
        from,
        to,
    })
}

/// Run the full registered chain against the current product version.
pub(crate) fn migrate_config(doc: &mut toml_edit::DocumentMut) -> anyhow::Result<MigrationReport> {
    migrate_with(doc, MIGRATIONS, CURRENT_VERSION)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(s)
}

/// Atomically replace `path` with `migrated`, keeping the previous content as
/// `path.bak`. Fails without touching `path` when the backup or the write
/// fails. The backup is taken with `fs::copy` so file permissions carry over.
pub(crate) fn persist_with_backup(path: &Path, migrated: &str) -> anyhow::Result<()> {
    std::fs::copy(path, backup_path(path)).with_context(|| {
        format!(
            "failed to back up {} to {}",
            path.display(),
            backup_path(path).display()
        )
    })?;
    atomic_write(path, migrated)
}

/// Write `content` to `path` via a same-directory temp file + rename. Keeps
/// the original file's permissions and fsyncs before renaming so the replaced
/// file is never observed half-written. The temp file is cleaned up on
/// failure.
fn atomic_write(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp = tmp_path(path);
    let result = (|| -> anyhow::Result<()> {
        let perms = std::fs::metadata(path)?.permissions();
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            f.set_permissions(perms)?;
        }
        #[cfg(windows)]
        {
            // `rename` does not overwrite an existing target on Windows.
            let _ = std::fs::remove_file(path);
        }
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(path.parent().unwrap_or_else(|| Path::new("."))) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn parse_doc(s: &str) -> toml_edit::DocumentMut {
        s.parse().expect("valid toml")
    }

    /// Concrete rename migrations. `ConfigMigration.apply` is a fn pointer, so
    /// it cannot capture — each rename gets its own named fn.
    macro_rules! rename_fn {
        ($name:ident, $from:literal, $to:literal) => {
            fn $name(doc: &mut toml_edit::DocumentMut) -> anyhow::Result<()> {
                if let Some(v) = doc.remove($from) {
                    doc.insert($to, v);
                }
                Ok(())
            }
        };
    }
    rename_fn!(rn_old_new, "old", "new");
    rename_fn!(rn_gone_here, "gone", "here");
    rename_fn!(rn_a_a1, "a", "a1");
    rename_fn!(rn_b_b1, "b", "b1");
    rename_fn!(rn_c_c1, "c", "c1");

    fn mig(version: &'static str, apply: MigrationFn) -> ConfigMigration {
        ConfigMigration {
            version,
            name: version,
            apply,
        }
    }

    #[test]
    fn missing_version_runs_all() {
        let migrations = [mig("0.1.9", rn_old_new), mig("0.1.10", rn_gone_here)];
        let mut doc = parse_doc("old = \"a\"\ngone = \"b\"\n");
        let report = migrate_with(&mut doc, &migrations, "0.1.10").unwrap();

        assert_eq!(report.applied, vec!["0.1.9", "0.1.10"]);
        assert!(report.changed);
        assert_eq!(report.from, "0.0.0");
        assert_eq!(report.to, "0.1.10");
        assert_eq!(read_config_version(&doc), "0.1.10");
        let out = doc.to_string();
        assert!(out.contains("new = \"a\""));
        assert!(out.contains("here = \"b\""));
        assert!(!out.contains("old ="));
        assert!(!out.contains("gone ="));
    }

    #[test]
    fn already_current_no_change() {
        let migrations = [mig("0.1.10", rn_old_new)];
        let mut doc = parse_doc("config_version = \"0.1.10\"\nold = \"a\"\n");
        let report = migrate_with(&mut doc, &migrations, "0.1.10").unwrap();

        assert!(report.applied.is_empty());
        assert!(!report.changed);
        assert_eq!(report.from, "0.1.10");
        assert_eq!(report.to, "0.1.10");
        assert_eq!(
            doc.to_string(),
            "config_version = \"0.1.10\"\nold = \"a\"\n"
        );
    }

    #[test]
    fn older_config_applies_pending_only() {
        let migrations = [
            mig("0.1.9", rn_a_a1),
            mig("0.1.10", rn_b_b1),
            mig("0.1.11", rn_c_c1),
        ];
        let mut doc = parse_doc("config_version = \"0.1.9\"\nb = \"b\"\nc = \"c\"\n");
        let report = migrate_with(&mut doc, &migrations, "0.1.11").unwrap();

        assert_eq!(report.applied, vec!["0.1.10", "0.1.11"]);
        assert!(report.changed);
        assert_eq!(read_config_version(&doc), "0.1.11");
        let out = doc.to_string();
        assert!(out.contains("b1 = \"b\"")); // 0.1.10 renamed b -> b1
        assert!(out.contains("c1 = \"c\""));
        assert!(!out.contains("a1")); // 0.1.9 not re-run
        assert!(!out.contains("a = \"a\""));
    }

    #[test]
    fn newer_config_left_alone() {
        let migrations = [mig("0.1.10", rn_old_new)];
        let mut doc = parse_doc("config_version = \"0.1.12\"\nold = \"a\"\n");
        let report = migrate_with(&mut doc, &migrations, "0.1.11").unwrap();

        assert!(report.applied.is_empty());
        assert!(!report.changed);
        assert_eq!(report.from, "0.1.12");
        assert_eq!(report.to, "0.1.12");
        assert_eq!(read_config_version(&doc), "0.1.12");
        assert!(doc.to_string().contains("old = \"a\""));
    }

    #[test]
    fn garbage_version_treated_oldest() {
        let migrations = [mig("0.1.9", rn_old_new)];
        let mut doc = parse_doc("config_version = \"asdf\"\nold = \"a\"\n");
        let report = migrate_with(&mut doc, &migrations, "0.1.9").unwrap();

        assert_eq!(report.applied, vec!["0.1.9"]);
        assert!(report.changed);
        assert_eq!(read_config_version(&doc), "0.1.9");
        assert!(doc.to_string().contains("new = \"a\""));
    }

    #[test]
    fn version_ordering() {
        // Semver handles 0.10 > 0.9, which string comparison would get wrong.
        let cmp = |a: &str, b: &str| parse_or_zero(a).cmp(&parse_or_zero(b));
        assert_eq!(cmp("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(cmp("0.10.0", "1.0.0"), Ordering::Less);
        assert_eq!(cmp("0.1.10", "0.1.10"), Ordering::Equal);
        assert_eq!(cmp("1.0.0", "0.99.0"), Ordering::Greater);
    }

    #[test]
    fn comments_preserved() {
        let migrations = [mig("0.1.9", rn_old_new)];
        // Comments attached to keys the migration doesn't touch must survive.
        // `old` lives in the root table (before `[server]`).
        let original =
            "old = \"x\"\n\n# server address\n[server]\n# listen port\naddr = \"0.0.0.0\"\n";
        let mut doc = parse_doc(original);
        let report = migrate_with(&mut doc, &migrations, "0.1.9").unwrap();
        assert!(report.changed);

        let out = doc.to_string();
        assert!(out.contains("# server address"));
        assert!(out.contains("# listen port"));
        assert!(out.contains("new = \"x\""));
        assert!(!out.contains("old ="));
        // The stamp comment added on first migration is present too.
        assert!(out.contains("# Auto-maintained by nanofile; do not edit by hand."));
        // The migrated document must still parse as a full TOML document.
        let _: toml::Value = toml::from_str(&out).expect("migrated doc is valid toml");
    }

    #[test]
    fn idempotent_no_write() {
        // Migration sets a key to the value it already has → nothing changed.
        let migrations = [ConfigMigration {
            version: "0.1.9",
            name: "same-value",
            apply: |doc| {
                doc.entry("port").or_insert(toml_edit::value(8082i64));
                Ok(())
            },
        }];
        // Older config, migration runs but sets the same value → no change,
        // and consequently no version stamp and no disk write.
        let mut doc = parse_doc("port = 8082\n");
        let report = migrate_with(&mut doc, &migrations, "0.1.9").unwrap();

        assert_eq!(report.applied, vec!["same-value"]);
        assert!(!report.changed);
        assert_eq!(doc.to_string(), "port = 8082\n");
        assert_eq!(read_config_version(&doc), "0.0.0");
    }

    #[test]
    fn migrations_are_sorted_valid() {
        // Every real migration must parse, be strictly ascending and be at or
        // below the current binary version.
        let mut prev: Option<&str> = None;
        for m in MIGRATIONS {
            let v = semver::Version::parse(m.version).unwrap_or_else(|_| {
                panic!("migration {} has invalid version {}", m.name, m.version)
            });
            let cur = semver::Version::parse(CURRENT_VERSION).unwrap();
            assert!(
                v <= cur,
                "migration {} version {} exceeds CURRENT_VERSION {}",
                m.name,
                m.version,
                CURRENT_VERSION
            );
            if let Some(p) = prev {
                assert!(
                    semver::Version::parse(p).unwrap() < v,
                    "migrations out of order: {p} then {}",
                    m.version
                );
            }
            prev = Some(m.version);
        }
    }

    #[test]
    fn scaffold_migration_is_noop() {
        // The placeholder migration must never touch an up-to-date file.
        let mut doc = parse_doc("config_version = \"0.1.9\"\n[server]\naddr = \"x\"\n");
        let report = migrate_config(&mut doc).unwrap();
        assert!(!report.changed);
        assert!(report.applied.is_empty());
    }

    #[cfg(unix)]
    mod persist {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn writes_backup_and_replaces() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "old").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

            persist_with_backup(&path, "new").unwrap();

            assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
            assert_eq!(std::fs::read_to_string(backup_path(&path)).unwrap(), "old");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "permissions must survive the atomic write");
        }

        #[test]
        fn fails_on_readonly_dir() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, "old").unwrap();
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

            // Root bypasses directory permissions — skip the assertion there.
            let is_root = libc_geteuid() == 0;
            let result = persist_with_backup(&path, "new");
            if is_root {
                let _ = result;
            } else {
                assert!(result.is_err(), "write into read-only dir must fail");
                // Original file untouched, no leftover temp file.
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
                assert!(!tmp_path(&path).exists());
            }
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[cfg(unix)]
    fn libc_geteuid() -> u32 {
        // Minimal euid check without pulling in a libc dev-dependency.
        let s = std::process::Command::new("id").arg("-u").output().unwrap();
        String::from_utf8_lossy(&s.stdout)
            .trim()
            .parse()
            .unwrap_or(1000)
    }
}
