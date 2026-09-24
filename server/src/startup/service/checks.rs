//! The shape of a preflight report and the predicates it is built from.
//!
//! Platform-independent so the rules that decide "this directory is not a place
//! to touch" and "the service may not keep running" are unit-tested on every
//! platform; the checks that need Win32 live in [`super::preflight`].
//!
//! Compiled with the Windows preflight, and in test builds everywhere so the
//! tests run on Linux CI too.

use std::io::Write;
use std::path::Path;

/// How a failed check is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    /// Refuse to install (before anything is written), or stop the service.
    Fail,
    /// Worth a confirmation before installing; the install may still proceed.
    Warn,
    /// Expected or merely informative: logged, never shown in a dialog.
    Note,
}

/// One check and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub severity: Severity,
    /// What was checked, e.g. `storage.block_dir` or `storage.ffmpeg_path`.
    pub subject: String,
    /// What the check found, meant to be read on its own.
    pub detail: String,
}

/// The result of a whole preflight run.
#[derive(Debug, Clone, Default)]
pub(crate) struct Preflight {
    pub checks: Vec<Check>,
}

impl Preflight {
    pub(crate) fn push(
        &mut self,
        severity: Severity,
        subject: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.checks.push(Check {
            severity,
            subject: subject.into(),
            detail: detail.into(),
        });
    }

    pub(crate) fn has(&self, severity: Severity) -> bool {
        self.checks.iter().any(|c| c.severity == severity)
    }

    pub(crate) fn of(&self, severity: Severity) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(move |c| c.severity == severity)
    }

    /// Whether the service may be registered (or keep running).
    pub(crate) fn is_ok(&self) -> bool {
        !self.has(Severity::Fail)
    }

    /// Every check at `severity` or worse, as lines for a dialog.
    pub(crate) fn lines_at_least(&self, severity: Severity) -> Vec<String> {
        let wanted = |s: Severity| match severity {
            Severity::Fail => s == Severity::Fail,
            Severity::Warn => matches!(s, Severity::Fail | Severity::Warn),
            Severity::Note => true,
        };
        self.checks
            .iter()
            .filter(|c| wanted(c.severity))
            .map(|c| format!("- {}: {}", c.subject, c.detail))
            .collect()
    }

    /// Log every check at its own level, so a failed service start leaves a
    /// record naming the directory.
    pub(crate) fn log(&self) {
        for check in &self.checks {
            match check.severity {
                Severity::Fail => tracing::error!("{}: {}", check.subject, check.detail),
                Severity::Warn => tracing::warn!("{}: {}", check.subject, check.detail),
                Severity::Note => tracing::info!("{}: {}", check.subject, check.detail),
            }
        }
    }
}

/// Create the directory and write a probe file into it, then remove it.
///
/// The server creates exactly these directories at startup, so creating one here
/// is not a side effect an install has to undo.
pub(crate) fn probe_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".nanofile-write-test-{}", std::process::id()));
    let mut file = std::fs::File::create(&probe)?;
    file.write_all(b"nanofile")?;
    file.sync_all()?;
    drop(file);
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// The volume or share root a path starts at: `C:\` for `C:\a\b`, and
/// `\\server\share` for `\\server\share\dir`.
pub(crate) fn volume_root(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    if text.starts_with("\\\\") {
        // `\\server\share\…`: the first two components name the share. `split`
        // rather than `splitn`, whose last item would swallow the rest of the
        // path.
        let mut parts = text.split('\\').filter(|p| !p.is_empty());
        let server = parts.next()?;
        let share = parts.next()?;
        return Some(format!("\\\\{server}\\{share}"));
    }
    let head: Vec<char> = text.chars().take(3).collect();
    if head.len() >= 2 && head[1] == ':' {
        Some(format!("{}:\\", head[0]))
    } else {
        None
    }
}

/// Whether `path` is itself one of the directories Windows owns, or a user
/// profile.
///
/// These are never a state directory and never a place to grant a service
/// account access: the directory itself would have to have its own access
/// control changed, which would hand the service (or anything running as it)
/// control of the system.
pub(crate) fn is_sensitive_root(path: &Path) -> bool {
    let mut roots: Vec<String> = Vec::new();
    for var in [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "USERPROFILE",
    ] {
        if let Ok(value) = std::env::var(var)
            && !value.trim().is_empty()
        {
            roots.push(value);
        }
    }
    // `C:\Users` holds every profile; `USERPROFILE` covers one of them.
    if let Some(profile) = roots.last()
        && let Some((parent, _)) = profile.rsplit_once('\\')
        && !parent.is_empty()
    {
        roots.push(parent.to_string());
    }
    is_sensitive_root_among(path, &roots)
}

/// [`is_sensitive_root`] against an explicit list of roots, so the rule can be
/// tested without depending on the machine's environment.
pub(crate) fn is_sensitive_root_among(path: &Path, roots: &[String]) -> bool {
    let normalize = |p: &str| {
        p.trim()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    let candidate = normalize(&path.to_string_lossy());
    if candidate.is_empty() {
        return false;
    }
    // A bare volume: `c:` or `c:\`.
    if candidate.len() == 2 && candidate.ends_with(':') {
        return true;
    }
    if candidate.len() == 3 && candidate.ends_with(":\\") {
        return true;
    }
    if candidate.starts_with("\\\\")
        && let Some(root) = volume_root(path)
    {
        // A share root (`\\server\share`) is as broad as a volume root; a
        // directory inside the share is not.
        return normalize(&root) == candidate;
    }
    roots.iter().any(|root| normalize(root) == candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_root_is_recognised() {
        assert_eq!(
            volume_root(Path::new(r"C:\nanofile\data")),
            Some(r"C:\".into())
        );
        assert_eq!(volume_root(Path::new(r"c:/nanofile")), Some(r"c:\".into()));
        assert_eq!(
            volume_root(Path::new(r"\\server\share\dir\file")),
            Some(r"\\server\share".into())
        );
        // A relative path has no volume.
        assert_eq!(volume_root(Path::new("data/blocks")), None);
        assert_eq!(volume_root(Path::new("/var/lib/nanofile")), None);
    }

    #[test]
    fn sensitive_roots_are_refused_and_ordinary_directories_are_not() {
        let roots = vec![
            r"C:\Windows".to_string(),
            r"C:\Program Files".to_string(),
            r"C:\Users\veoco".to_string(),
            r"C:\Users".to_string(),
        ];
        // Bare volumes, however written.
        for volume in [r"C:", r"C:\", r"c:/"] {
            assert!(
                is_sensitive_root_among(Path::new(volume), &roots),
                "{volume} is a volume root"
            );
        }
        // The directories Windows owns.
        for dir in [
            r"C:\Windows",
            r"c:\windows\\",
            r"C:/Program Files",
            r"C:\Users",
            r"C:\Users\veoco",
        ] {
            assert!(
                is_sensitive_root_among(Path::new(dir), &roots),
                "{dir} is sensitive"
            );
        }
        // A directory of its own, even underneath one of them, is fine.
        for dir in [
            r"C:\nanofile\data",
            r"C:\Users\veoco\AppData\Local\Nanofile\data",
            r"C:\Program Files\Nanofile\data",
        ] {
            assert!(
                !is_sensitive_root_among(Path::new(dir), &roots),
                "{dir} is a directory of its own"
            );
        }
    }

    #[test]
    fn a_share_root_is_as_broad_as_a_volume_root() {
        assert!(is_sensitive_root_among(Path::new(r"\\server\share"), &[]));
        assert!(!is_sensitive_root_among(
            Path::new(r"\\server\share\nanofile"),
            &[]
        ));
    }

    #[test]
    fn a_write_probe_reports_a_directory_it_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        assert!(probe_writable(&dir.path().join("nested/blocks")).is_ok());
        assert!(dir.path().join("nested/blocks").is_dir());

        // A file where a directory has to be: the probe must say so rather than
        // silently accepting the path.
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        assert!(probe_writable(&file).is_err());
    }

    #[test]
    fn severities_decide_refusal_and_what_reaches_a_dialog() {
        let mut report = Preflight::default();
        assert!(report.is_ok());
        report.push(Severity::Note, "server.addr", "in use");
        assert!(report.is_ok(), "a note never blocks");
        report.push(Severity::Warn, "storage.block_dir", "network drive");
        assert!(report.is_ok(), "a warning never blocks");
        assert_eq!(report.lines_at_least(Severity::Warn).len(), 1);
        assert_eq!(report.lines_at_least(Severity::Note).len(), 2);
        report.push(Severity::Fail, "storage.block_dir", "denied");
        assert!(!report.is_ok(), "a failure blocks");
        assert_eq!(report.lines_at_least(Severity::Fail).len(), 1);
        assert_eq!(report.lines_at_least(Severity::Warn).len(), 2);
        assert_eq!(
            report.of(Severity::Fail).next().unwrap().subject,
            "storage.block_dir"
        );
    }
}
