//! Confinement for the extraction child process.
//!
//! Document parsers are the only code here that reads attacker-chosen bytes, so
//! they run in a separate process ([`super::worker`]) that confines itself
//! before it reads a single byte. The layers, and what each is for:
//!
//! | Layer | What it bounds | Linux | macOS | Windows |
//! |---|---|---|---|---|
//! | limits | address space, CPU seconds, descriptors, file size | `prlimit64` | `setrlimit` | Job Object memory cap |
//! | files | reading or writing any path | Landlock, zero grants | Seatbelt profile | — |
//! | network | creating a socket | seccomp denylist | Seatbelt profile | — |
//! | process | `exec`, `fork`, extra processes | seccomp denylist | Seatbelt profile | Job active-process limit |
//!
//! # Levels and policy
//!
//! [`Level`] is what the layers add up to, and the `index.sandbox` setting says
//! what the server requires of them. `require` (the default) refuses to extract
//! a document that would run with nothing but resource limits, which is the
//! `None` level; `prefer` extracts anyway and logs the shortfall. Both run the
//! child — there is no in-process path to fall back to.
//!
//! # Claims are measured
//!
//! A layer is claimed by the mechanism that installed it and then *verified*:
//! reading a path outside the document and creating a socket have to fail for
//! the files and network layers to count. A measurement that contradicts the
//! mechanism clears the layer, so a sandbox that silently did nothing is
//! reported as `none` rather than as protection that is not there. The same
//! reasoning is why the macOS runner's claims are all three measured: the
//! profile is applied by a wrapper this process cannot inspect.
//!
//! # What this is not
//!
//! It bounds what a parser can do to the *host* — memory, files, sockets,
//! processes. It is not a boundary against a kernel vulnerability, nor against
//! another process running as the same user. Both references below say the same
//! of their own sandboxes.
//!
//! # References
//!
//! The shape follows two open-source agents that confine untrusted work:
//!
//! * DeepSeek Harness's `landlock-run` (a plain-C, libc-only Landlock launcher
//!   whose UAPI structs are defined locally so the file doubles as the audit
//!   record) and its `--probe`, which enforces a maximal ruleset to answer
//!   "is this kernel actually enforcing" rather than "does the syscall exist".
//! * OpenAI Codex's Linux sandbox, whose seccomp filter is a *denylist*
//!   returning `EPERM` rather than an allowlist, and which verifies afterwards
//!   that no capability survived.
//!
//! Deliberate differences: no `bwrap` and no namespaces (the child never spawns
//! a program, so a mount or PID namespace would add a dependency and no bound),
//! no external helper binary (the child is our own binary and confines itself),
//! and resource limits, which neither reference applies and which are the whole
//! point here.

use std::path::Path;

#[cfg(target_os = "linux")]
mod linux;
// The Seatbelt profile is built on every platform so its tests can check the
// text, but only macOS ever runs under it.
#[cfg(any(target_os = "macos", test))]
mod seatbelt;
#[cfg(target_os = "windows")]
mod windows;
/// Start the child the way Windows has to: a restricted token cannot be applied
/// to a running process, so it is part of creation.
#[cfg(target_os = "windows")]
pub(super) use windows::{Child as WindowsChild, spawn};

/// Most address space a child may use.
///
/// A document is at most `MAX_STRUCTURED_BYTES` on disk and at most 256 MiB
/// decompressed, and the text it yields is capped at 8 MiB; a gigabyte leaves
/// room for the parser's own structures and still bounds a bomb.
#[cfg(unix)]
const ADDRESS_SPACE_LIMIT: u64 = 1 << 30;

/// CPU seconds before the soft (`SIGXCPU`) and hard (`SIGKILL`) limits.
///
/// The parent also enforces a wall-clock timeout; this one holds even when the
/// parent is not scheduled to enforce it.
#[cfg(unix)]
const CPU_SOFT_SECONDS: u64 = 15;
#[cfg(unix)]
const CPU_HARD_SECONDS: u64 = 18;

/// Open descriptors: the three standard streams and slack for the runtime.
#[cfg(unix)]
const NOFILE_LIMIT: u64 = 32;

/// Most bytes the child may write to a *file*.
///
/// Not zero: the parent hands it pipes, but a redirected standard output is a
/// file too, and the reply is at most a few megabytes. Paths are denied by the
/// filesystem layer regardless, so this only bounds a descriptor that was
/// handed over.
#[cfg(unix)]
const FILE_SIZE_LIMIT: u64 = 16 * 1024 * 1024;

/// How much confinement a child established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Resource limits only, or nothing at all.
    None,
    /// Resource limits plus at least one real confinement layer.
    Partial,
    /// Every layer this platform can provide.
    Full,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::Partial => "partial",
            Level::Full => "full",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "none" => Some(Level::None),
            "partial" => Some(Level::Partial),
            "full" => Some(Level::Full),
            _ => None,
        }
    }
}

/// The confinement layers actually in force.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Layers {
    /// Address space, CPU seconds, descriptors, file size.
    pub limits: bool,
    /// Paths cannot be read or written.
    pub files: bool,
    /// Sockets cannot be created.
    pub network: bool,
    /// Programs cannot be started and processes cannot be multiplied.
    pub process: bool,
}

impl Layers {
    /// What these layers add up to.
    ///
    /// [`Level::Full`] needs all four. [`Level::Partial`] is any confinement
    /// beyond resource limits — a host without Landlock still confines the
    /// network, and Windows' Job Object confines processes — while a process
    /// held only by `RLIMIT_*` is [`Level::None`], which the `require` policy
    /// refuses.
    pub fn level(self) -> Level {
        if self.limits && self.files && self.network && self.process {
            Level::Full
        } else if self.limits && (self.files || self.network || self.process) {
            Level::Partial
        } else {
            Level::None
        }
    }
}

/// What the child established, and how.
#[derive(Debug, Clone)]
pub struct Report {
    pub layers: Layers,
    /// Mechanism tokens and measurements, comma-joined and whitespace-free.
    pub detail: String,
}

impl Report {
    pub fn level(&self) -> Level {
        self.layers.level()
    }

    /// The one-line report the self-test prints and the parent parses.
    pub fn line(&self) -> String {
        let yes = |on: bool| if on { "denied" } else { "open" };
        format!(
            "NFX1-sandbox level={} limits={} files={} network={} process={} detail={}",
            self.level().as_str(),
            if self.layers.limits { "on" } else { "off" },
            yes(self.layers.files),
            yes(self.layers.network),
            yes(self.layers.process),
            self.detail
        )
    }

    /// Parse a line produced by [`Report::line`].
    ///
    /// Returns `None` for anything that is not one, which is what makes a
    /// truncated or foreign line a failure rather than a wrong level.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        let mut fields = line.strip_prefix("NFX1-sandbox ")?.split(' ');
        let mut layers = Layers::default();
        let mut level = None;
        let mut detail = String::new();

        for field in fields.by_ref() {
            let (key, value) = field.split_once('=')?;
            match key {
                "level" => level = Level::parse(value),
                "limits" => layers.limits = parse_switch(value)?,
                "files" => layers.files = parse_denial(value)?,
                "network" => layers.network = parse_denial(value)?,
                "process" => layers.process = parse_denial(value)?,
                "detail" => {
                    detail = value.to_string();
                }
                // Extra tokens carry diagnostics the parent may want to log
                // (`text_chars=…`), so an unknown name is not an error; the
                // shape of the line still is.
                _ => {}
            }
        }

        // The level is redundant with the layers, so a line whose two halves
        // disagree is not one of ours.
        let report = Report { layers, detail };
        (level == Some(report.level())).then_some(report)
    }
}

fn parse_switch(value: &str) -> Option<bool> {
    match value {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn parse_denial(value: &str) -> Option<bool> {
    match value {
        "denied" => Some(true),
        "open" => Some(false),
        _ => None,
    }
}

/// What the child must have before it will read a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Refuse to extract without at least [`Level::Partial`].
    Require,
    /// Extract at any level, reporting the shortfall.
    Prefer,
}

impl Policy {
    /// Parse the `index.sandbox` setting.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "require" => Some(Policy::Require),
            "prefer" => Some(Policy::Prefer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Require => "require",
            Policy::Prefer => "prefer",
        }
    }

    pub fn accepts(self, level: Level) -> bool {
        match self {
            Policy::Require => level >= Level::Partial,
            Policy::Prefer => true,
        }
    }
}

/// A wrapper the *parent* must apply around the child.
#[derive(Debug, Clone)]
pub struct Runner {
    pub program: &'static str,
    pub args: Vec<String>,
    /// Whether the runner is what confines files, the network and process
    /// creation, so the child must not refuse for lacking those layers itself.
    pub external_confinement: bool,
}

/// The runner this platform needs around `exe`, if any.
///
/// Only macOS needs one: a Seatbelt profile can only be applied by
/// `/usr/bin/sandbox-exec`, and the path is hardcoded so a `PATH` entry cannot
/// substitute a different program.
pub fn runner(exe: &Path) -> Option<Runner> {
    #[cfg(target_os = "macos")]
    {
        Some(Runner {
            program: seatbelt::PROGRAM,
            args: seatbelt::args(exe),
            external_confinement: true,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = exe;
        None
    }
}

/// Apply every layer this platform offers to the current process.
///
/// Must be called on the child's only thread and before the document is read:
/// Landlock and seccomp are inherited by threads created afterwards, and the
/// parsers create one.
pub fn confine(external_confinement: bool) -> Report {
    let (mut layers, mut detail) = platform_confine();

    if external_confinement {
        // The parent asserts it wrapped this process in a runner whose profile
        // governs all three. Every claim is measured below, so a wrapper that
        // silently did nothing drops the level instead of reporting it.
        layers.files = true;
        layers.network = true;
        layers.process = true;
        detail.push("runner=external".to_string());
    }

    #[cfg(unix)]
    {
        if layers.files {
            if files_are_denied() {
                detail.push("files=measured-denied".to_string());
            } else {
                layers.files = false;
                detail.push("files=measured-open".to_string());
            }
        }
        if layers.network {
            if network_is_denied() {
                detail.push("network=measured-denied".to_string());
            } else {
                layers.network = false;
                detail.push("network=measured-open".to_string());
            }
        }
        if external_confinement {
            if process_is_denied() {
                detail.push("process=measured-denied".to_string());
            } else {
                layers.process = false;
                detail.push("process=measured-open".to_string());
            }
        }
    }

    // The parsers spawn one thread of their own; a sandbox that breaks that
    // would make every document unreadable, so it is reported rather than
    // assumed.
    detail.push(
        if threads_work() {
            "threads=ok"
        } else {
            "threads=failed"
        }
        .to_string(),
    );

    Report {
        layers,
        detail: detail.join(","),
    }
}

/// Whether reading a path outside the document is refused, measured.
///
/// `/` always exists, so a permission failure on it can only come from the
/// sandbox that was just installed.
#[cfg(unix)]
fn files_are_denied() -> bool {
    ["/", "/etc/hostname", "/etc/passwd", "/usr/lib"]
        .iter()
        .any(|path| {
            matches!(
                std::fs::File::open(path),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
            )
        })
}

/// Whether creating a socket is refused, measured.
///
/// Binding an ephemeral UDP port needs no peer and cannot fail for any reason
/// other than the sandbox, which makes it the honest probe: a refused `socket`
/// or `bind` is the denial, and anything else means sockets still work.
#[cfg(unix)]
fn network_is_denied() -> bool {
    match std::net::UdpSocket::bind("127.0.0.1:0") {
        Ok(socket) => {
            drop(socket);
            false
        }
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

/// Whether starting another program is refused, measured.
///
/// Only used to check a claim made by an external runner. A missing helper is
/// not a denial: only a permission failure counts.
///
/// The standard streams are inherited rather than set to null: a null stream
/// opens `/dev/null` for writing, which the macOS profile denies, and the probe
/// would then measure that open instead of the `exec` it is about.
#[cfg(unix)]
fn process_is_denied() -> bool {
    let helper = if cfg!(target_os = "macos") {
        "/usr/bin/true"
    } else {
        "/bin/true"
    };
    match std::process::Command::new(helper)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
    {
        Ok(_) => false,
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

/// Clamp the limits that bound what a parser can cost.
///
/// Both the soft and the hard value are set: a soft-only clamp can be raised by
/// the very process it is meant to bound.
#[cfg(unix)]
fn clamp_resources() -> bool {
    macro_rules! limit {
        ($resource:expr, $soft:expr, $hard:expr) => {{
            let limit = libc::rlimit {
                rlim_cur: $soft as libc::rlim_t,
                rlim_max: $hard as libc::rlim_t,
            };
            unsafe { libc::setrlimit($resource, &limit) == 0 }
        }};
    }

    let mut ok = true;
    ok &= limit!(libc::RLIMIT_AS, ADDRESS_SPACE_LIMIT, ADDRESS_SPACE_LIMIT);
    ok &= limit!(libc::RLIMIT_CPU, CPU_SOFT_SECONDS, CPU_HARD_SECONDS);
    ok &= limit!(libc::RLIMIT_NOFILE, NOFILE_LIMIT, NOFILE_LIMIT);
    // A crash must not write a core file, and the child has no business writing
    // a file at all: this is the belt to the filesystem layer's braces.
    ok &= limit!(libc::RLIMIT_FSIZE, FILE_SIZE_LIMIT, FILE_SIZE_LIMIT);
    ok &= limit!(libc::RLIMIT_CORE, 0, 0);
    ok
}

/// The one-line detail a platform reports for its resource limits.
#[cfg(unix)]
fn limits_detail() -> String {
    format!(
        "limits=as{},cpu{}/{},nofile{}",
        ADDRESS_SPACE_LIMIT, CPU_SOFT_SECONDS, CPU_HARD_SECONDS, NOFILE_LIMIT
    )
}

/// Whether this process can still spawn a thread.
fn threads_work() -> bool {
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(|| ())
        .and_then(|handle| {
            handle
                .join()
                .map_err(|_| std::io::Error::other("thread panicked"))
        })
        .is_ok()
}

#[cfg(target_os = "linux")]
fn platform_confine() -> (Layers, Vec<String>) {
    linux::confine()
}

#[cfg(target_os = "macos")]
fn platform_confine() -> (Layers, Vec<String>) {
    seatbelt::confine()
}

#[cfg(target_os = "windows")]
fn platform_confine() -> (Layers, Vec<String>) {
    windows::confine()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_confine() -> (Layers, Vec<String>) {
    (
        Layers::default(),
        vec![format!("backend=none,os={}", std::env::consts::OS)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_needs_limits_and_something_else() {
        assert_eq!(Layers::default().level(), Level::None);
        assert_eq!(
            Layers {
                limits: true,
                ..Layers::default()
            }
            .level(),
            Level::None,
            "resource limits alone are not confinement"
        );
        for other in ["files", "network", "process"] {
            let mut layers = Layers {
                limits: true,
                ..Layers::default()
            };
            match other {
                "files" => layers.files = true,
                "network" => layers.network = true,
                _ => layers.process = true,
            }
            assert_eq!(layers.level(), Level::Partial, "{other}");
        }
        assert_eq!(
            Layers {
                limits: true,
                files: true,
                network: true,
                process: true,
            }
            .level(),
            Level::Full
        );
        assert_eq!(
            Layers {
                files: true,
                ..Layers::default()
            }
            .level(),
            Level::None,
            "confinement without resource limits is not a level"
        );
    }

    #[test]
    fn the_require_policy_refuses_the_none_level() {
        assert!(!Policy::Require.accepts(Level::None));
        assert!(Policy::Require.accepts(Level::Partial));
        assert!(Policy::Require.accepts(Level::Full));
        for level in [Level::None, Level::Partial, Level::Full] {
            assert!(Policy::Prefer.accepts(level));
        }
        assert_eq!(Policy::parse(" require "), Some(Policy::Require));
        assert_eq!(Policy::parse("prefer"), Some(Policy::Prefer));
        assert_eq!(Policy::parse("off"), None);
    }

    #[test]
    fn a_report_round_trips_through_its_line() {
        let report = Report {
            layers: Layers {
                limits: true,
                files: true,
                network: true,
                process: true,
            },
            detail: "landlock_abi=6,seccomp=on".to_string(),
        };
        let line = report.line();
        assert!(line.starts_with("NFX1-sandbox level=full "), "{line}");
        let parsed = Report::parse(&line).expect("parses");
        assert_eq!(parsed.layers, report.layers);
        assert_eq!(parsed.level(), Level::Full);
        assert_eq!(parsed.detail, report.detail);
    }

    #[test]
    fn a_line_that_disagrees_with_itself_is_refused() {
        assert!(Report::parse("").is_none());
        assert!(Report::parse("level=full limits=on").is_none());
        assert!(
            Report::parse(
                "NFX1-sandbox level=full limits=on files=open network=open process=open detail=x"
            )
            .is_none(),
            "the level token must match the layers"
        );
        assert!(
            Report::parse(
                "NFX1-sandbox level=none limits=maybe files=open network=open process=open detail=x"
            )
            .is_none()
        );
    }
}
