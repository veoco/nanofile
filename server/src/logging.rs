//! Log initialization.
//!
//! Headless runs (servers, Docker, CLI subcommands) log to stdout as before.
//! Desktop runs log to a size-capped, rotating file instead — by default
//! `nanofile.log` next to the binary, so the log is found in the same place
//! no matter how the process was started (double-click, login autostart,
//! terminal). The resolved default path is written back into config.toml so
//! later runs — including login-started instances, whose working directory is
//! meaningless — always use the same file.
//!
//! Rotation is done by a small custom [`MakeWriter`]: `tracing-appender` only
//! supports time-based rotation, while the desktop requirement is a hard cap
//! per file with a bounded number of backups.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};

use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

use infra::config::Config;

/// Default log file name, resolved against the binary's directory.
const DEFAULT_LOG_FILE_NAME: &str = "nanofile.log";

/// Which kind of process is starting — decides the default log target.
pub enum Kind {
    /// The long-running HTTP server (headless or tray).
    Server,
    /// Short-lived CLI subcommands (`adduser`): interactive, always console.
    Cli,
}

/// Route panics through tracing so they reach the log file even in
/// GUI-subsystem Windows builds, where the default stderr hook is invisible.
/// The previous (default) hook still runs, keeping normal stderr output in
/// headless builds.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("PANIC {info}");
        default_hook(info);
    }));
}

/// Initialize the global tracing subscriber.
///
/// `tray_mode` says the server is presenting the desktop tray UI; `config_path`
/// is the config file this instance loaded, used to persist the resolved
/// default log file path (never created if the file doesn't exist yet).
pub fn init(config: &Config, kind: Kind, tray_mode: bool, config_path: &Path) {
    let want_file = match config.logging.file_enabled {
        Some(enabled) => enabled,
        None => match kind {
            Kind::Cli => false,
            // A Windows tray build is a GUI-subsystem binary: its stdout is
            // invisible even when the tray is disabled via config, so file
            // logging is the only useful default there too.
            Kind::Server => tray_mode || cfg!(all(target_os = "windows", feature = "tray")),
        },
    };
    if !want_file {
        init_stdout(config);
        return;
    }

    match open_file_logging(config) {
        Err(e) => {
            eprintln!("[nanofile] WARN: cannot open a log file ({e}); logging to stdout");
            init_stdout(config);
        }
        Ok(backend) => {
            if config.logging.file.is_none() {
                persist_default_path(config_path, &backend.path);
            }
            tracing_subscriber::fmt()
                .with_env_filter(env_filter(config))
                // No ANSI color codes in the log file.
                .with_ansi(false)
                .with_writer(backend.handle.clone())
                .init();
            tracing::info!(
                "Logging to {} (max {} MiB per file, {} backup file(s))",
                backend.path.display(),
                config.logging.max_file_size_mb.max(1),
                config.logging.max_backups
            );
        }
    }
}

fn env_filter(config: &Config) -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
        .parse_lossy(&config.logging.level)
}

fn init_stdout(config: &Config) {
    tracing_subscriber::fmt()
        .with_env_filter(env_filter(config))
        .init();
}

struct FileBackend {
    handle: LogHandle,
    path: PathBuf,
}

/// Open the rotating log at the first candidate path that works. Candidates
/// come in decreasing preference (binary dir → working directory), so a
/// read-only binary directory degrades gracefully instead of losing logs.
fn open_file_logging(config: &Config) -> io::Result<FileBackend> {
    let max_bytes = config
        .logging
        .max_file_size_mb
        .max(1)
        .saturating_mul(1024 * 1024);
    let backups = config.logging.max_backups;

    let mut last_err = None;
    for path in candidate_paths(config) {
        match RotatingLog::open(&path, max_bytes, backups) {
            Ok(handle) => {
                return Ok(FileBackend {
                    handle: LogHandle(Arc::new(handle)),
                    path,
                });
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no log file location available")
    }))
}

/// Log file candidates in preference order. Explicit absolute paths win;
/// relative paths resolve against the binary's directory (never the working
/// directory, which is `C:\Windows\System32` or `/` for login-started
/// instances), with the working directory as a fallback when the binary
/// directory cannot be determined or written.
fn candidate_paths(config: &Config) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    match &config.logging.file {
        Some(file) => {
            let p = PathBuf::from(file);
            if p.is_absolute() {
                candidates.push(p);
            } else {
                if let Some(dir) = exe_dir() {
                    candidates.push(dir.join(&p));
                }
                candidates.push(p);
            }
        }
        None => {
            if let Some(dir) = exe_dir() {
                candidates.push(dir.join(DEFAULT_LOG_FILE_NAME));
            }
            candidates.push(PathBuf::from(DEFAULT_LOG_FILE_NAME));
        }
    }
    candidates
}

fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = absolute(exe);
    exe.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

/// Canonical absolute path with Windows `\\?\` verbatim prefixes stripped,
/// falling back to the input when canonicalization fails.
fn absolute(path: PathBuf) -> PathBuf {
    let resolved = path.canonicalize().unwrap_or(path);
    #[cfg(windows)]
    {
        let s = resolved.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            resolved
        }
    }
    #[cfg(not(windows))]
    resolved
}

/// Bake the resolved default log path into config.toml so every later run —
/// including login-started instances — reads the same explicit path. Best
/// effort: a missing config file stays missing (zero-config setups), and a
/// write failure only degrades to a warning.
fn persist_default_path(config_path: &Path, resolved: &Path) {
    if !config_path.is_file() {
        return;
    }
    if let Err(e) = infra::config::write_config_value(
        config_path,
        "logging",
        "file",
        &resolved.to_string_lossy(),
    ) {
        eprintln!(
            "[nanofile] WARN: could not persist the log file path to {}: {e}",
            config_path.display()
        );
    }
}

// ── Size-capped rotating log file ────────────────────────────────────────────

struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    max_backups: u32,
    /// `None` while closed (after a rotation or a failed write); reopened
    /// lazily on the next event.
    state: Mutex<Option<(File, u64)>>,
}

impl RotatingLog {
    fn open(path: &Path, max_bytes: u64, max_backups: u32) -> io::Result<Self> {
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            max_backups,
            state: Mutex::new(Some((file, written))),
        })
    }

    /// Write one formatted event, rotating first if the size cap would be
    /// exceeded. Never fails: log writes must not take the server down, so
    /// failures close the file (the first failure is echoed to stderr) and
    /// later events retry the reopen.
    fn write_chunk(&self, buf: &[u8]) {
        if buf.is_empty() {
            return;
        }
        let mut slot = match self.state.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_none() && self.reopen(&mut slot).is_err() {
            return;
        }
        let over_cap = self.max_bytes > 0
            && slot
                .as_ref()
                .is_some_and(|(_, written)| *written + buf.len() as u64 > self.max_bytes);
        if over_cap {
            self.rotate(&mut slot);
        }
        if let Some((file, written)) = slot.as_mut() {
            match file.write_all(buf) {
                Ok(()) => *written += buf.len() as u64,
                Err(e) => {
                    write_failed_once(&e);
                    *slot = None;
                }
            }
        }
    }

    fn rotate(&self, slot: &mut Option<(File, u64)>) {
        // Drop the handle first so the rename succeeds on Windows, which
        // refuses to rename open files.
        *slot = None;
        if self.max_backups == 0 {
            // No backups kept: start over in place.
            if let Ok(f) = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.path)
            {
                *slot = Some((f, 0));
            }
            return;
        }
        let _ = std::fs::remove_file(backup_path(&self.path, self.max_backups));
        for i in (1..self.max_backups).rev() {
            let _ = std::fs::rename(backup_path(&self.path, i), backup_path(&self.path, i + 1));
        }
        let _ = std::fs::rename(&self.path, backup_path(&self.path, 1));
        let _ = self.reopen(slot);
    }

    fn reopen(&self, slot: &mut Option<(File, u64)>) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        *slot = Some((file, written));
        Ok(())
    }
}

fn backup_path(base: &Path, n: u32) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

fn write_failed_once(e: &io::Error) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        eprintln!("[nanofile] WARN: log file writes failing, giving up on them: {e}");
    });
}

impl<'a> MakeWriter<'a> for LogHandle {
    type Writer = RotatingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingWriter(Arc::clone(&self.0))
    }
}

/// Cloneable handle to the rotating log, accepted by the tracing fmt builder.
#[derive(Clone)]
struct LogHandle(Arc<RotatingLog>);

struct RotatingWriter(Arc<RotatingLog>);

impl io::Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_chunk(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_log(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        dir.path().join(name)
    }

    #[test]
    fn rotates_current_file_to_backup_when_cap_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_log(&dir, "nanofile.log");
        let log = RotatingLog::open(&path, 100, 2).unwrap();

        log.write_chunk(&[b'a'; 60]);
        assert_eq!(std::fs::read(&path).unwrap().len(), 60);

        // Second chunk crosses the cap → current file becomes .1.
        log.write_chunk(&[b'b'; 60]);
        assert_eq!(
            std::fs::read(backup_path(&path, 1)).unwrap(),
            vec![b'a'; 60]
        );
        assert_eq!(std::fs::read(&path).unwrap(), vec![b'b'; 60]);
    }

    #[test]
    fn shifts_backups_and_drops_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_log(&dir, "nanofile.log");
        let log = RotatingLog::open(&path, 10, 2).unwrap();

        for tag in [b'1', b'2', b'3', b'4'] {
            log.write_chunk(&[tag; 12]); // each write rotates
        }
        // Four rotations: .log.2 / .log.1 / .log kept, nothing older.
        assert_eq!(
            std::fs::read(backup_path(&path, 2)).unwrap(),
            vec![b'2'; 12]
        );
        assert_eq!(
            std::fs::read(backup_path(&path, 1)).unwrap(),
            vec![b'3'; 12]
        );
        assert_eq!(std::fs::read(&path).unwrap(), vec![b'4'; 12]);
        assert!(!backup_path(&path, 3).exists());
    }

    #[test]
    fn appends_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_log(&dir, "nanofile.log");

        RotatingLog::open(&path, 10_000, 1)
            .unwrap()
            .write_chunk(b"first\n");
        RotatingLog::open(&path, 10_000, 1)
            .unwrap()
            .write_chunk(b"second\n");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
    }

    #[test]
    fn zero_backups_truncates_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_log(&dir, "nanofile.log");
        let log = RotatingLog::open(&path, 10, 0).unwrap();

        log.write_chunk(&[b'x'; 20]);
        log.write_chunk(&[b'y'; 5]);

        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, vec![b'y'; 5], "truncated on rotation");
        assert!(!backup_path(&path, 1).exists());
    }

    #[test]
    fn make_writer_routes_events_through_the_rotator() {
        let dir = tempfile::tempdir().unwrap();
        let path = tmp_log(&dir, "nanofile.log");
        let handle = LogHandle(Arc::new(RotatingLog::open(&path, 100_000, 1).unwrap()));

        let mut writer = handle.make_writer();
        writer.write_all(b"event line\n").unwrap();
        drop(writer);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "event line\n");
    }

    #[test]
    fn default_candidates_prefer_the_binary_directory() {
        let mut config = Config::default();
        let candidates = candidate_paths(&config);
        assert!(candidates.len() >= 1);
        assert_eq!(
            candidates[0].file_name().and_then(|n| n.to_str()),
            Some("nanofile.log")
        );
        if let Some(dir) = exe_dir() {
            assert_eq!(candidates[0].parent(), Some(dir.as_path()));
        }

        // A relative custom path resolves against the binary dir first, with
        // the raw (cwd-relative) path as fallback.
        config.logging.file = Some(PathBuf::from("logs/custom.log"));
        let candidates = candidate_paths(&config);
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[0].file_name().and_then(|n| n.to_str()),
            Some("custom.log")
        );

        // Absolute paths are used verbatim, no fallback needed.
        config.logging.file = Some(PathBuf::from("/var/log/nanofile.log"));
        assert_eq!(candidate_paths(&config).len(), 1);
    }
}
