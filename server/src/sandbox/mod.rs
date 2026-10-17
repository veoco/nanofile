//! Resource bounds and platform confinement for every child that parses
//! attacker-chosen bytes.
//!
//! A separate `worker` process bounds itself before it reads a byte. `limits`
//! (memory, CPU, and on unix descriptors and file size) is the hard invariant:
//! a child that cannot establish it is refused by both sides. `files`,
//! `network` and `process` are best-effort defense in depth; a host that cannot
//! supply one runs on `limits` alone and the report says so. The process item
//! does not apply to the media profile, which exists to start a program.
//!
//! A claim is measured by the child and checked by the parent, so a layer that
//! silently did nothing is reported as missing. `sandbox.enabled` is the master
//! switch, and there is no in-process fallback.
//!
//! This bounds a parser's effect on the host, not a kernel vulnerability or
//! another process running as the same user. The operator-facing description is
//! `docs/guide/sandbox.md`; the mechanisms and probe are in `docs/dev/sandbox.md`.

use std::path::Path;

pub mod jobs;
#[cfg(target_os = "linux")]
mod linux;
pub mod worker;
// The Seatbelt profile is built everywhere so its text can be tested; only macOS
// runs under it.
/// How macOS states the memory bound: the space mapped plus the cap.
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
/// container is asked for first and a refusal is remembered so it is not retried;
/// the token is applied either way. All three creations name the same job object.
#[cfg(target_os = "windows")]
pub(super) use windows::{Child as WindowsChild, spawn, spawn_token_only, spawn_unrestricted};

/// Whether the child runs in a less privileged container, where the platform
/// has one.
///
/// A fact about the launch, not an item: the opt-out is `WIN://NOALLAPPPKG` (`false`
/// on Windows 10), and the system tree stays readable either way through
/// `ALL RESTRICTED APPLICATION PACKAGES`. The note comes from the child's `system=`
/// measurement, not from this.
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

/// Stop asking for the less privileged container, for this process's life: the
/// attribute is accepted at creation, so a container whose loader cannot start
/// inside it fails *after* `CreateProcess` succeeds. The plain container keeps the
/// confinement; only the opt-out is given up.
pub fn disable_lpac() {
    #[cfg(target_os = "windows")]
    {
        windows::disable_lpac();
    }
}

/// Whether this profile gave the opt-out up and runs in the plain container.
///
/// Per profile, because only the media profile runs a second program. The container
/// is kept; what is lost is `ALL APPLICATION PACKAGES`, reported as `lpac=off`.
pub fn plain_container(profile: Profile) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::plain_container(profile)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = profile;
        false
    }
}

pub fn use_plain_container(profile: Profile) {
    #[cfg(target_os = "windows")]
    {
        windows::use_plain_container(profile);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = profile;
    }
}

/// Forget which profiles gave the opt-out up: a different helper is a different
/// measurement.
pub fn forget_plain_container() {
    #[cfg(target_os = "windows")]
    {
        windows::forget_plain_container();
    }
}

/// Forget the reasons the last launch recorded, before the next sequence begins. A
/// sequence is everything one probe does, not one creation.
pub(super) fn clear_container_shortfall() {
    #[cfg(target_os = "windows")]
    {
        windows::clear_shortfall();
    }
}

/// The parent's reason for not applying the container, out of a report's detail: a
/// child cannot see why it was not created in one, so the token is read from the
/// report.
pub(super) fn container_refusal(detail: &str) -> Option<&str> {
    detail
        .split(',')
        .find_map(|fact| fact.strip_prefix("container_refused="))
}

/// Why the child was not started in an AppContainer, where the platform has one.
///
/// `None` when it was applied, never asked for, or cannot exist here. The child can
/// only report that its token is not one, not why, so the probe prints this.
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

/// Most address space a child may use: a document is at most 256 MiB decompressed
/// and yields at most 8 MiB of text, and a gigabyte still bounds a bomb.
#[cfg(unix)]
const ADDRESS_SPACE_LIMIT: u64 = 1 << 30;

/// CPU seconds before the soft (`SIGXCPU`) and hard (`SIGKILL`) limits. The parent's
/// wall-clock timeout bounds a hang; this holds even when the parent is not
/// scheduled.
#[cfg(unix)]
const CPU_SOFT_SECONDS: u64 = 15;
#[cfg(unix)]
const CPU_HARD_SECONDS: u64 = 18;

/// Open descriptors: the three standard streams and slack for the runtime.
#[cfg(unix)]
const NOFILE_LIMIT: u64 = 32;

/// Most bytes the child may write to a *file*: not zero, because a redirected
/// standard output is a file too, and paths are denied by the filesystem layer
/// regardless.
#[cfg(unix)]
const FILE_SIZE_LIMIT: u64 = 16 * 1024 * 1024;

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
    /// Never set for [`Profile::Media`], which starts the helper by copying the
    /// process: the page says the item is missing rather than claiming that bound.
    pub process: bool,
}

impl Protections {
    /// The one item no child may run without, and the hard invariant
    /// [`Requirement::accepts`] enforces: a host that establishes no memory and CPU
    /// bound runs nothing. Every other item is what the platform *could* add.
    pub const REQUIRED: &'static str = "limits";

    /// Whether one item is the required one, by the token [`Protections::items`]
    /// carries.
    pub fn required(item: &str) -> bool {
        item == Self::REQUIRED
    }

    /// One item by its token, or `None` for a name this build does not know.
    pub fn get(self, item: &str) -> Option<bool> {
        self.items()
            .into_iter()
            .find(|(name, _)| *name == item)
            .map(|(_, present)| present)
    }

    /// Whether every item is in place. Not a policy: it says what was established,
    /// while [`Requirement::accepts`] is what refuses a host that established
    /// nothing.
    pub fn complete(self) -> bool {
        self.limits && self.files && self.network && self.process
    }

    /// The four items as one line, using the report line's own tokens (`on|off`,
    /// `denied|open`), so a log entry and a raw report read the same way.
    pub fn summary(self) -> String {
        self.items()
            .into_iter()
            .map(|(item, present)| {
                let token = match (present, item == Self::REQUIRED) {
                    (true, true) => "on",
                    (true, false) => "denied",
                    (false, true) => "off",
                    (false, false) => "open",
                };
                format!("{item}={token}")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Every item, in page order; the name is the token the report line carries, so
    /// the page and the line cannot drift apart.
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
/// Told on the command line *before* the request is read, because a Landlock or
/// Seatbelt grant must be installed before the bytes it applies to.
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
    /// Only the media profile: every other child is denied `exec`, and the media
    /// child's `exec` is narrowed to the one binary the parent names.
    pub fn runs_helper(self) -> bool {
        matches!(self, Profile::Media)
    }

    pub const ALL: [Profile; 3] = [Profile::Documents, Profile::Images, Profile::Media];
}

/// What the profile's own work produced, when the self-test measured it: the items
/// cannot say whether the program still works, and the child writes that as
/// `parse=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobVerdict {
    Ok,
    /// No helper is configured, so there was nothing to run. An administrator's
    /// choice rather than a fault, named in the settings.
    NoHelper,
    /// There was nothing to work with — no file to read. A state, not a failure.
    Nothing,
    Empty,
    Different,
    Unsupported,
    /// The program that does the work could not be started, or failed.
    Failed,
}

/// What the child established, and how.
#[derive(Debug, Clone)]
pub struct Report {
    /// Which pipeline this child confined itself for.
    pub profile: Profile,
    pub protections: Protections,
    /// What the profile's own work produced in the self-test. `None` for a
    /// request's child, which reports nothing.
    ///
    /// Read from the `parse=` token, which is what crosses the process boundary.
    /// Unlike a note this one *is* decided on, for the media profile alone.
    pub job: Option<JobVerdict>,
    /// Mechanism tokens and measurements, comma-joined and whitespace-free.
    pub detail: String,
}

impl Report {
    /// The residuals the detail carries, as note keys for the settings page.
    ///
    /// Every token is a measured fact and a token no backend emits produces no note.
    /// They are *notes*: every decision is made from [`Protections`] alone, so
    /// wording can never weaken one.
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
        // The container without the restricted token still holds the privileges the
        // flags would have removed; reported, not decided on.
        if has("token=unrestricted") {
            notes.push("token_unrestricted");
        }
        // How much the media helper may read, which is wider than "the helper and
        // its loader": the libraries beside it in every case, and on macOS the
        // package-manager trees a packaged build links against.
        if has("helper_scope=trees") {
            notes.push("helper_trees");
        } else if has("helper_scope=libs") {
            notes.push("helper_libs");
        }
        // The creation the parent settled for. `container-only` comes from the media
        // profile's opt-out retry; `unrestricted` is the step below `plain` and
        // carries the same note.
        if has("rung=plain") || has("rung=unrestricted") {
            notes.push("container_plain");
        }
        if has("rung=container-only") {
            notes.push("container_no_token");
        }
        if container_refusal(&self.detail).is_some() {
            notes.push("container_refused");
        }
        if has("lpac=off") {
            notes.push("lpac_off");
        }
        // The helper starts by copying the process, and nothing bounds how many
        // copies there may be: the item is missing for this profile, and the note
        // says what bounds them instead.
        if has("media_process=unbounded") {
            notes.push("media_process");
        }
        notes
    }

    /// The one-line report the self-test prints and the parent parses.
    ///
    /// Fields are whitespace-separated, so the detail is made whitespace-free: an OS
    /// error string would split it, and the parent treats that as no report.
    pub fn line(&self) -> String {
        let yes = |on: bool| if on { "denied" } else { "open" };
        let detail = self.detail.replace([' ', '\t', '\n'], "_");
        format!(
            "NFS2-sandbox profile={} limits={} files={} network={} process={} detail={}",
            self.profile.as_str(),
            if self.protections.limits { "on" } else { "off" },
            yes(self.protections.files),
            yes(self.protections.network),
            yes(self.protections.process),
            detail
        )
    }

    /// Parse a line produced by [`Report::line`].
    ///
    /// `None` for anything that is not one, and every item field must be present: a
    /// line cut before `limits` reads as unbounded and is refused, one cut after it
    /// under-claims, never over-claims. An unknown name is not an error.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        let mut fields = line.strip_prefix("NFS2-sandbox ")?.split(' ');
        let mut profile = None;
        let mut protections = Protections::default();
        let mut items = 0u8;
        let mut detail = String::new();

        for field in fields.by_ref() {
            let (key, value) = field.split_once('=')?;
            match key {
                "profile" => profile = Profile::parse(value),
                "limits" => {
                    protections.limits = parse_switch(value)?;
                    items += 1;
                }
                "files" => {
                    protections.files = parse_denial(value)?;
                    items += 1;
                }
                "network" => {
                    protections.network = parse_denial(value)?;
                    items += 1;
                }
                "process" => {
                    protections.process = parse_denial(value)?;
                    items += 1;
                }
                "detail" => {
                    detail = value.to_string();
                }
                // Extra tokens carry diagnostics the parent may want to log
                // (`text_chars=…`), so an unknown name is not an error; the
                // shape of the line still is.
                _ => {}
            }
        }

        // All four, exactly once: a field the line lost, or one it repeated, is
        // not a report this build knows how to read.
        if items != 4 {
            return None;
        }

        let job = job_verdict(&detail);
        Some(Report {
            profile: profile?,
            protections,
            job,
            detail,
        })
    }
}

/// The profile's own work, as the self-test's `parse=` token says.
///
/// The child writes the token last and the parent appends its facts after it, so
/// the item list ends at the first field naming a key again. The answer is the first
/// item that is not `ok`. An unknown verdict is skipped.
fn job_verdict(detail: &str) -> Option<JobVerdict> {
    let mut overall: Option<JobVerdict> = None;
    let mut inside = false;
    for field in detail.split(',') {
        let item = match field.strip_prefix("parse=") {
            Some(item) => {
                inside = true;
                item
            }
            None if !inside => continue,
            None if field.contains('=') => break,
            None => field,
        };
        match item_verdict(item) {
            Some(JobVerdict::Ok) => overall = overall.or(Some(JobVerdict::Ok)),
            Some(other) => return Some(other),
            None => {}
        }
    }
    overall
}

/// One item of the token: `pdf-ok`, `media-ok(2333)`, `media-failed(exit_1)`.
fn item_verdict(item: &str) -> Option<JobVerdict> {
    let (_, verdict) = item.split_once('-')?;
    let verdict = verdict.split('(').next().unwrap_or(verdict);
    Some(match verdict {
        "ok" => JobVerdict::Ok,
        "no-text" => JobVerdict::Empty,
        "no-helper" => JobVerdict::NoHelper,
        "no-source" => JobVerdict::Nothing,
        "text-differs" => JobVerdict::Different,
        "unsupported" => JobVerdict::Unsupported,
        "failed" | "unavailable" => JobVerdict::Failed,
        _ => return None,
    })
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
/// The master switch, and nothing else: `limits` is read back from the child's own
/// report, and every layer above it is best-effort, reported and never demanded. The
/// switch crosses the process boundary so the child can refuse on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// `sandbox.enabled`: the master switch over every parsing feature.
    pub enabled: bool,
}

impl Default for Requirement {
    /// The shipped default: the sandbox is on. A host that cannot bound its
    /// children refuses them however this reads; a host that can runs them at
    /// whatever confinement it offers.
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Requirement {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Whether a report is enough, and if not, why not.
    ///
    /// Asked by the parent at startup and again by the child before it reads a
    /// request. The switch is the admin's choice; `limits` is the hard invariant, and
    /// a child without it is refused whatever the configuration says.
    pub fn accepts(&self, report: &Report) -> Result<(), Refusal> {
        if !self.enabled {
            return Err(Refusal::Disabled);
        }
        if !report
            .protections
            .get(Protections::REQUIRED)
            .unwrap_or(false)
        {
            return Err(Refusal::Unbounded);
        }
        Ok(())
    }
}

/// Why a confinement was refused: the switch is the admin's choice and leaves the
/// work undone, while an unbounded child is an environment problem with no safe way
/// to run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `sandbox.enabled` is false.
    Disabled,
    /// The child established no resource bound at all.
    Unbounded,
}

impl Refusal {
    /// The refusal as a sentence, for a log line or a error.
    pub fn reason(self) -> String {
        match self {
            Refusal::Disabled => "the sandbox is switched off (`sandbox.enabled`)".to_string(),
            Refusal::Unbounded => {
                "this host could not bound the worker's memory and CPU".to_string()
            }
        }
    }
}

/// Whether a profile's features may run at all on this host: the switch is on, the
/// child probed ready, and it established its resource bound. A caller with no
/// unconfined fallback asks this first.
pub fn available(profile: Profile) -> Result<(), String> {
    match worker::status(profile) {
        worker::Status::Ready(_) => Ok(()),
        worker::Status::Unavailable(why) => Err(why),
    }
}

/// How much of the report is measured by effect.
///
/// The report says which tier produced it. The decision is made from the thorough
/// tier; the per-document child uses the cheap one, leaving expensive probes to the
/// probe that runs once.
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

/// The runner this platform needs around `exe`, if any: only macOS, where a Seatbelt
/// profile can only be applied by `/usr/bin/sandbox-exec`. The path is hardcoded so
/// a `PATH` entry cannot substitute another program.
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
/// Only the media profile has any: ffmpeg's own image, its libraries, and the
/// scratch source the parent wrote. Everything else gets the empty deny-all set.
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
/// Must run on the child's only thread before the request is read: Landlock and
/// seccomp are inherited by threads created afterwards, and the parsers create one.
/// `profile` says what the child may reach and is carried into the report; `grants`
/// is what that profile may reach.
///
/// `probe` is a file the parent created and no profile grants, the one candidate
/// decisive on every platform; `None` leaves it to the platform's heuristics.
/// `measure_facts` runs the probes and `protect` decides the items, so every branch
/// is testable without real confinement.
pub fn confine(
    external: External,
    profile: Profile,
    grants: Grants,
    measure: Measure,
    probe: Option<&Path>,
) -> Report {
    let (installed, detail) = platform_confine(profile, grants);
    // The parent's claim is applied *before* the probes, so a layer a runner claims is
    // measured like any other. Applied after, the macOS report would say
    // `files=installed` instead of the refusal the probe found.
    let claimed = claimed_by_mechanism(installed, external);
    let facts = measure_facts(claimed, profile, grants, measure, probe);
    let (protections, detail) = protect(claimed, detail, external, profile, measure, &facts);

    Report {
        profile,
        protections,
        job: None,
        detail: detail.join(","),
    }
}

/// What a mechanism claimed, before any of it is measured.
///
/// The runner is the one claim the *parent* makes: a profile governing files, the
/// network and process creation, none of it visible from here. Each is still put to
/// its probe, so a wrapper that did nothing drops the item.
fn claimed_by_mechanism(installed: Protections, external: External) -> Protections {
    if external.runner {
        Protections {
            files: true,
            network: true,
            process: true,
            ..installed
        }
    } else {
        installed
    }
}

/// What the effect probes answered: `None` when *this tier* did not measure that
/// item, which is not the same as measuring it missing.
#[derive(Default)]
struct Facts {
    /// Whether a path no profile grants was refused. `Some(true)` is the layer
    /// standing, `Some(false)` the layer not being there.
    files: Option<bool>,
    /// Whether a socket could not be made, or a connection not opened.
    network: Option<bool>,
    /// Which side of the files item the system tree is on: the residual the two
    /// platforms that grant it report beside the item.
    system: Option<&'static str>,
    /// What the fork/exec probe answered, on the tier that can afford one.
    process: Option<ProcessFacts>,
    /// Why opening the granted helper failed, where the grant is creation
    /// rather than a rule this side can read back.
    helper_refused: Option<String>,
    /// Whether the parent named a helper for this profile at all.
    helper_granted: bool,
    /// Whether this process can still spawn a thread.
    threads: bool,
}

/// Run the effect probes this tier can afford.
///
/// A claim no mechanism made is never put to a probe: opening the parent's file or
/// binding a socket is a cost, and a layer never installed cannot be measured here.
fn measure_facts(
    claimed: Protections,
    profile: Profile,
    grants: Grants<'_>,
    measure: Measure,
    probe: Option<&Path>,
) -> Facts {
    let mut facts = Facts {
        helper_granted: grants.helper.is_some(),
        threads: threads_work(),
        ..Facts::default()
    };
    // The profile is read by the helper probe alone, and that probe only exists
    // on the two platforms whose grant is creation rather than a rule this side
    // can read back.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = profile;

    // Every protection that was claimed by a mechanism is put to its probe,
    // whichever process installed it: the child's own Landlock ruleset, a token
    // the parent created, or a profile a runner applied.
    #[cfg(any(unix, windows))]
    {
        if claimed.files && files_are_measured(measure) {
            facts.files = Some(files_are_denied(probe));
        }
        if claimed.network && network_is_measured(measure) {
            facts.network = Some(network_is_denied());
        }
    }
    // The far edge of the files item, reported beside it: on Windows the system tree
    // stays readable through the package ACLs; on macOS the dynamically linked worker
    // grants the system library trees. Which one it found is part of the answer.
    #[cfg(windows)]
    if claimed.files {
        facts.system = Some(system_tree());
    }
    #[cfg(target_os = "macos")]
    if claimed.files {
        facts.system = Some(macos::system_tree());
    }
    // Process creation is claimed by a mechanism whose effect is not cheap to see:
    // macOS's runner and Linux's seccomp filter. Checked here, in the tier that can
    // afford a fork.
    #[cfg(unix)]
    if measure == Measure::Thorough {
        facts.process = Some(process_facts());
    }
    // The helper is a grant the parent made before this process existed, applied as
    // creation on some platforms, so the only check is to try opening the file. The
    // effect is `parse=`; this is the grant that leads to it.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if profile.runs_helper()
        && let Some(helper) = grants.helper
        && let Err(error) = std::fs::File::open(helper)
    {
        facts.helper_refused = Some(error.to_string());
    }

    facts
}

/// Decide the items from the claims and the measurements.
///
/// The fail-closed half: a claimed layer is cleared when the probe says the effect is
/// not there, and a claim no probe reached is reported as installed. Nothing here
/// touches the process, so tests can take the paths a confining host never takes.
fn protect(
    mut protections: Protections,
    mut detail: Vec<String>,
    external: External,
    profile: Profile,
    measure: Measure,
    facts: &Facts,
) -> (Protections, Vec<String>) {
    if external.runner {
        // The runner's claim was applied before the probes; this is what they
        // backed up.
        detail.push("runner=external".to_string());
    }

    if protections.files {
        match facts.files {
            Some(true) => detail.push("files=measured-denied".to_string()),
            Some(false) => {
                protections.files = false;
                detail.push("files=measured-open".to_string());
            }
            // What the mechanism installed. The confirming probe needs the parent's
            // file, so it belongs to the probe that runs once.
            None => detail.push("files=installed".to_string()),
        }
    }
    if protections.network {
        match facts.network {
            Some(true) => detail.push("network=measured-denied".to_string()),
            Some(false) => {
                protections.network = false;
                detail.push("network=measured-open".to_string());
            }
            // What the kernel's token says. The confirming connection attempt belongs
            // to the probe that runs once.
            None => detail.push("network=installed".to_string()),
        }
    }
    // Beside the item, never in place of it: a cleared files layer has no system
    // tree to describe.
    if protections.files
        && let Some(system) = facts.system
    {
        detail.push(format!("system={system}"));
    }

    if let Some(process) = facts.process {
        detail.push(format!("fork={}", denial_token(process.fork_denied)));
        detail.push(format!("exec={}", denial_token(process.exec_denied)));
        if external.runner {
            // A profile that may copy itself has not bounded process creation, whatever
            // it denies about starting programs: `fork=open` says they can multiply.
            // Media keeps the fork for the helper, so the item stays missing and the
            // `fork` note is the reason.
            if process.fork_denied == Some(false) {
                protections.process = false;
                detail.push("process=measured-open".to_string());
            } else {
                match process.exec_denied {
                    Some(true) => detail.push("process=measured-denied".to_string()),
                    Some(false) => {
                        protections.process = false;
                        detail.push("process=measured-open".to_string());
                    }
                    None => detail.push("process=unmeasured".to_string()),
                }
            }
        }
    }

    if let Some(error) = &facts.helper_refused {
        detail.push(format!("helper-grant-refused({error})"));
    }

    // The media profile's process item and the helper it starts. The item is cleared
    // whatever the platform installed, because every way of starting a program copies
    // the process: the page says it does not apply, and `fork` is the note beside it.
    // The helper is a *fact* rather than a protection; its effect is `parse=`.
    if profile.runs_helper() {
        protections.process = false;
        if helper_is_granted(facts.helper_granted, &detail) {
            detail.push("helper=allowed".to_string());
        }
    }

    // The parsers spawn a thread of their own; a sandbox that breaks that makes
    // every document unreadable, so it is reported rather than assumed.
    detail.push(
        if facts.threads {
            "threads=ok"
        } else {
            "threads=failed"
        }
        .to_string(),
    );
    detail.push(format!("measure={}", measure.as_str()));

    (protections, detail)
}

/// Whether the platform said the helper's grant took. `helper-rule-refused`,
/// `helper_grants=0` and `helper-grant-refused…` say it did not; without a complaint
/// the grant is what the profile was built with.
fn helper_is_granted(helper: bool, detail: &[String]) -> bool {
    helper
        && !detail.iter().any(|fact| {
            fact == "helper_grants=0"
                || fact == "helper-rule-refused"
                || fact.starts_with("helper-grant-refused")
        })
}

/// How a three-valued denial is spelled in the report.
fn denial_token(denied: Option<bool>) -> &'static str {
    match denied {
        Some(true) => "denied",
        Some(false) => "open",
        None => "unmeasured",
    }
}

/// Whether this tier runs the network layer's effect probe.
///
/// Unix always does: an ephemeral bind needs no peer. On Windows a silent host can
/// stretch the connection attempt to its whole timeout, once per document, so the
/// per-document child claims the layer from the container token instead.
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

/// Whether this tier runs the files layer's effect probe.
///
/// Unix always does. On Windows the decisive probe needs the parent's file, so a
/// per-request child claims the layer from the container token instead.
#[cfg(any(unix, windows))]
fn files_are_measured(measure: Measure) -> bool {
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
/// Both directions: a write to the host is what the layer is for, and a denial in the
/// temporary directory, the one place everyone may write, is the sandbox and nothing
/// else.
///
/// The parent's `probe` file comes first and is decisive: no profile grants it, so a
/// refusal is the sandbox and an open is the sandbox failing. The write candidate
/// alone is not enough, because a Windows container redirects a write and still
/// confines the read. Any read path failing to open is the denial, and the candidates
/// are only paths no profile grants — not `/` or `/usr/lib`, which macOS grants.
#[cfg(unix)]
fn files_are_denied(probe: Option<&Path>) -> bool {
    if let Some(probe) = probe {
        match std::fs::File::open(probe) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return true,
            // The parent's file is gone: no measurement, fall through.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            // Opened, or failed in a way that says nothing about the sandbox:
            // a read that is not refused is not a files layer.
            _ => return false,
        }
    }

    let reads = ["/etc/hostname", "/etc/passwd"].iter().any(|path| {
        matches!(
            std::fs::File::open(path),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
        )
    });

    // A unique name, the way the parent's own probe file is made: a path a
    // local user could predict is a path they could leave a symlink at.
    let probe = std::env::temp_dir().join(format!(
        "nanofile-extraction-write-probe-{}",
        uuid::Uuid::new_v4()
    ));
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
/// The container is not deny-everything: `ALL APPLICATION PACKAGES` leaves the system
/// tree readable. The parent's `probe` file is decisive — it is in the user's
/// temporary directory, which carries no package ACE, and a read cannot be
/// redirected.
#[cfg(windows)]
fn files_are_denied(probe: Option<&Path>) -> bool {
    let denied = |error: &std::io::Error| error.kind() == std::io::ErrorKind::PermissionDenied;

    if let Some(probe) = probe {
        match std::fs::File::open(probe) {
            Err(error) if denied(&error) => return true,
            // The parent's file is gone: no measurement, fall through.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            // Opened, or failed in a way that says nothing about the sandbox.
            _ => return false,
        }
    }

    if let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        && let Err(error) = std::fs::read_dir(directory)
        && denied(&error)
    {
        return true;
    }

    // Unique for the same reason the unix candidate is: a predictable path is a
    // path a local user could leave a symlink at.
    let probe = std::env::temp_dir().join(format!(
        "nanofile-extraction-write-probe-{}",
        uuid::Uuid::new_v4()
    ));
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
/// An ephemeral UDP bind needs no peer, so a refused `socket` or `bind` is the
/// sandbox and anything else means sockets still work.
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
/// A zero-capability container cannot connect: the Windows Filtering Platform refuses
/// it with `WSAEACCES` rather than the timeout a silent drop gives. A firewall rule
/// can produce the same error, so only a process whose token says it is in a
/// container claims this layer.
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
/// A residual: the packages a low-box process is checked against are granted across
/// the system tree, so a loose parser reads `Windows` while the user's files are
/// refused. The principal is left to the platform (`ALL RESTRICTED APPLICATION
/// PACKAGES` under the opt-out), and a host answering `denied` is stricter, not
/// broken. Candidates read from `SystemRoot`, not a hard-coded `C:`.
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

/// What trying to fork and to start a program answered: the fork's copy is where
/// `execve` is tried, and which failed tells a profile that denies programs from one
/// that denies nothing. `None` means the question could not be asked.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
struct ProcessFacts {
    fork_denied: Option<bool>,
    exec_denied: Option<bool>,
}

/// Whether forking and starting a program are refused, measured.
///
/// A syscall pair rather than a started program, so measuring the sandbox is not
/// itself unbounded: nothing of ours runs in the copy, and no allocation or lock is
/// touched in a child of a threaded process. `exit 0` means the helper *ran*.
#[cfg(unix)]
fn process_facts() -> ProcessFacts {
    use std::ffi::CString;

    /// The copy's code for "`execve` was refused for permission".
    const DENIED: libc::c_int = 2;
    /// The copy's code for "`execve` failed for some other reason": the syscall
    /// itself was allowed, so this says nothing about the sandbox.
    const OTHER: libc::c_int = 3;
    /// The errors a confinement layer refuses an `exec` with: `EPERM` from seccomp
    /// or Seatbelt, `EACCES` from Landlock, which is the files layer. Both are the
    /// sandbox saying no.
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
    // Inherited rather than null: a null stream opens `/dev/null` for writing, which
    // the macOS profile denies, and the probe would measure that open instead.
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

/// The limits a process can set for itself, and whether each took.
///
/// Kept apart because macOS has no address-space limit; the caller adds its own.
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
    /// Whether these limits plus a caller-supplied memory bound make a limits
    /// layer. Every limit is required; memory may come from the address-space limit
    /// or from `memory`.
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
/// Linux takes the cap itself; Darwin refuses a limit below what is already mapped,
/// so there it is the mapped size plus the cap, spelled `+<cap>`, and the growth is
/// what is bounded.
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
/// Both the soft and hard values are set: a soft-only clamp can be raised by the
/// process it is meant to bound. The detail lists every limit that took and every
/// one refused. Darwin has no address-space cap; `watchdog` stands in for it.
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

    /// Unconfined, the probe must *say* unconfined: one that could only answer
    /// "denied" would certify a sandbox that is not there.
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

    /// The other two probes, in the same direction: a probe that could only answer
    /// one way would certify a sandbox that is not there.
    #[cfg(unix)]
    #[test]
    fn the_files_and_network_probes_report_an_unconfined_process() {
        assert!(
            !files_are_denied(None),
            "this process can read the host's files and write its temporary directory"
        );

        let readable =
            std::env::temp_dir().join(format!("nanofile-open-file-{}", std::process::id()));
        std::fs::write(&readable, b"x").expect("write a file this process may read");
        assert!(
            !files_are_denied(Some(&readable)),
            "a file this process can read is not a refusal"
        );
        let _ = std::fs::remove_file(&readable);

        // A probe file the parent is done with says nothing and is not itself a
        // refusal: the candidates below it are what answer.
        assert!(
            !files_are_denied(Some(Path::new("/nonexistent-sandbox-probe"))),
            "a probe file that is gone is not a denial"
        );

        assert!(
            !network_is_denied(),
            "this process may bind a socket, so the network layer is not there"
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

    /// The page lists exactly the items the report carries, in the order it
    /// carries them, under the names the report line uses.
    #[test]
    fn the_page_lists_the_items_in_the_report_order() {
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
    fn the_requirement_refuses_the_switch_and_the_unbounded_and_nothing_else() {
        /// A report whose items are `protections`.
        fn report_with(protections: Protections) -> Report {
            Report {
                profile: Profile::Documents,
                protections,
                job: None,
                detail: String::new(),
            }
        }

        /// The shapes a host can report, from the floor up: the resource bound
        /// alone, plus the files layer, plus everything but the process item (the
        /// media profile's shape), and every item in place.
        fn host_shapes() -> [Protections; 4] {
            [
                Protections {
                    limits: true,
                    ..Protections::default()
                },
                Protections {
                    limits: true,
                    files: true,
                    ..Protections::default()
                },
                Protections {
                    limits: true,
                    files: true,
                    network: true,
                    process: false,
                },
                Protections {
                    limits: true,
                    files: true,
                    network: true,
                    process: true,
                },
            ]
        }

        // The switch refuses whatever the host gives.
        for items in host_shapes() {
            assert_eq!(
                Requirement::new(false).accepts(&report_with(items)),
                Err(Refusal::Disabled),
                "the switch is off at {items:?}"
            );
        }

        // Every shape above the floor is accepted: the layers are what the
        // platform could add, not a condition of running the feature.
        for items in host_shapes() {
            assert!(
                Requirement::new(true).accepts(&report_with(items)).is_ok(),
                "a bounded host at {items:?} must be accepted"
            );
        }

        // The resource bound is the one refusal left: a report with no bound is
        // refused however many other items it carries.
        let unbounded = report_with(Protections {
            files: true,
            network: true,
            process: true,
            ..Protections::default()
        });
        assert_eq!(
            Requirement::new(true).accepts(&unbounded),
            Err(Refusal::Unbounded),
            "no resource bound"
        );

        // A residual in the detail never changes the decision: it is a note on
        // an item, not an item of its own.
        let mut full = report_with(Protections {
            limits: true,
            files: true,
            network: true,
            process: true,
        });
        assert!(Requirement::default().accepts(&full).is_ok());
        full.detail = "system=readable,fork=open".to_string();
        assert!(Requirement::default().accepts(&full).is_ok());

        assert!(Requirement::default().enabled);
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
            job: None,
        };
        let line = report.line();
        assert!(
            line.starts_with(
                "NFS2-sandbox profile=documents limits=on files=denied \
                 network=denied process=denied detail="
            ),
            "{line}"
        );
        let parsed = Report::parse(&line).expect("parses");
        assert_eq!(parsed.profile, Profile::Documents);
        assert_eq!(parsed.protections, report.protections);
        assert_eq!(parsed.detail, report.detail);

        // The media profile's shape: the process item is the one it cannot have,
        // so its line says `open`, and it still round-trips.
        let media = Report {
            profile: Profile::Media,
            protections: Protections {
                limits: true,
                files: true,
                network: true,
                process: false,
            },
            detail: "fork=open,helper=allowed".to_string(),
            job: None,
        };
        let line = media.line();
        assert!(
            line.starts_with(
                "NFS2-sandbox profile=media limits=on files=denied \
                 network=denied process=open detail="
            ),
            "{line}"
        );
        let parsed = Report::parse(&line).expect("parses");
        assert_eq!(parsed.protections, media.protections);
        assert_eq!(parsed.notes(), ["fork", "helper"]);
    }

    /// The profile's own verdict is read from the token the child wrote, and a
    /// token this build does not know refuses nothing.
    #[test]
    fn the_job_verdict_is_read_from_the_parse_token() {
        let job = |detail: &str| {
            Report::parse(&format!(
                "NFS2-sandbox profile=media limits=on files=denied \
                 network=denied process=open detail={detail}"
            ))
            .expect("a media line parses")
            .job
        };

        assert_eq!(job("parse=media-ok(2333)"), Some(JobVerdict::Ok));
        assert_eq!(job("parse=media-no-helper"), Some(JobVerdict::NoHelper));
        assert_eq!(job("parse=media-no-source"), Some(JobVerdict::Nothing));
        assert_eq!(
            job("parse=media-failed(exit_status:_1)"),
            Some(JobVerdict::Failed)
        );
        assert_eq!(
            job("parse=media-unavailable(Access_is_denied)"),
            Some(JobVerdict::Failed)
        );
        // macOS appends the hand-attempted exec check after the verdict.
        assert_eq!(job("parse=media-ok(2333),hand=ok"), Some(JobVerdict::Ok));
        // Every item has to be `ok` for a profile with several to be `ok`.
        assert_eq!(
            job("parse=pdf-ok,docx-ok,xlsx-ok,pptx-ok"),
            Some(JobVerdict::Ok)
        );
        assert_eq!(
            job("parse=pdf-ok,docx-text-differs"),
            Some(JobVerdict::Different)
        );
        assert_eq!(job("parse=pdf-unsupported"), Some(JobVerdict::Unsupported));
        assert_eq!(job("parse=pdf-no-text"), Some(JobVerdict::Empty));
        // A parent's own facts follow the token and are not items.
        assert_eq!(
            job("parse=media-ok(1),rung=container,lpac=on"),
            Some(JobVerdict::Ok)
        );
        // News from a build that spoke another token: no verdict, so nothing is
        // refused on it.
        assert_eq!(job("fork=open,helper=allowed"), None);
        assert_eq!(job("parse=media-invented(1)"), None);
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
            job: Some(JobVerdict::Failed),
        };
        let line = report.line();
        assert!(!line.contains("Permission denied"), "{line}");
        assert!(line.contains("Permission_denied_(os_error_13)"), "{line}");
        let parsed = Report::parse(&line).expect("the parent must be able to read it");
        assert!(parsed.detail.contains("media-unavailable"));
        assert_eq!(parsed.job, Some(JobVerdict::Failed));
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

    /// A line is a report only when every item is there: a field lost to truncation
    /// is not a protection that is absent, so the line is refused.
    #[test]
    fn a_line_that_is_not_a_whole_report_is_refused() {
        assert!(Report::parse("").is_none());
        assert!(Report::parse("level=full limits=on").is_none());

        // No profile.
        assert!(
            Report::parse(
                "NFS2-sandbox limits=on files=denied network=denied process=denied detail=x"
            )
            .is_none()
        );
        // A line from the format before the profile existed.
        assert!(
            Report::parse(
                "NFX1-sandbox limits=on files=denied network=denied process=denied detail=x"
            )
            .is_none(),
            "a line from before the profile existed is not a report"
        );

        // One item missing, whichever it is: the line cannot say what the host
        // gave, and defaulting the field would silently report a layer as absent.
        let fields = [
            ("profile", "documents"),
            ("limits", "on"),
            ("files", "denied"),
            ("network", "denied"),
            ("process", "denied"),
            ("detail", "x"),
        ];
        for missing in ["limits", "files", "network", "process"] {
            let line = fields
                .iter()
                .filter(|(key, _)| *key != missing)
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                Report::parse(&format!("NFS2-sandbox {line}")).is_none(),
                "a line without {missing} is not a report"
            );
        }

        // A value this build does not know is refused rather than guessed at, and
        // so is a profile name it does not know.
        assert!(
            Report::parse(
                "NFS2-sandbox profile=documents limits=maybe files=open network=open process=open \
                 detail=x"
            )
            .is_none()
        );
        assert!(
            Report::parse(
                "NFS2-sandbox profile=elsewhere limits=off files=open network=open process=open \
                 detail=x"
            )
            .is_none()
        );

        // A whole line is one, and the redundant `level=` token a build before
        // this one printed is ignored: the items are the decision.
        assert!(
            Report::parse(
                "NFS2-sandbox profile=documents level=full limits=on files=denied network=denied \
                 process=denied detail=x"
            )
            .is_some()
        );
    }

    /// The residuals are notes on the items they weaken, never items themselves:
    /// each known token produces its note and an unknown one produces none.
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
            job: None,
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
        // The media profile keeps the fork and no platform bounds how many
        // copies it may make, which is the note beside the item it cannot have.
        assert_eq!(report("media_process=unbounded").notes(), ["media_process"]);
        assert_eq!(report("helper=allowed").notes(), ["helper"]);
        // A container child whose token is not restricted: the container bounds
        // files and the network, but the flags that strip privileges did not
        // apply, and the page says so rather than letting the item imply it.
        assert_eq!(
            report("token=restricted,helper=allowed").notes(),
            ["helper"]
        );
        assert_eq!(report("token=unrestricted").notes(), ["token_unrestricted"]);
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
        assert_eq!(
            report("rung=container-only").notes(),
            ["container_no_token"]
        );
        assert_eq!(report("rung=plain").notes(), ["container_plain"]);
        assert_eq!(report("rung=unrestricted").notes(), ["container_plain"]);
        assert_eq!(
            report("container_refused=sid-refused").notes(),
            ["container_refused"]
        );
        assert_eq!(report("lpac=off").notes(), ["lpac_off"]);
        // The notes never touch the items.
        assert!(report("system=readable,fork=open").protections.complete());
        // A token this build does not know is not a note.
        assert!(report("something=else").notes().is_empty());
    }

    /// The protections a mechanism claimed, and the detail that came with them:
    /// what `protect` is handed before it decides anything.
    fn claimed(files: bool, network: bool, process: bool) -> (Protections, Vec<String>) {
        (
            Protections {
                limits: true,
                files,
                network,
                process,
            },
            vec!["backend=test".to_string()],
        )
    }

    /// What the fold appended to a detail, which is everything after the
    /// mechanism's own tokens.
    fn appended(detail: Vec<String>) -> Vec<String> {
        detail.into_iter().skip(1).collect()
    }

    /// The runner's claim is the parent's, the one a liar could make: every layer it
    /// claims is put to a probe, and "open" drops the item.
    #[test]
    fn a_claim_the_probe_does_not_back_up_is_cleared() {
        let (protections, detail) = claimed(true, true, true);
        let facts = Facts {
            files: Some(false),
            network: Some(false),
            process: Some(ProcessFacts {
                fork_denied: Some(false),
                exec_denied: Some(false),
            }),
            threads: true,
            ..Facts::default()
        };
        let (protections, detail) = protect(
            protections,
            detail,
            External { runner: true },
            Profile::Documents,
            Measure::Thorough,
            &facts,
        );

        assert!(
            !protections.files && !protections.network && !protections.process,
            "a wrapper that did nothing may not keep a layer: {protections:?}"
        );
        assert!(
            !protections.complete(),
            "resource limits alone are the floor, not every item"
        );
        assert_eq!(
            appended(detail),
            [
                "runner=external",
                "files=measured-open",
                "network=measured-open",
                "fork=open",
                "exec=open",
                "process=measured-open",
                "threads=ok",
                "measure=thorough",
            ]
        );
    }

    /// The runner's claim reaches the probes, so a wrapper that did nothing drops the
    /// level.
    #[test]
    fn the_runners_claim_is_applied_before_the_probes() {
        let installed = Protections {
            limits: true,
            ..Protections::default()
        };
        assert_eq!(
            claimed_by_mechanism(installed, External { runner: true }),
            Protections {
                limits: true,
                files: true,
                network: true,
                process: true,
            }
        );
        assert_eq!(
            claimed_by_mechanism(installed, External::default()),
            installed,
            "with no runner there is nothing to claim"
        );
    }

    /// A probe runs only for what a mechanism claimed: it costs a file open and a
    /// socket.
    #[cfg(unix)]
    #[test]
    fn only_a_claimed_layer_is_put_to_its_probe() {
        let claimed = measure_facts(
            claimed_by_mechanism(
                Protections {
                    limits: true,
                    ..Protections::default()
                },
                External { runner: true },
            ),
            Profile::Documents,
            Grants::default(),
            Measure::Thorough,
            None,
        );
        assert!(claimed.files.is_some(), "a claimed files layer is measured");
        assert!(
            claimed.network.is_some(),
            "and so is a claimed network layer"
        );
        assert!(
            claimed.process.is_some(),
            "the process probe runs in the thorough tier"
        );

        let unclaimed = measure_facts(
            Protections {
                limits: true,
                ..Protections::default()
            },
            Profile::Documents,
            Grants::default(),
            Measure::Cheap,
            None,
        );
        assert!(
            unclaimed.files.is_none() && unclaimed.network.is_none(),
            "a layer no mechanism installed is not put to a probe"
        );
        assert!(
            unclaimed.process.is_none(),
            "and the cheap tier runs no process probe at all"
        );
    }

    /// The macOS shape: the runner claims three layers, the probes back files and the
    /// network up, and the report must say *measured* rather than installed.
    #[test]
    fn a_runner_claim_backed_by_its_probes_is_measured() {
        let claimed = claimed_by_mechanism(
            Protections {
                limits: true,
                ..Protections::default()
            },
            External { runner: true },
        );
        let facts = Facts {
            files: Some(true),
            network: Some(true),
            system: Some("readable"),
            // The profile denies the fork, so there was no copy to try `execve`
            // in and the process item stands unmeasured.
            process: Some(ProcessFacts {
                fork_denied: Some(true),
                exec_denied: None,
            }),
            threads: true,
            ..Facts::default()
        };
        let (protections, detail) = protect(
            claimed,
            vec!["limits=as+1073741824".to_string()],
            External { runner: true },
            Profile::Documents,
            Measure::Thorough,
            &facts,
        );

        assert!(protections.complete());
        assert_eq!(
            appended(detail),
            [
                "runner=external",
                "files=measured-denied",
                "network=measured-denied",
                "system=readable",
                "fork=denied",
                "exec=unmeasured",
                "process=unmeasured",
                "threads=ok",
                "measure=thorough",
            ]
        );
    }

    /// A claim a probe *did* back up stands, and one no probe reached is reported
    /// as what the mechanism installed.
    #[test]
    fn a_measured_layer_stands_and_an_unmeasured_one_is_what_was_installed() {
        let (protections, detail) = claimed(true, true, true);
        let facts = Facts {
            files: Some(true),
            network: Some(true),
            // The fork was refused, so there was no copy to try `execve` in. The
            // claim is not measured — and is not cleared by a measurement that
            // could not happen either.
            process: Some(ProcessFacts {
                fork_denied: Some(true),
                exec_denied: None,
            }),
            threads: true,
            ..Facts::default()
        };
        let (protections, detail) = protect(
            protections,
            detail,
            External { runner: true },
            Profile::Documents,
            Measure::Thorough,
            &facts,
        );
        assert!(protections.complete());
        assert_eq!(
            appended(detail),
            [
                "runner=external",
                "files=measured-denied",
                "network=measured-denied",
                "fork=denied",
                "exec=unmeasured",
                "process=unmeasured",
                "threads=ok",
                "measure=thorough",
            ]
        );

        // The cheap tier is the per-request child: it may not afford the effect
        // probes, so it says what the mechanism installed rather than claiming a
        // measurement it never made.
        let (protections, detail) = claimed(true, true, false);
        let (protections, detail) = protect(
            protections,
            detail,
            External::default(),
            Profile::Documents,
            Measure::Cheap,
            &Facts {
                threads: true,
                ..Facts::default()
            },
        );
        assert!(protections.files && protections.network);
        assert_eq!(
            appended(detail),
            [
                "files=installed",
                "network=installed",
                "threads=ok",
                "measure=cheap",
            ]
        );
    }

    /// Media starts a program by definition, so it cannot carry the process item; the
    /// `fork` fact is the note beside it.
    #[test]
    fn the_media_profile_cannot_claim_the_process_item() {
        let (protections, detail) = claimed(true, true, true);
        let facts = Facts {
            files: Some(true),
            network: Some(true),
            process: Some(ProcessFacts {
                fork_denied: Some(false),
                exec_denied: Some(false),
            }),
            helper_granted: true,
            threads: true,
            ..Facts::default()
        };
        let (protections, detail) = protect(
            protections,
            detail,
            External::default(),
            Profile::Media,
            Measure::Thorough,
            &facts,
        );

        assert!(
            !protections.process,
            "a profile that starts a helper cannot bound process creation"
        );
        assert!(
            protections.limits && protections.files && protections.network,
            "the bound, the files and the network are still established: {protections:?}"
        );
        let tokens = appended(detail);
        assert!(tokens.contains(&"fork=open".to_string()), "{tokens:?}");
        assert!(
            tokens.contains(&"helper=allowed".to_string()),
            "the grant that took is a fact beside the item: {tokens:?}"
        );
    }

    /// A platform that refused the helper's grant has to suppress the note that
    /// says it took, and the note needs the parent to have named a helper at all.
    #[test]
    fn a_refused_helper_grant_is_not_reported_as_allowed() {
        let (protections, detail) = claimed(true, true, true);
        let facts = Facts {
            helper_granted: true,
            helper_refused: Some("Access is denied".to_string()),
            threads: true,
            ..Facts::default()
        };
        let (_, detail) = protect(
            protections,
            detail,
            External::default(),
            Profile::Media,
            Measure::Cheap,
            &facts,
        );
        let tokens = appended(detail);
        assert!(
            tokens.contains(&"helper-grant-refused(Access is denied)".to_string()),
            "{tokens:?}"
        );
        assert!(
            !tokens.contains(&"helper=allowed".to_string()),
            "the refusal is the platform saying the grant did not take: {tokens:?}"
        );

        // No helper grant at all: there is nothing to have failed and nothing to
        // report.
        let (protections, detail) = claimed(true, true, true);
        let (_, detail) = protect(
            protections,
            detail,
            External::default(),
            Profile::Media,
            Measure::Cheap,
            &Facts {
                threads: true,
                ..Facts::default()
            },
        );
        assert!(!appended(detail).contains(&"helper=allowed".to_string()));
    }

    /// The system-tree token describes the files layer it sits beside, and is
    /// dropped with it: it is a fact about that layer's reach, not a claim of
    /// its own.
    #[test]
    fn the_system_tree_token_belongs_to_a_files_layer_that_stands() {
        let (protections, detail) = claimed(true, false, false);
        let facts = Facts {
            files: Some(true),
            system: Some("readable"),
            threads: true,
            ..Facts::default()
        };
        let (_, detail) = protect(
            protections,
            detail,
            External::default(),
            Profile::Documents,
            Measure::Thorough,
            &facts,
        );
        assert!(appended(detail).contains(&"system=readable".to_string()));

        let (protections, detail) = claimed(true, false, false);
        let facts = Facts {
            files: Some(false),
            system: Some("readable"),
            threads: true,
            ..Facts::default()
        };
        let (protections, detail) = protect(
            protections,
            detail,
            External::default(),
            Profile::Documents,
            Measure::Thorough,
            &facts,
        );
        assert!(!protections.files);
        assert!(
            !appended(detail).contains(&"system=readable".to_string()),
            "a cleared layer has no tree to speak of"
        );
    }

    /// Every token the fold can put in a report: adding one fails this test until it
    /// is added here, and removing one until it is removed.
    const FOLD_TOKENS: [&str; 23] = [
        "runner=external",
        "files=measured-denied",
        "files=measured-open",
        "files=installed",
        "network=measured-denied",
        "network=measured-open",
        "network=installed",
        "system=readable",
        "fork=denied",
        "fork=open",
        "fork=unmeasured",
        "exec=denied",
        "exec=open",
        "exec=unmeasured",
        "process=measured-denied",
        "process=measured-open",
        "process=unmeasured",
        "helper-grant-refused(denied)",
        "helper=allowed",
        "threads=ok",
        "threads=failed",
        "measure=cheap",
        "measure=thorough",
    ];

    #[test]
    fn the_fold_says_only_what_this_list_knows_about() {
        let processes = [
            None,
            Some(ProcessFacts {
                fork_denied: Some(false),
                exec_denied: Some(false),
            }),
            Some(ProcessFacts {
                fork_denied: Some(true),
                exec_denied: None,
            }),
            Some(ProcessFacts {
                fork_denied: None,
                exec_denied: Some(true),
            }),
        ];

        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for files in [None, Some(true), Some(false)] {
            for network in [None, Some(true), Some(false)] {
                for system in [None, Some("readable")] {
                    for process in processes {
                        for helper_refused in [None, Some("denied".to_string())] {
                            for helper_granted in [false, true] {
                                for threads in [true, false] {
                                    for measure in [Measure::Cheap, Measure::Thorough] {
                                        for external in
                                            [External::default(), External { runner: true }]
                                        {
                                            for profile in [Profile::Documents, Profile::Media] {
                                                let facts = Facts {
                                                    files,
                                                    network,
                                                    system,
                                                    process,
                                                    helper_refused: helper_refused.clone(),
                                                    helper_granted,
                                                    threads,
                                                };
                                                let (protections, detail) =
                                                    claimed(true, true, true);
                                                let (_, detail) = protect(
                                                    protections,
                                                    detail,
                                                    external,
                                                    profile,
                                                    measure,
                                                    &facts,
                                                );
                                                seen.extend(appended(detail));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let known: std::collections::BTreeSet<String> =
            FOLD_TOKENS.iter().map(|token| token.to_string()).collect();
        assert_eq!(
            seen, known,
            "the fold's vocabulary is exactly what this list says it is"
        );
    }
}
