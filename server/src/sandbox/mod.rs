//! Confinement for every child process that reads attacker-chosen bytes.
//!
//! Documents, images and media are all parsed or decoded by a separate process
//! ([`worker`]) that confines itself before it reads a single byte. The
//! protections, and what each is for:
//!
//! | Item | What it bounds | Linux | macOS | Windows |
//! |---|---|---|---|---|
//! | limits | memory, CPU seconds, descriptors, file size | `RLIMIT_AS` | `setrlimit` (mapped space plus the cap) | Job Object memory cap |
//! | files | reading or writing any path | Landlock, zero grants (the helper, its interpreter and the source for media) | Seatbelt profile | AppContainer |
//! | network | creating a socket | seccomp denylist, Landlock TCP rights | Seatbelt profile | AppContainer with no capabilities |
//! | process | `exec`, `fork`, extra processes | seccomp denylist (`exec` allowed only for the media helper and its loader) | Seatbelt profile, `exec` only (`process-fork` is allowed) | Job active-process limit, plus the kernel's child-process policy |
//!
//! The process item is the one the media profile cannot have. That profile
//! exists to start a program — that is what the helper is — and every platform
//! starts one the same way, by copying the process first, so a media child that
//! denied both would deny its own job. Its report therefore says the item is
//! *not* in place, and the grade is `partial`: what bounds the helper is the
//! files layer (only the helper and the interpreter the kernel runs before it
//! may be executed) and what bounds the copies is the CPU and wall-clock limit
//! the parent enforces, which is a note and not a fourth protection. The
//! document and image profiles deny both outright and do grade `full`.
//!
//! Memory is the one limit each platform has to be told about differently. Linux
//! takes the cap as an address-space limit outright; Windows caps committed
//! memory with a Job Object; Darwin refuses a limit below what a process already
//! has mapped, so there the limit is the mapped size plus the cap — the same
//! bound, stated from where the process already is. The report says which of
//! them took rather than which were attempted.
//!
//! # Levels, items and the two settings
//!
//! There are exactly three grades, and an admin sees them by name:
//!
//! * [`Level::Full`] (完整) — every protection this platform can provide is in
//!   place: resource limits, files, network and process.
//! * [`Level::Partial`] (部分) — resource limits *and* the files layer, with at
//!   least one of network and process missing; the settings page names which.
//! * [`Level::None`] (无) — anything less, including a host that denies the
//!   network and the processes but lets the parser read every path.
//!
//! The files layer is a necessary condition of `partial`, not one of three ways
//! to reach it: a parser that can read the host's files is the exposure this
//! module exists to close, and the hosts that end up there — a kernel without
//! Landlock, a Windows launch that fell back to a token with no container — are
//! exactly the ones a grade must not call adequate.
//!
//! Two settings govern the sandbox, and both live on the admin's "Sandbox" page:
//! `sandbox.enabled` is the master switch over every feature that parses
//! untrusted bytes, and `sandbox.min_level` is the grade the host must reach
//! before those features run at all. When the switch is off, or the host grades
//! below the minimum, the features are *disabled* — there is no in-process path
//! to fall back to. [`Requirement`] is the pair, and [`Refusal`] says which of
//! the two refused. `sandbox.min_level = "none"` is the one value that accepts a
//! host without the files layer; the page says so in those words.
//!
//! # What a grade does not say
//!
//! [`Level::Full`] means "every protection this platform can provide", and the
//! three platforms provide materially different things under that name: Linux
//! denies every path and every socket through Landlock and seccomp, the Windows
//! container denies the user's own files and the network but reads the system
//! tree it loads from, and macOS's profile is a deny-by-default text with
//! `process-fork` allowed because the parsers' thread needs it. Those are the
//! residuals [`Report::notes`] carries — `fork=`, `helper=`, `system=`,
//! `writes=`, `metadata=`, `ll_gaps=` — and they are *notes on the item they
//! weaken*, not a fourth grade: an operator reads "进程创建: 有（注：macOS 允许
//! fork）" rather than having to understand a second, stronger scale.
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
//! writes for itself: a low-box access check needs an ACE for the package the
//! process is checked against, over and above whatever the user's own ACLs
//! grant, so what the child can still reach is the system tree it loads from.
//! Which principals supply that access depends on the container — with the
//! Windows 11 opt-out it is not `ALL APPLICATION PACKAGES` — so the child
//! measures both edges of the window and reports the far one as `system=`,
//! without letting it clear the layer: a parser that gets loose in the child
//! reads `Windows` whether or not that is recorded, and what the report can do is
//! say so next to the fact that the user's own files are refused.
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

pub mod jobs;
#[cfg(target_os = "linux")]
mod linux;
pub mod worker;
// The Seatbelt profile is built on every platform so its tests can check the
// text, but only macOS ever runs under it.
/// How macOS states the memory bound: the space already mapped plus the cap.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod seatbelt;
// Read on the two platforms that run a helper, and compiled for real by the
// aarch64 syscall check in CI, which copies this file beside the Linux module.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod shebang;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) use shebang::interpreter as shebang_interpreter;
#[cfg(target_os = "windows")]
mod windows;
/// Start the child the way Windows has to: a restricted token and an AppContainer
/// cannot be applied to a running process, so they are part of creation. The
/// token-only and unrestricted starts are the weaker creations the probe walks
/// down to when a stronger one reports nothing.
#[cfg(target_os = "windows")]
pub(super) use windows::{Child as WindowsChild, spawn, spawn_token_only, spawn_unrestricted};

/// Whether the child runs in a less privileged container, where the platform
/// has one.
///
/// Windows 11 can opt a container out of `ALL APPLICATION PACKAGES` with the
/// `WIN://NOALLAPPPKG` security attribute, and the answer here is what the last
/// launch actually got — `false` on Windows 10, where the attribute does not
/// exist. It is a fact about the launch, not a grade: the system tree stays
/// readable either way, because the files a low-box process loads carry ACEs for
/// `ALL RESTRICTED APPLICATION PACKAGES` as well. That is a property of the
/// Windows build rather than a documented invariant, which is why the note is
/// decided by the child's own `system=` measurement instead of by this.
pub fn lpac() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::lpac()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Whether a launch asked for the less privileged container.
pub fn lpac_attempted() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::lpac_attempted()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Stop asking for the less privileged container, for this process's life.
///
/// The attribute is accepted at creation, so a container whose loader cannot
/// start inside the opt-out fails *after* `CreateProcess` succeeds. When that
/// happens the parent stops asking and starts over with the plain container:
/// the confinement is kept, and only the opt-out is given up.
pub fn disable_lpac() {
    #[cfg(target_os = "windows")]
    {
        windows::disable_lpac();
    }
}

/// Why the child was not started in an AppContainer, where the platform has one.
///
/// `None` when the container was applied, was never asked for, or cannot exist on
/// this platform. The probe prints what this says, because the child can only
/// report that its token is not one, not why. Owned rather than `'static`: it is
/// the reasons the *last* launch recorded, joined, and a launch that succeeds
/// after an earlier one failed clears them.
pub fn container_shortfall() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows::shortfall()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

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
    /// Resource limits plus the files layer, and not everything else.
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

/// The protections actually in force, one flag per item the admin page lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Protections {
    /// Memory, CPU seconds, descriptors, file size.
    pub limits: bool,
    /// Paths cannot be read or written.
    pub files: bool,
    /// Sockets cannot be created.
    pub network: bool,
    /// Programs cannot be started and processes cannot be multiplied.
    ///
    /// Never set for [`Profile::Media`], which starts the helper and the copy
    /// that leads to it by definition: the page then says the item is missing
    /// for that profile rather than claiming a bound the profile cannot have.
    pub process: bool,
}

impl Protections {
    /// What these items add up to.
    ///
    /// [`Level::Full`] needs all four. [`Level::Partial`] needs resource limits
    /// *and* the files layer. Anything else is [`Level::None`].
    ///
    /// The files layer is what makes `partial` a grade worth having rather than
    /// the weaker of two grades. What a document parser can do to the host is
    /// bounded by what it can read: a host that confines the network and the
    /// processes but not the paths has a parser that can still read every file
    /// the server's user can, and that is the exposure this whole module exists
    /// to close. The two ways a host lands there are both real and both silent —
    /// a kernel without Landlock (or a container runtime that does not allow the
    /// syscalls), and a Windows launch that could not produce an AppContainer and
    /// fell back to a token that bounds nothing about files. So `network` and
    /// `process` are refinements on top of a files-confined host, never
    /// substitutes for one, and a host without the files layer grades `none`
    /// whatever else it denies. `sandbox.min_level` then refuses it by default,
    /// and `none` is the one setting that accepts it — which is the explicit
    /// choice, not an accident.
    pub fn level(self) -> Level {
        if self.limits && self.files && self.network && self.process {
            Level::Full
        } else if self.limits && self.files {
            Level::Partial
        } else {
            Level::None
        }
    }

    /// Every item, in the order the settings page lists them.
    ///
    /// The name is the token the report line carries, so the page and the line
    /// cannot drift apart.
    pub fn items(self) -> [(&'static str, bool); 4] {
        [
            ("limits", self.limits),
            ("files", self.files),
            ("network", self.network),
            ("process", self.process),
        ]
    }
}

/// Which pipeline a confined child runs, and therefore what it may reach.
///
/// The profile is what the child is told on its command line *before* it reads
/// a request, because a grant Landlock or a Seatbelt profile can only express
/// has to be installed before the bytes it applies to are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Document text extraction: no file, no socket, no program.
    Documents,
    /// Image decode, EXIF and avatar processing: the same shape as documents.
    Images,
    /// Media (ffmpeg) frame extraction: the one profile that may execute the
    /// configured helper, and read the one scratch file it is handed.
    Media,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Documents => "documents",
            Profile::Images => "images",
            Profile::Media => "media",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "documents" => Some(Profile::Documents),
            "images" => Some(Profile::Images),
            "media" => Some(Profile::Media),
            _ => None,
        }
    }

    /// Whether this profile may execute the configured helper.
    ///
    /// Only the media profile: ffmpeg is an external program, so it is the one
    /// child a request may start. Everything else is denied the `exec` outright,
    /// and this profile's own `exec` is narrowed to the one binary the parent
    /// names (see `parse` of the profile's grants in `linux`/`seatbelt`).
    pub fn runs_helper(self) -> bool {
        matches!(self, Profile::Media)
    }

    /// Every profile, for probes and tests.
    pub const ALL: [Profile; 3] = [Profile::Documents, Profile::Images, Profile::Media];
}

/// What the child established, and how.
#[derive(Debug, Clone)]
pub struct Report {
    /// Which pipeline this child confined itself for.
    pub profile: Profile,
    pub protections: Protections,
    /// Mechanism tokens and measurements, comma-joined and whitespace-free.
    pub detail: String,
}

impl Report {
    pub fn level(&self) -> Level {
        self.protections.level()
    }

    /// The residuals the detail carries, as note keys for the settings page.
    ///
    /// Derived from the detail rather than carried as claims of their own: every
    /// token is a fact this process (or the parent) measured, and a token no
    /// backend emits simply produces no note. They are *notes* — the grade is
    /// decided from [`Protections`] alone, so a wording change here can never
    /// weaken a decision.
    ///
    /// The list is what the page has to say beyond "which item is missing". An
    /// item can be in place and still be narrower than its name suggests — the
    /// Windows container reads the system tree, the media worker may read the
    /// libraries beside its helper — and a fact only the parent saw (which
    /// creation was used, why the container was not) reaches here the same way,
    /// because the parent appends it to the report before the page reads it.
    pub fn notes(&self) -> Vec<&'static str> {
        let has = |token: &str| self.detail.split(',').any(|fact| fact == token);
        let mut notes = Vec::new();
        if has("fork=open") {
            notes.push("fork");
        }
        if has("system=readable") {
            notes.push("system_tree");
        }
        if has("writes=store") {
            notes.push("writes");
        }
        if has("writes=user") {
            notes.push("writes_user");
        }
        if has("metadata=open") {
            notes.push("metadata");
        }
        if has("ll_gaps=open") {
            notes.push("ll_gaps");
        }
        if has("helper=allowed") {
            notes.push("helper");
        }
        // How much the media helper may read, which is wider than "the helper and
        // its loader": the libraries beside it in every case, and on macOS the
        // package-manager trees a packaged build links against.
        if has("helper_scope=trees") {
            notes.push("helper_trees");
        } else if has("helper_scope=libs") {
            notes.push("helper_libs");
        }
        // The creation the parent settled for, when it is weaker than the one
        // this host can give, and why the strongest one did not work.
        if has("rung=token") || has("rung=plain") {
            notes.push("container_plain");
        }
        if self
            .detail
            .split(',')
            .any(|fact| fact.starts_with("container_refused="))
        {
            notes.push("container_refused");
        }
        if has("lpac=off") {
            notes.push("lpac_off");
        }
        // The media profile starts its helper by copying the process, and on the
        // two platforms without a Job Object nothing bounds how many copies there
        // may be. The item is already reported as missing for this profile; this
        // is the note that says what does bound them instead.
        if has("media_process=unbounded") {
            notes.push("media_process");
        }
        notes
    }

    /// The one-line report the self-test prints and the parent parses.
    ///
    /// Every field of the line is whitespace-separated, so the detail is made
    /// whitespace-free here rather than trusted to be: a token that carries an
    /// error string — an OS error is `Permission denied (os error 13)` — would
    /// otherwise split the detail across fields and make the whole line
    /// unreadable, which the parent treats as a probe that never reported. That
    /// is the difference between "no ffmpeg on this host" and "the media profile
    /// is not available at all", and only one of them is true.
    pub fn line(&self) -> String {
        let yes = |on: bool| if on { "denied" } else { "open" };
        let detail = self.detail.replace([' ', '\t', '\n'], "_");
        format!(
            "NFS2-sandbox profile={} level={} limits={} files={} network={} process={} detail={}",
            self.profile.as_str(),
            self.level().as_str(),
            if self.protections.limits { "on" } else { "off" },
            yes(self.protections.files),
            yes(self.protections.network),
            yes(self.protections.process),
            detail
        )
    }

    /// Parse a line produced by [`Report::line`].
    ///
    /// Returns `None` for anything that is not one — including a line from
    /// before the profile existed — which is what makes a truncated or foreign
    /// line a failure rather than a wrong level.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        let mut fields = line.strip_prefix("NFS2-sandbox ")?.split(' ');
        let mut profile = None;
        let mut protections = Protections::default();
        let mut level = None;
        let mut detail = String::new();

        for field in fields.by_ref() {
            let (key, value) = field.split_once('=')?;
            match key {
                "profile" => profile = Profile::parse(value),
                "level" => level = Level::parse(value),
                "limits" => protections.limits = parse_switch(value)?,
                "files" => protections.files = parse_denial(value)?,
                "network" => protections.network = parse_denial(value)?,
                "process" => protections.process = parse_denial(value)?,
                "detail" => {
                    detail = value.to_string();
                }
                // Extra tokens carry diagnostics the parent may want to log
                // (`text_chars=…`), so an unknown name is not an error; the
                // shape of the line still is.
                _ => {}
            }
        }

        // The level is redundant with the items, so a line whose halves disagree
        // is not one of ours.
        let report = Report {
            profile: profile?,
            protections,
            detail,
        };
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
///
/// The master switch and the minimum grade, as the admin set them. Both live on
/// the settings page's Sandbox section; the pair crosses the process boundary so
/// the child can refuse on its own, before it reads a byte, exactly as the
/// parent refused to start it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// `sandbox.enabled`: the master switch over every parsing feature.
    pub enabled: bool,
    /// `sandbox.min_level`: the grade the host must reach.
    pub min_level: Level,
}

impl Default for Requirement {
    /// The shipped default: sandbox on, and a host that gives resource limits
    /// plus the files layer. A host below that has the features disabled rather
    /// than parsed with the server user's own read access to every path.
    fn default() -> Self {
        Self {
            enabled: true,
            min_level: Level::Partial,
        }
    }
}

impl Requirement {
    pub fn new(enabled: bool, min_level: Level) -> Self {
        Self { enabled, min_level }
    }

    /// The requirement the `[sandbox]` section states.
    ///
    /// An unparseable `min_level` falls back to the shipped default rather than
    /// to "none": a typo must not quietly disable the confinement.
    pub fn from_config(enabled: bool, min_level: &str) -> Self {
        Self::new(enabled, Level::parse(min_level).unwrap_or(Level::Partial))
    }

    /// Whether this confinement is enough, and if not, which half refused.
    ///
    /// Takes the whole report rather than its level because the answer is about
    /// the report's items on both sides of the process boundary: the parent asks
    /// it once at startup, and the child asks it again before it reads a request.
    pub fn accepts(&self, report: &Report) -> Result<(), Refusal> {
        if !self.enabled {
            return Err(Refusal::Disabled);
        }
        let got = report.level();
        if got < self.min_level {
            return Err(Refusal::BelowMinimum {
                got,
                min: self.min_level,
            });
        }
        Ok(())
    }
}

/// Why a confinement was refused.
///
/// The two cases are not the same problem: the switch is the admin's own choice
/// and leaves the work undone, while a host below the minimum is an environment
/// problem the backfill retries once the host can confine the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `sandbox.enabled` is false.
    Disabled,
    /// The host grades below `sandbox.min_level`.
    BelowMinimum { got: Level, min: Level },
}

impl Refusal {
    /// The refusal as a sentence, for a log line or an error.
    pub fn reason(self) -> String {
        match self {
            Refusal::Disabled => "the sandbox is switched off (`sandbox.enabled`)".to_string(),
            Refusal::BelowMinimum { got, min } => format!(
                "this host gives `{}` confinement and `sandbox.min_level` is `{}`",
                got.as_str(),
                min.as_str()
            ),
        }
    }
}

/// Whether a profile's features may run at all on this host.
///
/// `Ok` only when the switch is on, the host grades at or above the minimum,
/// and the child has been probed ready. A caller that has no unconfined path to
/// fall back to asks this before it does anything, and reports the reason when
/// the answer is no.
pub fn available(profile: Profile) -> Result<(), String> {
    match worker::status(profile) {
        worker::Status::Ready(_) => Ok(()),
        worker::Status::Unavailable(why) => Err(why),
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
pub fn runner(exe: &Path, profile: Profile, grants: Grants) -> Option<Runner> {
    #[cfg(target_os = "macos")]
    {
        Some(Runner {
            program: seatbelt::PROGRAM,
            args: seatbelt::args(exe, profile, grants),
            external_confinement: true,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (exe, profile, grants);
        None
    }
}

/// Paths a profile may reach beyond the deny-all default.
///
/// Only the media profile has any: ffmpeg is an external program and needs its
/// own image and its libraries readable, and the file it decodes is a scratch
/// copy the parent wrote. Every other profile gets the empty set, which is the
/// deny-all grant the document and image jobs rely on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Grants<'a> {
    /// The configured helper binary the media profile may execute and load.
    pub helper: Option<&'a Path>,
    /// The scratch source the media profile may read.
    pub source: Option<&'a Path>,
}

impl Grants<'_> {
    /// Whether this profile reaches nothing beyond the default.
    pub fn is_empty(&self) -> bool {
        self.helper.is_none() && self.source.is_none()
    }
}

/// Apply every protection this platform offers to the current process.
///
/// Must be called on the child's only thread and before the request is read:
/// Landlock and seccomp are inherited by threads created afterwards, and the
/// parsers create one. `profile` says what the child is allowed to reach, and is
/// carried into the report so a probe of one profile can never be read as the
/// answer for another. `grants` is what that profile may reach, and is empty for
/// every profile that may reach nothing.
pub fn confine(external: External, profile: Profile, grants: Grants, measure: Measure) -> Report {
    let (mut protections, mut detail) = platform_confine(profile, grants);

    if external.runner {
        // The parent asserts it wrapped this process in a runner whose profile
        // governs all three. Every claim is measured below, so a wrapper that
        // silently did nothing drops the level instead of reporting it.
        protections.files = true;
        protections.network = true;
        protections.process = true;
        detail.push("runner=external".to_string());
    }

    // Every protection that was claimed by a mechanism is now measured,
    // whichever process installed it: the child's own Landlock ruleset, a token
    // the parent created, or a profile a runner applied.
    #[cfg(any(unix, windows))]
    {
        if protections.files {
            if files_are_denied() {
                detail.push("files=measured-denied".to_string());
            } else {
                protections.files = false;
                detail.push("files=measured-open".to_string());
            }
        }
        if protections.network {
            if network_is_measured(measure) {
                if network_is_denied() {
                    detail.push("network=measured-denied".to_string());
                } else {
                    protections.network = false;
                    detail.push("network=measured-open".to_string());
                }
            } else {
                // The protection is what the kernel's token says it is; the
                // effect probe that would confirm it costs a connection attempt,
                // and the probe that runs once is where that belongs.
                detail.push("network=installed".to_string());
            }
        }
    }
    // The far edge of the Windows files protection, reported beside the near
    // one: a low-box check needs the package the process is checked against, over
    // and above the user's own ACLs, so the system tree the child loads from
    // stays readable. Which of the two it found is as much of the answer as the
    // item, and the mechanism behind the grant is left to the platform docs
    // rather than asserted here — it differs with the container.
    #[cfg(windows)]
    if protections.files {
        detail.push(format!("system={}", system_tree()));
    }

    // Process creation is only ever claimed by a mechanism this process cannot
    // see the effect of cheaply: an external runner's profile on macOS, and on
    // Linux the seccomp filter that is installed in the same step that denies
    // `fork`. Both are checked here, in the tier that can afford a fork; the
    // per-request child claims what it installed and leaves the effect to the
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
                    protections.process = false;
                    detail.push("process=measured-open".to_string());
                }
                // The fork itself was refused, so the exec probe never ran: the
                // runner's claim stands unmeasured rather than being cleared by
                // a measurement that did not happen.
                None => detail.push("process=unmeasured".to_string()),
            }
        }
    }

    // The helper is a grant the parent made before this process existed, and on
    // the platforms that apply one as creation rather than as a rule this side
    // cannot inspect, the only way to check it is to try: opening the file is
    // what a profile or an ACL that does not reach it looks like from in here.
    // The effect is `parse=`; this is the grant that leads to it.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if profile.runs_helper()
        && let Some(helper) = grants.helper
        && let Err(error) = std::fs::File::open(helper)
    {
        detail.push(format!("helper-grant-refused({error})"));
    }

    // The media profile's process item, and the helper it exists to start.
    //
    // The item is cleared whatever the platform installed. This profile starts
    // a program by definition, and every way of starting one copies the process
    // first, so it cannot claim what the document and image profiles claim; the
    // grade that follows is `partial`, and the settings page says which item is
    // missing rather than implying a bound the profile cannot have. The `fork`
    // fact the tier above measured is the note that goes with it.
    //
    // The helper is a *fact* rather than a protection: the grant took unless a
    // platform said it did not, and the effect — the helper actually running —
    // is what the probe reports as `parse=`.
    if profile.runs_helper() {
        protections.process = false;
        if helper_is_granted(grants.helper.is_some(), &detail) {
            detail.push("helper=allowed".to_string());
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
        profile,
        protections,
        detail: detail.join(","),
    }
}

/// Whether the platform said the helper's grant took.
///
/// Three tokens can say it did not, and they are the platforms' own words: a
/// ruleset that counted rules but not the helper's (`helper-rule-refused`), a
/// Linux grant with nothing in it (`helper_grants=0`), and a Windows ACL that
/// could not be written (`helper-grant-refused…`). Without a complaint, the
/// grant is what the profile was built with.
fn helper_is_granted(helper: bool, detail: &[String]) -> bool {
    helper
        && !detail.iter().any(|fact| {
            fact == "helper_grants=0"
                || fact == "helper-rule-refused"
                || fact.starts_with("helper-grant-refused")
        })
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
/// Any one of the read paths failing to open is the denial. The candidates are
/// only paths no profile of ours grants, which is why `/` is not among them: the
/// macOS profile grants it on purpose — the child's working directory is there,
/// and a process that cannot read it is aborted rather than refused — so a
/// candidate that is granted could never produce the denial this is looking for
/// and would only make the read half look measured when it was not. `/usr/lib`
/// is granted by that profile for the same reason. What is left is two files
/// every unconfined process can read and no profile grants.
#[cfg(unix)]
fn files_are_denied() -> bool {
    let reads = ["/etc/hostname", "/etc/passwd"].iter().any(|path| {
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
/// the packages a low-box process is checked against are granted across the
/// system tree, because that is how a packaged app loads the system it runs on,
/// so a parser that gets loose in the child reads `Windows` even though the
/// user's own files are refused. Which package supplies the grant is left to the
/// platform: the Windows 11 opt-out makes the check ignore
/// `ALL APPLICATION PACKAGES`, and the readable set then comes from the ACEs a
/// different principal carries. Reported as a fact and never as a contradiction —
/// a host where this answered `denied` would be stricter than the platform, not
/// broken.
///
/// The candidates are tried in order and the first one that exists decides. The
/// first two are files nothing maps — the legacy `win.ini` and the hosts file —
/// and the last is `ntdll.dll`, which this process has already mapped, so the
/// answer cannot be `absent` on a host where the child runs at all.
///
/// `SystemRoot` is where they are read from rather than a hard-coded `C:`, which
/// is only the usual install: a host whose Windows lives on another volume would
/// otherwise answer `absent` for every candidate and report a residual it never
/// measured.
#[cfg(windows)]
fn system_tree() -> &'static str {
    let root = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    let candidates: [std::path::PathBuf; 3] = [
        root.join("win.ini"),
        root.join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts"),
        root.join("System32").join("ntdll.dll"),
    ];

    let mut refused = false;
    for candidate in &candidates {
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
    /// The errors a confinement layer refuses an `exec` with.
    ///
    /// `EPERM` is what a seccomp filter returns and what a Seatbelt denial looks
    /// like, and `EACCES` is what Landlock returns — Landlock is the files layer,
    /// so on Linux a refused `exec` is a permission error on the path and not the
    /// filter's. Both are the sandbox saying no; treating only one of them as a
    /// denial reported the other as "could not be measured", which is what the
    /// Linux reports used to say about an `exec` that never happened.
    const DENIALS: [Option<libc::c_int>; 2] = [Some(libc::EPERM), Some(libc::EACCES)];

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
            if DENIALS.contains(&std::io::Error::last_os_error().raw_os_error()) {
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
/// The watchdog's `mapped_address_space` is where the base comes from, and a host
/// that will not report it leaves the limit below what is mapped — a refusal the
/// report shows and the fallback covers.
#[cfg(unix)]
fn address_space_limit() -> (u64, String) {
    #[cfg(target_os = "macos")]
    {
        let mapped = macos::mapped_address_space().unwrap_or(0);
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
fn platform_confine(profile: Profile, grants: Grants) -> (Protections, Vec<String>) {
    linux::confine(profile, grants)
}

#[cfg(target_os = "macos")]
fn platform_confine(profile: Profile, grants: Grants) -> (Protections, Vec<String>) {
    seatbelt::confine(profile, grants)
}

#[cfg(target_os = "windows")]
fn platform_confine(profile: Profile, grants: Grants) -> (Protections, Vec<String>) {
    let _ = grants;
    windows::confine(profile)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_confine(profile: Profile, grants: Grants) -> (Protections, Vec<String>) {
    let _ = (profile, grants);
    (
        Protections::default(),
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
    fn a_level_needs_limits_and_the_files_layer() {
        assert_eq!(Protections::default().level(), Level::None);
        assert_eq!(
            Protections {
                limits: true,
                ..Protections::default()
            }
            .level(),
            Level::None,
            "resource limits alone are not confinement"
        );
        assert_eq!(
            Protections {
                limits: true,
                files: true,
                ..Protections::default()
            }
            .level(),
            Level::Partial,
            "the files layer is enough for partial on its own"
        );
        // The two hosts that reach the network and process items without the
        // files layer: a kernel without Landlock, and a Windows launch that fell
        // back to a token with no container. Neither may be called partial —
        // each has a parser that can read every path the server user can.
        for missing_files in [
            Protections {
                limits: true,
                files: false,
                network: true,
                process: true,
            },
            Protections {
                limits: true,
                files: false,
                network: true,
                process: false,
            },
            Protections {
                limits: true,
                files: false,
                network: false,
                process: true,
            },
        ] {
            assert_eq!(
                missing_files.level(),
                Level::None,
                "no files layer is no grade: {missing_files:?}"
            );
        }
        // Everything but the process item, which is the media profile's shape.
        assert_eq!(
            Protections {
                limits: true,
                files: true,
                network: true,
                process: false,
            }
            .level(),
            Level::Partial
        );
        assert_eq!(
            Protections {
                limits: true,
                files: true,
                network: true,
                process: true,
            }
            .level(),
            Level::Full
        );
        assert_eq!(
            Protections {
                files: true,
                ..Protections::default()
            }
            .level(),
            Level::None,
            "confinement without resource limits is not a level"
        );
        // The page lists exactly these items, in this order.
        assert_eq!(
            Protections {
                limits: true,
                files: true,
                network: false,
                process: true,
            }
            .items(),
            [
                ("limits", true),
                ("files", true),
                ("network", false),
                ("process", true)
            ]
        );
    }

    #[test]
    fn the_requirement_refuses_the_switch_and_the_grades_below_it() {
        /// A report whose items add up to `level`.
        fn report_at(level: Level) -> Report {
            let items = match level {
                Level::None => Protections {
                    limits: true,
                    ..Protections::default()
                },
                Level::Partial => Protections {
                    limits: true,
                    files: true,
                    ..Protections::default()
                },
                Level::Full => Protections {
                    limits: true,
                    files: true,
                    network: true,
                    process: true,
                },
            };
            Report {
                profile: Profile::Documents,
                protections: items,
                detail: String::new(),
            }
        }

        let accepts = |enabled, min_level, level| {
            Requirement::new(enabled, min_level)
                .accepts(&report_at(level))
                .is_ok()
        };

        // The switch refuses whatever the host gives.
        for level in [Level::None, Level::Partial, Level::Full] {
            assert_eq!(
                Requirement::new(false, Level::None).accepts(&report_at(level)),
                Err(Refusal::Disabled),
                "the switch is off at {level:?}"
            );
        }

        // `min_level` refuses the grades below it and accepts the rest.
        for min in [Level::None, Level::Partial, Level::Full] {
            for level in [Level::None, Level::Partial, Level::Full] {
                assert_eq!(
                    accepts(true, min, level),
                    level >= min,
                    "min {min:?} against a host that gives {level:?}"
                );
            }
        }
        assert_eq!(
            Requirement::new(true, Level::Full).accepts(&report_at(Level::Partial)),
            Err(Refusal::BelowMinimum {
                got: Level::Partial,
                min: Level::Full
            })
        );

        // A residual in the detail never changes the decision: it is a note on
        // an item, not a grade.
        let mut full = report_at(Level::Full);
        assert!(Requirement::default().accepts(&full).is_ok());
        full.detail = "system=readable,fork=open".to_string();
        assert!(Requirement::default().accepts(&full).is_ok());

        assert!(Requirement::default().enabled);
        assert_eq!(Requirement::default().min_level, Level::Partial);
        assert_eq!(
            Requirement::from_config(true, "typo").min_level,
            Level::Partial,
            "an unreadable level falls back to the default, never to `none`"
        );
        assert_eq!(
            Requirement::from_config(true, "full").min_level,
            Level::Full
        );
    }

    #[test]
    fn a_report_round_trips_through_its_line() {
        let report = Report {
            profile: Profile::Documents,
            protections: Protections {
                limits: true,
                files: true,
                network: true,
                process: true,
            },
            detail: "landlock_abi=6,seccomp=on".to_string(),
        };
        let line = report.line();
        assert!(
            line.starts_with("NFS2-sandbox profile=documents level=full "),
            "{line}"
        );
        let parsed = Report::parse(&line).expect("parses");
        assert_eq!(parsed.profile, Profile::Documents);
        assert_eq!(parsed.protections, report.protections);
        assert_eq!(parsed.level(), Level::Full);
        assert_eq!(parsed.detail, report.detail);

        // The media profile's shape: the process item is the one it cannot have,
        // so its line says `open` and grades `partial`, and it still round-trips.
        let media = Report {
            profile: Profile::Media,
            protections: Protections {
                limits: true,
                files: true,
                network: true,
                process: false,
            },
            detail: "fork=open,helper=allowed".to_string(),
        };
        let line = media.line();
        assert!(
            line.starts_with(
                "NFS2-sandbox profile=media level=partial limits=on files=denied \
                 network=denied process=open detail="
            ),
            "{line}"
        );
        let parsed = Report::parse(&line).expect("parses");
        assert_eq!(parsed.protections, media.protections);
        assert_eq!(parsed.level(), Level::Partial);
        assert_eq!(parsed.notes(), ["fork", "helper"]);
    }

    /// A detail token that carries an error string still produces a line the
    /// parent can read: the fields are whitespace-separated, and an OS error is
    /// not whitespace-free.
    #[test]
    fn a_detail_with_an_error_string_is_still_one_field() {
        let report = Report {
            profile: Profile::Media,
            protections: Protections {
                limits: true,
                files: true,
                network: true,
                process: false,
            },
            detail: "parse=media-unavailable(Permission denied (os error 13))".to_string(),
        };
        let line = report.line();
        assert!(!line.contains("Permission denied"), "{line}");
        assert!(line.contains("Permission_denied_(os_error_13)"), "{line}");
        let parsed = Report::parse(&line).expect("the parent must be able to read it");
        assert!(parsed.detail.contains("media-unavailable"));
        assert_eq!(parsed.protections, report.protections);
    }

    /// The helper note is a fact only when the platform did not say the grant
    /// failed. Each platform has its own word for that.
    #[test]
    fn the_helper_note_needs_the_grant_to_have_taken() {
        let detail = |facts: &[&str]| facts.iter().map(|f| f.to_string()).collect::<Vec<_>>();

        assert!(helper_is_granted(true, &detail(&["seccomp=97"])));
        assert!(
            !helper_is_granted(false, &detail(&[])),
            "a profile with no helper grant has nothing to report"
        );
        for refusal in [
            "helper_grants=0",
            "helper-rule-refused",
            "helper-grant-refused(Permission_denied)",
        ] {
            assert!(
                !helper_is_granted(true, &detail(&[refusal])),
                "{refusal} must suppress the note"
            );
        }
        // A refusal that is not about the helper leaves it alone.
        assert!(helper_is_granted(
            true,
            &detail(&["source-grant-refused(Access_is_denied)", "helper_grants=12"])
        ));
    }

    #[test]
    fn a_line_that_disagrees_with_itself_is_refused() {
        assert!(Report::parse("").is_none());
        assert!(Report::parse("level=full limits=on").is_none());
        assert!(
            Report::parse(
                "NFS2-sandbox profile=documents level=full limits=on files=open network=open \
                 process=open detail=x"
            )
            .is_none(),
            "the level token must match the items"
        );
        assert!(
            Report::parse(
                "NFS2-sandbox profile=documents level=none limits=maybe files=open network=open \
                 process=open detail=x"
            )
            .is_none()
        );
        assert!(
            Report::parse(
                "NFS2-sandbox profile=documents level=full limits=on files=denied network=denied \
                 process=denied detail=x"
            )
            .is_some()
        );
        // A profile is what every report names now, so a line without one — and
        // a line from the format before it — is not a report.
        assert!(
            Report::parse(
                "NFS2-sandbox level=full limits=on files=denied network=denied process=denied \
                 detail=x"
            )
            .is_none()
        );
        assert!(
            Report::parse(
                "NFX1-sandbox level=full limits=on files=denied network=denied process=denied \
                 detail=x"
            )
            .is_none(),
            "a line from before the profile existed is not a report"
        );
        assert!(Report::parse("NFS2-sandbox profile=elsewhere level=none limits=off files=open network=open process=open detail=x").is_none());
    }

    /// The residuals are notes on the items they weaken, never a grade: each
    /// known token produces its note and an unknown one produces none.
    #[test]
    fn the_residuals_are_notes_on_the_items() {
        let report = |detail: &str| Report {
            profile: Profile::Media,
            protections: Protections {
                limits: true,
                files: true,
                network: true,
                process: true,
            },
            detail: detail.to_string(),
        };
        assert!(report("seccomp=1").notes().is_empty());
        assert_eq!(report("fork=open").notes(), ["fork"]);
        assert_eq!(report("system=readable").notes(), ["system_tree"]);
        assert_eq!(report("writes=store").notes(), ["writes"]);
        // A write the container did not redirect is its own note: the benign word
        // must not stand for the measurement that cannot tell the two apart.
        assert_eq!(report("writes=user").notes(), ["writes_user"]);
        assert_eq!(report("writes=denied").notes(), Vec::<&str>::new());
        assert_eq!(report("metadata=open").notes(), ["metadata"]);
        assert_eq!(report("ll_gaps=open").notes(), ["ll_gaps"]);
        assert_eq!(report("helper=allowed").notes(), ["helper"]);
        // How much of the filesystem the helper may read, which is wider than the
        // process note beside it says; the wider macOS shape wins when both are
        // somehow present.
        assert_eq!(report("helper_scope=libs").notes(), ["helper_libs"]);
        assert_eq!(
            report("helper_scope=libs,helper_scope=trees").notes(),
            ["helper_trees"]
        );
        // The creation the parent settled for, and why the strongest one failed.
        assert_eq!(
            report("rung=container,container=appcontainer").notes(),
            Vec::<&str>::new()
        );
        assert_eq!(report("rung=token").notes(), ["container_plain"]);
        assert_eq!(report("rung=plain").notes(), ["container_plain"]);
        assert_eq!(
            report("container_refused=sid-refused").notes(),
            ["container_refused"]
        );
        assert_eq!(report("lpac=off").notes(), ["lpac_off"]);
        // The notes never touch the grade.
        assert_eq!(report("system=readable,fork=open").level(), Level::Full);
        // A token this build does not know is not a note.
        assert!(report("something=else").notes().is_empty());
    }
}
