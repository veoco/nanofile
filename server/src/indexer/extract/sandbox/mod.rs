//! Confinement for the extraction child process.
//!
//! Document parsers are the only code here that reads attacker-chosen bytes, so
//! they run in a separate process ([`super::worker`]) that confines itself
//! before it reads a single byte. The layers, and what each is for:
//!
//! | Layer | What it bounds | Linux | macOS | Windows |
//! |---|---|---|---|---|
//! | limits | memory, CPU seconds, descriptors, file size | `RLIMIT_AS` | `setrlimit` (mapped space plus the cap), footprint watchdog as fallback | Job Object memory cap |
//! | files | reading or writing any path | Landlock, zero grants | Seatbelt profile | — |
//! | network | creating a socket | seccomp denylist | Seatbelt profile | — |
//! | process | `exec`, `fork`, extra processes | seccomp denylist | Seatbelt profile, `exec` only (`process-fork` is allowed) | Job active-process limit, plus the kernel's child-process policy |
//!
//! Memory is the one limit each platform has to be told about differently. Linux
//! takes the cap as an address-space limit outright; Windows caps committed
//! memory with a Job Object; Darwin refuses a limit below what a process already
//! has mapped, so there the limit is the mapped size plus the cap — the same
//! bound, stated from where the process already is. Where even that is refused
//! (macOS 11 and older, whose VM map has no size limit) the child watches its own
//! footprint instead. The report says which of them took rather than which were
//! attempted.
//!
//! # Levels and policy
//!
//! [`Level`] is what the layers add up to, and the `index.sandbox` setting says
//! what the server requires of them. `require` (the default) refuses to extract
//! a document that would run with nothing but resource limits, which is the
//! `None` level; `strict` refuses anything short of [`Level::Full`], which is
//! what makes a layer the platform can only sometimes provide — the Windows
//! AppContainer is the one — a condition rather than a hope; `sealed` refuses a
//! confinement that is full but not [`Closure::Full`], which is the one thing
//! the level cannot say; `prefer` extracts anyway and logs the shortfall. All
//! four run the child — there is no in-process path to fall back to.
//!
//! # What a level does not say
//!
//! [`Level::Full`] means "every layer this platform can provide", and the three
//! platforms provide materially different things under that name: Linux denies
//! every path and every socket through Landlock and seccomp, the Windows
//! container denies the user's own files and the network but reads the system
//! tree it loads from, and macOS's profile is a deny-by-default text with
//! `process-fork` allowed because the parsers' thread needs it. `strict` is a
//! statement about the *deployment* — this host gave what it has — and an
//! operator who needs the stronger claim wants `sealed`, which reads
//! [`Closure`]: the layer set plus the absence of the residuals each platform
//! documents (`system=`, `writes=`, `fork=`, `ll_gaps=`).
//!
//! One consequence is worth stating where the setting is: the residuals those
//! tokens name cannot be closed on Windows without a less privileged container
//! (LPAC) and cannot be closed on macOS without bounding `fork`, so `sealed`
//! is expected to refuse every document there. It is not a stronger version of
//! the same promise; it is the promise that the sandbox has no known way out,
//! which on those two platforms is not true.
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
//! The Windows files layer is the one whose boundary is not a rule this process
//! writes for itself: an AppContainer denies what the user's own ACLs grant and
//! permits what `ALL APPLICATION PACKAGES` grants, which is the system tree it
//! loads from. So the child measures both edges of that window and reports the
//! far one as `system=`, without letting it clear the layer — a parser that gets
//! loose in the child reads `Windows` whether or not that is recorded, and what
//! the report can do is say so next to the fact that the user's own files are
//! refused.
//!
//! Memory is measured the same way. The limit counts only if the kernel took it
//! — on macOS that means a limit stated from the space the process has already
//! mapped, and a query that failed to report that space leaves the limit below
//! what is mapped and the layer off. Where the kernel takes no such limit at all,
//! the fallback reports itself armed only after reading the footprint it watches:
//! a bound that cannot read the number it bounds is not a bound.
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
/// How macOS states an address-space limit, and the watchdog it falls back to
/// where the kernel will not take one.
#[cfg(target_os = "macos")]
mod watchdog;
#[cfg(target_os = "windows")]
mod windows;
/// Start the child the way Windows has to: a restricted token cannot be applied
/// to a running process, so it is part of creation. `spawn_unrestricted` is the
/// same start without the token, which is how the probe tells a child its token
/// killed from a child that never started.
#[cfg(target_os = "windows")]
pub(super) use windows::{Child as WindowsChild, spawn, spawn_token_only, spawn_unrestricted};

/// Why the child was not started in an AppContainer, where the platform has one.
///
/// `None` when the container was applied, was never asked for, or cannot exist on
/// this platform. The probe prints what this says, because the child can only
/// report that its token is not one, not why.
pub fn container_shortfall() -> Option<&'static str> {
    #[cfg(target_os = "windows")]
    {
        windows::shortfall()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Exit code the child uses when it stops itself at its memory bound.
///
/// Nothing else the child does produces it, and the parent reads it as a
/// document that expanded past the extraction budget rather than as a crash:
/// only the macOS watchdog exits this way, because the other two platforms have
/// the kernel stop the process instead.
pub const EXIT_MEMORY_LIMIT: i32 = 124;

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
    /// Memory, CPU seconds, descriptors, file size.
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

    /// What the layers add up to, and whether anything closable is still open.
    ///
    /// Derived from the detail rather than carried as a claim of its own: every
    /// input is a fact this process (or the parent) measured, so the answer is
    /// one rule over those facts instead of a second thing to keep in step. The
    /// line carries the conclusion for a reader, and [`Report::parse`] checks it
    /// against the facts the way it checks the level against the layers.
    pub fn closure(&self) -> Closure {
        let open = |token: &str| self.detail.split(',').any(|fact| fact == token);

        if !(self.layers.limits && self.layers.files && self.layers.network && self.layers.process)
        {
            return Closure::Bounded;
        }
        // Facts, not layers: each is something the platform grants that a layer
        // above cannot take back.
        if open("system=readable") || open("fork=open") || open("ll_gaps=open") {
            return Closure::Bounded;
        }
        if open("writes=own-store") || open("writes=user") {
            return Closure::Bounded;
        }
        Closure::Full
    }

    /// The one-line report the self-test prints and the parent parses.
    pub fn line(&self) -> String {
        let yes = |on: bool| if on { "denied" } else { "open" };
        format!(
            "NFX1-sandbox level={} limits={} files={} network={} process={} closure={} detail={}",
            self.level().as_str(),
            if self.layers.limits { "on" } else { "off" },
            yes(self.layers.files),
            yes(self.layers.network),
            yes(self.layers.process),
            self.closure().as_str(),
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
        let mut closure = None;
        let mut detail = String::new();

        for field in fields.by_ref() {
            let (key, value) = field.split_once('=')?;
            match key {
                "level" => level = Level::parse(value),
                "limits" => layers.limits = parse_switch(value)?,
                "files" => layers.files = parse_denial(value)?,
                "network" => layers.network = parse_denial(value)?,
                "process" => layers.process = parse_denial(value)?,
                "closure" => closure = Closure::parse(value),
                "detail" => {
                    detail = value.to_string();
                }
                // Extra tokens carry diagnostics the parent may want to log
                // (`text_chars=…`), so an unknown name is not an error; the
                // shape of the line still is.
                _ => {}
            }
        }

        // The level and the closure are both redundant with the facts, so a
        // line whose halves disagree is not one of ours.
        let report = Report { layers, detail };
        (level == Some(report.level()) && closure == Some(report.closure())).then_some(report)
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

/// What the confinement adds up to, and whether anything it should close is
/// still open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closure {
    /// Every layer is there *and* nothing this platform could close is open.
    Full,
    /// Every layer is there, and at least one residual is not.
    ///
    /// The residual is in the report's detail, next to the layer it belongs to:
    /// `system=readable` (a Windows container reads the system tree),
    /// `writes=own-store` (it writes its own profile store),
    /// `fork=open` (macOS has to allow `process-fork` for the parsers' thread),
    /// `ll_gaps=open` (a Linux without seccomp, where Landlock's blind spots
    /// are unguarded).
    Bounded,
}

impl Closure {
    pub fn as_str(self) -> &'static str {
        match self {
            Closure::Full => "full",
            Closure::Bounded => "bounded",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "full" => Some(Closure::Full),
            "bounded" => Some(Closure::Bounded),
            _ => None,
        }
    }
}

/// What the child must have before it will read a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Refuse to extract without at least [`Level::Partial`].
    ///
    /// This refuses the `None` level — a child a host could not confine at all
    /// — and accepts a host that has some of the layers. A layer the platform
    /// can only sometimes provide, which on Windows is the AppContainer, is not
    /// a condition here.
    Require,
    /// Refuse to extract without [`Level::Full`]: every layer this platform can
    /// provide, the Windows AppContainer included.
    ///
    /// The difference from [`Policy::Require`] is what a host does when it
    /// cannot provide one of them: `require` reads the document with the layers
    /// that are left, `strict` reads nothing and leaves the documents for a host
    /// that can.
    Strict,
    /// Refuse to extract without a confinement that is also [`Closure::Full`].
    ///
    /// `strict` says every layer this platform can give is there, which is not
    /// the same claim on every platform: the Windows container reads the system
    /// tree, macOS has to allow `process-fork`, and a Linux without seccomp has
    /// Landlock's blind spots. Those are the residuals `closure` names. `sealed`
    /// is for the operator who wants the sandbox the platform can *seal* and
    /// would rather index nothing than index under a residual — which on
    /// Windows and macOS means indexing nothing, because those residuals cannot
    /// be closed without a lower-privileged container (LPAC) and a way to bound
    /// process creation.
    Sealed,
    /// Extract at any level, reporting the shortfall.
    Prefer,
}

impl Policy {
    /// Parse the `index.sandbox` setting.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "require" => Some(Policy::Require),
            "strict" => Some(Policy::Strict),
            "sealed" => Some(Policy::Sealed),
            "prefer" => Some(Policy::Prefer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Require => "require",
            Policy::Strict => "strict",
            Policy::Sealed => "sealed",
            Policy::Prefer => "prefer",
        }
    }

    /// Whether this confinement is enough for this policy.
    ///
    /// Takes the whole report rather than its level because the decision is
    /// made from the report's facts on both sides of the process boundary: the
    /// parent asks it once at startup, and the child asks it again before it
    /// reads a request.
    pub fn accepts(self, report: &Report) -> bool {
        match self {
            Policy::Require => report.level() >= Level::Partial,
            Policy::Strict => report.level() == Level::Full,
            Policy::Sealed => report.level() == Level::Full && report.closure() == Closure::Full,
            Policy::Prefer => true,
        }
    }
}

/// How much of the report is measured by effect.
///
/// Both tiers are honest about what they did — the report says which one
/// produced it — and the difference is only which probes a process can afford.
/// The *decision* is made from the thorough tier (the parent probes once at
/// startup); the per-document child uses the cheap one, where a probe that
/// costs a connection attempt or a started program is left to the probe that
/// runs once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// The effect probes that cost no more than a syscall.
    Cheap,
    /// Everything this platform can measure, however long it takes.
    Thorough,
}

impl Measure {
    pub fn as_str(self) -> &'static str {
        match self {
            Measure::Cheap => "cheap",
            Measure::Thorough => "thorough",
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

/// What the parent did around this process that this process cannot see.
#[derive(Debug, Clone, Copy, Default)]
pub struct External {
    /// The parent wrapped this process in the platform's runner (macOS
    /// `sandbox-exec`), so files, the network and process creation are the
    /// runner's layers rather than this process's own.
    pub runner: bool,
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
pub fn confine(external: External, measure: Measure) -> Report {
    let (mut layers, mut detail) = platform_confine();

    if external.runner {
        // The parent asserts it wrapped this process in a runner whose profile
        // governs all three. Every claim is measured below, so a wrapper that
        // silently did nothing drops the level instead of reporting it.
        layers.files = true;
        layers.network = true;
        layers.process = true;
        detail.push("runner=external".to_string());
    }

    // Every layer that was claimed by a mechanism is now measured, whichever
    // process installed it: the child's own Landlock ruleset, a token the parent
    // created, or a profile a runner applied.
    #[cfg(any(unix, windows))]
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
            if network_is_measured(measure) {
                if network_is_denied() {
                    detail.push("network=measured-denied".to_string());
                } else {
                    layers.network = false;
                    detail.push("network=measured-open".to_string());
                }
            } else {
                // The layer is what the kernel's token says it is; the effect
                // probe that would confirm it costs a connection attempt, and
                // the probe that runs once is where that belongs.
                detail.push("network=installed".to_string());
            }
        }
    }
    // The far edge of the Windows files layer, reported beside the near one: an
    // AppContainer denies what the user's own ACLs grant and permits what `ALL
    // APPLICATION PACKAGES` grants, which is the system tree the child loads
    // from. Which of the two it found is as much of the answer as the layer
    // count.
    #[cfg(windows)]
    if layers.files {
        detail.push(format!("system={}", system_tree()));
    }

    // Process creation is only ever claimed by a mechanism this process cannot
    // see the effect of cheaply: an external runner's profile on macOS, and on
    // Linux the seccomp filter that is installed in the same step that denies
    // `fork`. Both are checked here, in the tier that can afford a fork; the
    // per-document child claims what it installed and leaves the effect to the
    // probe.
    #[cfg(unix)]
    if measure == Measure::Thorough {
        let facts = process_facts();
        detail.push(format!("fork={}", denial_token(facts.fork_denied)));
        detail.push(format!("exec={}", denial_token(facts.exec_denied)));
        if external.runner {
            match facts.exec_denied {
                Some(true) => detail.push("process=measured-denied".to_string()),
                Some(false) => {
                    layers.process = false;
                    detail.push("process=measured-open".to_string());
                }
                // The fork itself was refused, so the exec probe never ran: the
                // runner's claim stands unmeasured rather than being cleared by
                // a measurement that did not happen.
                None => detail.push("process=unmeasured".to_string()),
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
    detail.push(format!("measure={}", measure.as_str()));

    Report {
        layers,
        detail: detail.join(","),
    }
}

/// How a three-valued denial is spelled in the report.
#[cfg(unix)]
fn denial_token(denied: Option<bool>) -> &'static str {
    match denied {
        Some(true) => "denied",
        Some(false) => "open",
        None => "unmeasured",
    }
}

/// Whether this tier runs the network layer's effect probe.
///
/// On unix it always does: binding an ephemeral port needs no peer, cannot fail
/// for any reason other than the sandbox, and returns immediately. On Windows
/// the probe is a connection attempt that a host which neither refuses nor
/// answers makes take its whole timeout, once per document — so there the
/// per-document child claims the layer from the container token the kernel gave
/// it and leaves the connection to the probe that runs once.
#[cfg(any(unix, windows))]
fn network_is_measured(measure: Measure) -> bool {
    #[cfg(windows)]
    {
        measure == Measure::Thorough
    }
    #[cfg(not(windows))]
    {
        let _ = measure;
        true
    }
}

/// Whether reading or writing a path outside the document is refused, measured.
///
/// Both directions: a ruleset that granted writes would still deny every read
/// here, and a parser that can write the host is the thing the layer is for.
/// The temporary directory is the one place a process may write without asking
/// anyone, so a refusal there is the sandbox and nothing else.
///
/// Any one of the read paths failing to open is the denial: `/` always exists,
/// and the other three are readable by every process that is not confined. The
/// macOS profile grants `/` on purpose — the child's working directory is there,
/// and a process that cannot read it is aborted rather than refused — so the
/// measurement rests on the paths beneath it, which no profile of ours grants.
#[cfg(unix)]
fn files_are_denied() -> bool {
    let reads = ["/", "/etc/hostname", "/etc/passwd", "/usr/lib"]
        .iter()
        .any(|path| {
            matches!(
                std::fs::File::open(path),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
            )
        });

    let probe = std::env::temp_dir().join("nanofile-extraction-write-probe");
    let writes = match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        // A temporary directory that is not there says nothing about the
        // sandbox; only a refusal does.
        Err(error) => error.kind() == std::io::ErrorKind::PermissionDenied,
    };

    reads || writes
}

/// Whether reading or writing a path outside the document is refused, measured.
///
/// A Windows AppContainer is not a deny-everything rule: what it leaves readable
/// is the system tree it loads from, which is what `ALL APPLICATION PACKAGES`
/// grants on `Windows` and `Program Files`. So the measurement asks for paths a
/// user can use and a container cannot: listing the directory the child's own
/// image sits in — a per-user install, which does not carry that grant — and
/// creating a file in the user's temporary directory. Either one failing is the
/// denial.
#[cfg(windows)]
fn files_are_denied() -> bool {
    let denied = |error: &std::io::Error| error.kind() == std::io::ErrorKind::PermissionDenied;

    if let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        && let Err(error) = std::fs::read_dir(directory)
        && denied(&error)
    {
        return true;
    }

    let probe = std::env::temp_dir().join("nanofile-extraction-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(error) => denied(&error),
    }
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

/// Whether opening a connection is refused, measured.
///
/// A container with no capabilities cannot open one at all: the Windows Filtering
/// Platform refuses the connect before a packet leaves, and the refusal arrives
/// as the socket error `WSAEACCES` rather than as the timeout a silent drop
/// would give. The address is one a *working* connection answers quickly, so a
/// host whose container is not holding reports `open` instead of waiting: what
/// this asks is whether the connection was forbidden.
///
/// The same error can come from a firewall rule that has nothing to do with the
/// container, which is why this layer is only ever claimed by a process whose own
/// token says it is in one — see `windows::confine`.
#[cfg(windows)]
fn network_is_denied() -> bool {
    /// `WSAEACCES`, from `winerror.h`.
    const WSAEACCES: i32 = 10013;
    /// A well-known address, on the port it answers on.
    const ADDRESS: std::net::SocketAddr = std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)),
        443,
    );
    /// Long enough for a working connection to answer, short enough that a host
    /// which neither connects nor refuses does not stall the child.
    const WAIT: std::time::Duration = std::time::Duration::from_secs(2);

    match std::net::TcpStream::connect_timeout(&ADDRESS, WAIT) {
        Ok(stream) => {
            drop(stream);
            false
        }
        Err(error) => error.raw_os_error() == Some(WSAEACCES),
    }
}

/// Whether the system tree is still readable, which under an AppContainer it is.
///
/// The far edge of the Windows files layer, and a residual rather than a claim:
/// `ALL APPLICATION PACKAGES` is granted across the system tree because that is
/// how a packaged app loads the system it runs on, so a parser that gets loose
/// in the child reads `Windows` even though the user's own files are refused.
/// Reported as a fact and never as a contradiction — a host where this answered
/// `denied` would be stricter than the platform, not broken.
///
/// The candidates are tried in order and the first one that exists decides. The
/// first two are files nothing maps — the legacy `win.ini` and the hosts file —
/// and the last is `ntdll.dll`, which this process has already mapped, so the
/// answer cannot be `absent` on a host where the child runs at all.
#[cfg(windows)]
fn system_tree() -> &'static str {
    const CANDIDATES: [&str; 3] = [
        r"C:\Windows\win.ini",
        r"C:\Windows\System32\drivers\etc\hosts",
        r"C:\Windows\System32\ntdll.dll",
    ];

    let mut refused = false;
    for candidate in CANDIDATES {
        match std::fs::File::open(candidate) {
            Ok(_) => return "readable",
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => refused = true,
            Err(_) => {}
        }
    }
    if refused { "denied" } else { "absent" }
}

/// What trying to fork and to start a program answered.
///
/// Two facts from one probe, because on unix they are one call apart: the copy
/// made by `fork` is where `execve` is tried, and which of the two failed is
/// what tells a profile that denies starting programs from one that denies
/// nothing. `None` means the question could not be asked (the fork was refused,
/// so there was no copy to try `execve` in, or the attempt failed in a way that
/// says nothing about the sandbox).
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct ProcessFacts {
    fork_denied: Option<bool>,
    exec_denied: Option<bool>,
}

/// Whether forking and starting a program are refused, measured.
///
/// The probe is a syscall pair rather than a started program: the version this
/// replaced ran `/usr/bin/true` for every document, which made measuring the
/// sandbox into one of the things the sandbox exists to bound. Nothing of ours
/// runs in the copy — the argument vectors are built before the `fork` and only
/// `execve` and `_exit` are called after it, so no allocation and no lock is
/// touched in a child of a process that may have had threads before.
///
/// `exit 0` from the copy means the helper *ran*, which is the only way to tell
/// a refusal from a program that started and finished.
#[cfg(unix)]
fn process_facts() -> ProcessFacts {
    use std::ffi::CString;

    /// The copy's code for "`execve` was refused for permission".
    const DENIED: libc::c_int = 2;
    /// The copy's code for "`execve` failed for some other reason": the syscall
    /// itself was allowed, so this says nothing about the sandbox.
    const OTHER: libc::c_int = 3;

    let helper = if cfg!(target_os = "macos") {
        "/usr/bin/true"
    } else {
        "/bin/true"
    };
    let Ok(program) = CString::new(helper) else {
        return ProcessFacts {
            fork_denied: None,
            exec_denied: None,
        };
    };
    // The standard streams are inherited rather than set to null: a null stream
    // opens `/dev/null` for writing, which the macOS profile denies, and the
    // probe would then measure that open instead of the `exec` it is about.
    let argv: [*const libc::c_char; 2] = [program.as_ptr(), std::ptr::null()];
    let envp: [*const libc::c_char; 1] = [std::ptr::null()];

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let refused = matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM) | Some(libc::EACCES)
        );
        return ProcessFacts {
            fork_denied: refused.then_some(true),
            exec_denied: None,
        };
    }
    if pid == 0 {
        let code = unsafe {
            libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) {
                DENIED
            } else {
                OTHER
            }
        };
        unsafe { libc::_exit(code) };
    }

    let mut status: libc::c_int = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return ProcessFacts {
            fork_denied: Some(false),
            exec_denied: None,
        };
    }
    let exited = libc::WIFEXITED(status);
    let code = if exited {
        libc::WEXITSTATUS(status)
    } else {
        -1
    };
    ProcessFacts {
        fork_denied: Some(false),
        exec_denied: match code {
            // The helper ran: `execve` was allowed through.
            0 => Some(false),
            DENIED => Some(true),
            _ => None,
        },
    }
}

/// The limits a process can set for itself, and whether each of them took.
///
/// Kept apart rather than added up here because the memory bound is not always
/// one of them: macOS has no address-space limit to set and supplies its own
/// bound instead, so the caller is the one that knows what to add to the rest.
#[cfg(unix)]
struct Clamped {
    address_space: bool,
    cpu: bool,
    descriptors: bool,
    file_size: bool,
    core: bool,
    detail: String,
}

#[cfg(unix)]
impl Clamped {
    /// Whether these limits, plus a memory bound the caller established some
    /// other way, add up to a limits layer.
    ///
    /// Every limit is required, and memory may come from either the address-space
    /// limit or from `memory`. A `limits` layer missing any of them would let a
    /// document cost what the layer exists to bound.
    fn enforced(&self, memory: bool) -> bool {
        self.cpu
            && self.descriptors
            && self.file_size
            && self.core
            && (self.address_space || memory)
    }
}

/// The address-space limit to set, and how the report spells it.
///
/// Linux takes the cap itself. Darwin refuses a limit below what the process
/// already has mapped, so there the limit is the mapped size plus the cap, and
/// the token says `+<cap>`: what is bounded is the growth, which is the same
/// thing the absolute cap bounds on a platform that can state it from zero. The
/// watchdog's `mapped_address_space` is where the base comes from, and a host
/// that will not report it leaves the limit below what is mapped — a refusal the
/// report shows and the fallback covers.
#[cfg(unix)]
fn address_space_limit() -> (u64, String) {
    #[cfg(target_os = "macos")]
    {
        let mapped = watchdog::mapped_address_space().unwrap_or(0);
        (
            mapped.saturating_add(ADDRESS_SPACE_LIMIT),
            format!("+{ADDRESS_SPACE_LIMIT}"),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        (ADDRESS_SPACE_LIMIT, ADDRESS_SPACE_LIMIT.to_string())
    }
}

/// Clamp the limits that bound what a parser can cost.
///
/// Both the soft and the hard value are set: a soft-only clamp can be raised by
/// the very process it is meant to bound. The returned detail lists every limit
/// that took and every one the kernel refused, so a report whose layer is
/// missing says which limit to look at.
///
/// The address-space cap is the one limit Darwin does not have; see the
/// `watchdog` module for what stands in for it there.
#[cfg(unix)]
fn clamp_resources() -> Clamped {
    macro_rules! clamp {
        ($resource:expr, $soft:expr, $hard:expr) => {{
            let limit = libc::rlimit {
                rlim_cur: $soft as libc::rlim_t,
                rlim_max: $hard as libc::rlim_t,
            };
            unsafe { libc::setrlimit($resource, &limit) == 0 }
        }};
    }

    let (address_space_value, address_space_label) = address_space_limit();
    let address_space = clamp!(libc::RLIMIT_AS, address_space_value, address_space_value);
    let cpu = clamp!(libc::RLIMIT_CPU, CPU_SOFT_SECONDS, CPU_HARD_SECONDS);
    let descriptors = clamp!(libc::RLIMIT_NOFILE, NOFILE_LIMIT, NOFILE_LIMIT);
    let file_size = clamp!(libc::RLIMIT_FSIZE, FILE_SIZE_LIMIT, FILE_SIZE_LIMIT);
    // A crash must not write a core file, and the child has no business writing
    // a file at all: this is the belt to the filesystem layer's braces.
    let core = clamp!(libc::RLIMIT_CORE, 0, 0);

    let mut detail = Vec::new();
    detail.push(took(address_space, "as", &address_space_label));
    detail.push(took(
        cpu,
        "cpu",
        &format!("{CPU_SOFT_SECONDS}/{CPU_HARD_SECONDS}"),
    ));
    detail.push(took(descriptors, "nofile", &NOFILE_LIMIT.to_string()));
    detail.push(took(
        file_size,
        "fsize",
        &format!("{}m", FILE_SIZE_LIMIT / (1024 * 1024)),
    ));
    detail.push(took(core, "core", "0"));

    Clamped {
        address_space,
        cpu,
        descriptors,
        file_size,
        core,
        detail: detail.join(","),
    }
}

/// One limit, as the report spells it: `nofile32` when it took, `nofile=refused`
/// when the kernel said no.
#[cfg(unix)]
fn took(set: bool, name: &str, value: &str) -> String {
    if set {
        format!("{name}{value}")
    } else {
        format!("{name}=refused")
    }
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

    /// Every limit is required, and memory may be the address-space limit or the
    /// bound that stands in for it where the kernel does not take one.
    #[cfg(unix)]
    #[test]
    fn a_limits_layer_needs_every_limit_and_one_memory_bound() {
        let clamped = |address_space, cpu, descriptors, file_size, core| Clamped {
            address_space,
            cpu,
            descriptors,
            file_size,
            core,
            detail: String::new(),
        };
        assert!(
            clamped(true, true, true, true, true).enforced(false),
            "the address-space limit is a memory bound"
        );
        assert!(
            clamped(false, true, true, true, true).enforced(true),
            "so is the fallback for a kernel that has none"
        );
        assert!(
            !clamped(false, true, true, true, true).enforced(false),
            "and without either there is no memory bound at all"
        );
        for (cpu, descriptors, file_size, core) in [
            (false, true, true, true),
            (true, false, true, true),
            (true, true, false, true),
            (true, true, true, false),
        ] {
            assert!(
                !clamped(true, cpu, descriptors, file_size, core).enforced(true),
                "every limit is required: {cpu} {descriptors} {file_size} {core}"
            );
        }
    }

    /// Unconfined, the probe has to *say* unconfined. A probe that could only
    /// answer "denied" would certify a sandbox that is not there, which is the
    /// failure the whole report exists to prevent.
    #[cfg(unix)]
    #[test]
    fn the_process_probe_reports_what_it_finds() {
        let facts = process_facts();
        assert_eq!(facts.fork_denied, Some(false), "a test process may fork");
        assert_eq!(
            facts.exec_denied,
            Some(false),
            "and may start a program it can read"
        );
    }

    /// The report names every limit that took and every one the kernel refused,
    /// so a missing limits layer says which limit to look at.
    #[cfg(unix)]
    #[test]
    fn a_limit_is_reported_by_whether_it_took() {
        assert_eq!(took(true, "nofile", "32"), "nofile32");
        assert_eq!(took(true, "cpu", "15/18"), "cpu15/18");
        assert_eq!(took(false, "as", "1073741824"), "as=refused");
    }

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
    fn each_policy_refuses_the_levels_below_it() {
        /// A report whose layers add up to `level`.
        fn report_at(level: Level) -> Report {
            let layers = match level {
                Level::None => Layers {
                    limits: true,
                    ..Layers::default()
                },
                Level::Partial => Layers {
                    limits: true,
                    files: true,
                    ..Layers::default()
                },
                Level::Full => Layers {
                    limits: true,
                    files: true,
                    network: true,
                    process: true,
                },
            };
            Report {
                layers,
                detail: String::new(),
            }
        }

        for level in [Level::None, Level::Partial, Level::Full] {
            let report = report_at(level);
            assert_eq!(report.level(), level, "the fixture is the level it says");
            assert_eq!(
                Policy::Require.accepts(&report),
                level >= Level::Partial,
                "require at {level:?}"
            );
            assert_eq!(
                Policy::Strict.accepts(&report),
                level == Level::Full,
                "strict at {level:?}"
            );
            assert!(Policy::Prefer.accepts(&report), "prefer at {level:?}");
        }
        assert_eq!(Policy::parse(" require "), Some(Policy::Require));
        assert_eq!(Policy::parse("strict"), Some(Policy::Strict));
        assert_eq!(Policy::parse("sealed"), Some(Policy::Sealed));
        assert_eq!(Policy::parse("PREFER"), Some(Policy::Prefer));
        assert_eq!(Policy::parse("off"), None);

        // `sealed` is `strict` plus the closure, so it is the one policy that
        // cares whether a residual is open.
        let mut report = report_at(Level::Full);
        assert!(Policy::Strict.accepts(&report));
        assert!(Policy::Sealed.accepts(&report));
        report.detail = "system=readable".to_string();
        assert!(Policy::Strict.accepts(&report), "the layers are unchanged");
        assert!(
            !Policy::Sealed.accepts(&report),
            "a residual is what sealed refuses"
        );
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
        // A line missing the closure, or claiming one the facts do not support,
        // is a line this process did not write.
        let closed = "NFX1-sandbox level=full limits=on files=denied network=denied \
                      process=denied closure={closure} detail=x";
        assert!(Report::parse(&closed.replace("{closure}", "full")).is_some());
        assert!(Report::parse(&closed.replace("{closure}", "bounded")).is_none());
        assert!(
            Report::parse(
                "NFX1-sandbox level=full limits=on files=denied network=denied process=denied \
                 detail=x"
            )
            .is_none(),
            "a line from before the closure existed is not a report"
        );
    }

    /// The residuals are what `sealed` is about: a layer being there is not the
    /// same as nothing being open, and the difference is a fact in the detail.
    #[test]
    fn a_residual_bounds_an_otherwise_full_report() {
        let report = |detail: &str| Report {
            layers: Layers {
                limits: true,
                files: true,
                network: true,
                process: true,
            },
            detail: detail.to_string(),
        };
        assert_eq!(report("seccomp=1").closure(), Closure::Full);
        for residual in [
            "system=readable",
            "writes=own-store",
            "writes=user",
            "fork=open",
            "ll_gaps=open",
        ] {
            assert_eq!(
                report(&format!("ll_scoped=on,{residual}")).closure(),
                Closure::Bounded,
                "{residual} is a residual"
            );
        }
        // `system=denied` is a host stricter than the platform, not a residual.
        assert_eq!(report("system=denied").closure(), Closure::Full);

        // A missing layer is bounded whatever the facts say.
        let partial = Report {
            layers: Layers {
                limits: true,
                files: true,
                ..Layers::default()
            },
            detail: "seccomp=1".to_string(),
        };
        assert_eq!(partial.closure(), Closure::Bounded);
    }
}
