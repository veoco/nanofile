//! The extraction child process: the only place a document is parsed.
//!
//! Every document is extracted by re-executing this binary as
//! `nanofile extract-worker`, which confines itself ([`sandbox`]) and only then
//! reads the request on stdin. There is no in-process path: if the child cannot
//! be started, or refuses to run without confinement, the document is a
//! *failure* — retried later, never parsed by the server.
//!
//! # Protocol
//!
//! Request, on the child's stdin: `NFX1`, one byte naming the [`Plan`], then the
//! file's bytes to end of file. Reply, on stdout: `T` or `U`, a little-endian
//! `u32` length, then that many bytes of UTF-8 — the text, or the reason the
//! document is not indexable. `--selftest` replaces the request with a report
//! line and no reply.
//!
//! # Why a process, not a thread
//!
//! What a parser can do to the *server* is bounded by what the process carrying
//! it may do. A memory limit, a file boundary and a CPU limit are all per-process
//! on the platforms this ships to, and a parser that overflows its stack or
//! aborts on a failed allocation takes its own process down instead of the
//! server. The panic boundary in [`crate::indexer::extract::guard`] covers
//! panics only; this
//! covers everything a panic cannot.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::indexer::extract::{Extracted, MAX_INDEXED_CONTENT_BYTES, Plan, reason};
use crate::sandbox::{self, Level, Profile, Report, Requirement};

/// The subcommand that turns this binary into the worker.
pub const SUBCOMMAND: &str = "extract-worker";

/// Magic that opens a request and the self-test report.
pub(crate) const MAGIC: &[u8; 4] = b"NFX1";

/// Wall-clock ceiling for one document.
///
/// The slowest legitimate document measured here took 6.6 seconds with the
/// previous PDF parser and a fraction of that with the current one; the child
/// also carries its own CPU limit, so this only has to catch a hang.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Wall-clock ceiling for the startup self-test.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Wall-clock ceiling for one image request: decode, resize, encode.
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Wall-clock ceiling for one media request: ffmpeg plus the resize of the
/// frame it produced. Longer than the others because ffmpeg has to open and
/// seek a whole container before it can hand over one frame.
pub const MEDIA_TIMEOUT: Duration = Duration::from_secs(35);

/// Most bytes of a reply the parent will read.
const MAX_REPLY_BYTES: u64 = (MAX_INDEXED_CONTENT_BYTES + (1 << 20)) as u64;

/// Most bytes of the child's *diagnostics* the parent will hold.
///
/// A parser that logs per page, or a document that makes it loop, would
/// otherwise let the child spend the server's memory through the one channel
/// nothing bounds: the reply is capped and so is the request, and stderr was
/// not. What is kept is the head, which is where the reason is; the rest is
/// drained and dropped so the child never blocks writing into a full pipe.
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

/// How long the parent waits for the protocol pipes after the child is gone.
///
/// The child has already exited by then, so what is left to read is at most one
/// pipe buffer. The deadline exists for the case it is not: a process the child
/// started can hold the inherited pipes open past its parent's death, and an
/// unbounded read there would hold this call — and the caller's extraction
/// permit — for as long as that process lives. Whatever was not read by the
/// deadline is abandoned and the run is decided on what *was* read, which is
/// the fail-closed direction: no reply is a failed document.
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// How long an unavailable sandbox is left alone before it is probed again.
const UNAVAILABLE_RETRY: Duration = Duration::from_secs(300);

/// `STATUS_DLL_INIT_FAILED`, which Windows gives a process whose loader could
/// not finish.
///
/// Nothing of the document has been read at that point — the child confines
/// itself before it reads its request, and the loader runs before that — so this
/// is the environment rather than the file, and the document is a failure the
/// backfill retries rather than a verdict on its bytes.
#[cfg(windows)]
const STATUS_DLL_INIT_FAILED: i32 = 0xC000_0142u32 as i32;

/// Exit code the child uses when it will not run unconfined.
///
/// Nothing the child does on its own uses it, and it is outside the range a
/// shell reserves for "not executable" and "not found".
const EXIT_SANDBOX_UNAVAILABLE: i32 = 125;

/// The prefix of the child's own refusal, which the parent recognises when the
/// exit code alone is not enough (a runner in between may rewrite it).
const SANDBOX_REFUSAL: &str = "extract-worker: sandbox unavailable";

/// Which rung of confinement a child is started on.
///
/// `Confined` is what a document and the probe use. The other two exist for the
/// diagnosis a failed launch prints ([`token_diagnosis`]): they are never a
/// fallback, because a child that only runs on one of them is a child that does
/// not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Whatever this host can give: the runner on macOS, the container and the
    /// restricted token on Windows.
    Confined,
    /// The restricted token without the container, on Windows.
    #[cfg(windows)]
    TokenOnly,
    /// Neither: this process's own token, ideally nothing else.
    #[cfg(windows)]
    Plain,
}

impl Start {
    /// How a rung is named when the chain reports which ones failed.
    fn label(self) -> &'static str {
        match self {
            Start::Confined => "with every layer",
            #[cfg(windows)]
            Start::TokenOnly => "with a token and no container",
            #[cfg(windows)]
            Start::Plain => "without a token or a container",
        }
    }
}

/// The rungs this platform can start a child on, strongest first.
///
/// One rung everywhere but Windows: the macOS runner and the unix layers are
/// applied by the child or by the parent without an alternative, so there is
/// nothing below them to fall back to — and that is the point of the chain, not
/// a gap in it. Windows has two weaker ones, and the container is the part of
/// that launch a host can lose on its own.
fn rungs() -> &'static [Start] {
    #[cfg(windows)]
    {
        &[Start::Confined, Start::TokenOnly, Start::Plain]
    }
    #[cfg(not(windows))]
    {
        &[Start::Confined]
    }
}

/// What the child was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Report the confinement this process could establish, and exit.
    Selftest,
    /// Extract one file, reading the request from stdin.
    Extract,
}

/// What the parent did around this process that the child cannot see itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct External {
    /// The parent wrapped this process in the platform's runner (macOS
    /// `sandbox-exec`), so files, the network and process creation are not its
    /// own layers to establish.
    pub runner: bool,
    /// The parent created this process with a restricted token (Windows).
    pub restricted_token: bool,
}

/// What the parent learned from running the child.
#[derive(Debug)]
pub enum Outcome {
    /// The child produced a verdict for this document.
    Extracted(Extracted),
    /// The child could not be run, or would not run without confinement. An
    /// environment problem: the document is a *failure* the backfill retries.
    Unavailable(String),
    /// The child ran and did not answer — a crash, a kill or a timeout. A
    /// property of the document: it is skipped rather than retried forever.
    Failed(&'static str),
}

/// Whether the worker can run, and how well it confines itself.
#[derive(Debug, Clone)]
pub enum Status {
    /// The child ran and reported its confinement.
    Ready(Report),
    /// The child could not be run, or reported less confinement than the
    /// policy accepts. The string is what to log.
    Unavailable(String),
}

impl Status {
    pub fn level(&self) -> Option<Level> {
        match self {
            Status::Ready(report) => Some(report.level()),
            Status::Unavailable(_) => None,
        }
    }
}

/// Run the child side of the protocol and exit.
pub fn run(
    job: Job,
    external: External,
    profile: Profile,
    requirement: Requirement,
    grants: crate::sandbox::Grants<'_>,
) -> anyhow::Result<()> {
    // The parser limits are process-global and this process never runs the
    // server, so they are set here and nowhere else.
    configure_parser_limits();
    // The self-test is the one run that can afford every effect probe; a
    // request's child is not, and it is not where the decision is made.
    let measure = if job == Job::Selftest {
        sandbox::Measure::Thorough
    } else {
        sandbox::Measure::Cheap
    };
    let mut report = sandbox::confine(
        sandbox::External {
            runner: external.runner,
        },
        profile,
        grants,
        measure,
    );
    if external.restricted_token {
        // A restricted token is a property of the process, not a protection this
        // process installed, so it is reported rather than claimed as one.
        report.detail.push_str(",token=restricted");
    }

    if job == Job::Selftest {
        // Parsing is the other half of the answer: the report says what was
        // installed, this says the installed thing can still do its work.
        report
            .detail
            .push_str(&format!(",parse={}", probe_parse(profile, grants)));
        println!("{}", selftest_line(&report));
        return Ok(());
    }

    if let Err(refusal) = requirement.accepts(&report) {
        eprintln!(
            "{SANDBOX_REFUSAL}: {} ({})",
            refusal.reason(),
            report.detail
        );
        std::process::exit(EXIT_SANDBOX_UNAVAILABLE);
    }

    match profile {
        Profile::Documents => {
            let (plan, data) = read_request()?;
            write_reply(crate::indexer::extract::extract(plan, data))
        }
        Profile::Images => crate::sandbox::jobs::images::serve(),
        Profile::Media => crate::sandbox::jobs::media::serve(grants),
    }
}

/// Print what the probe a serving process runs at startup found, and exit.
///
/// `extract-worker --probe` calls this. It starts the child exactly as the
/// server does — the platform runner around it included — which makes it the
/// only way to observe the macOS profile's effect from outside, and what CI
/// asserts on.
pub fn probe_report(profile: Profile) -> anyhow::Result<()> {
    match status(profile) {
        Status::Ready(report) => {
            let mut line = report.line();
            // The parent's half of the answer: the child can say whether its
            // token is an AppContainer's, and only the parent knows what it asked
            // for and what came back.
            if let Some(reason) = sandbox::container_shortfall() {
                line.push_str(&format!(",container={reason}"));
            }
            println!("{line} text_chars={MAX_INDEXED_CONTENT_BYTES}");
            Ok(())
        }
        Status::Unavailable(reason) => {
            anyhow::bail!("the extraction worker is not available: {reason}")
        }
    }
}

/// The parser limits the child sets for itself.
pub fn configure_parser_limits() {
    crate::indexer::extract::configure_limits();
}

/// The self-test line: the confinement report, plus the parser limits this
/// process set for itself, which is the one thing about the child the parent
/// cannot otherwise see.
fn selftest_line(report: &Report) -> String {
    format!(
        "{} text_chars={} ",
        report.line(),
        MAX_INDEXED_CONTENT_BYTES
    )
    .trim_end()
    .to_string()
}

/// Do the profile's own work here, under the confinement just installed.
///
/// One whitespace-free verdict per item, so the report stays a line the parent
/// can parse and a probe step can `grep`. What a caller can act on is "this
/// confined worker cannot do its job", and *why* belongs to the job's own tests,
/// which can say more than a token can.
fn probe_parse(profile: Profile, grants: crate::sandbox::Grants<'_>) -> String {
    match profile {
        Profile::Documents => document_verdicts(),
        Profile::Images => crate::sandbox::jobs::images::probe(),
        Profile::Media => crate::sandbox::jobs::media::probe(grants),
    }
}

/// Parse the probe documents, in the confined child.
///
/// Every way a parser can decline reads as `unsupported`: a panic caught by
/// [`crate::indexer::extract::guard`], a document the reader will not open, a
/// plan that stopped being supported.
fn document_verdicts() -> String {
    crate::indexer::extract::PROBE_DOCUMENTS
        .iter()
        .map(|document| {
            let verdict =
                match crate::indexer::extract::extract(document.plan, document.bytes.to_vec()) {
                    crate::indexer::extract::Extracted::Text(text)
                        if text.contains(document.word) =>
                    {
                        "ok"
                    }
                    crate::indexer::extract::Extracted::Text(_) => "no-text",
                    crate::indexer::extract::Extracted::Unsupported(_) => "unsupported",
                };
            format!("{}-{verdict}", document.name)
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Read one request: magic, plan byte, then the file to end of input.
fn read_request() -> anyhow::Result<(Plan, Vec<u8>)> {
    let mut stdin = std::io::stdin().lock();
    let mut prologue = [0u8; 5];
    stdin.read_exact(&mut prologue)?;
    if &prologue[..4] != MAGIC {
        anyhow::bail!("the request does not start with the protocol magic");
    }
    let plan = Plan::from_tag(prologue[4])
        .ok_or_else(|| anyhow::anyhow!("unknown plan tag {}", prologue[4]))?;

    let mut data = Vec::new();
    stdin.read_to_end(&mut data)?;
    Ok((plan, data))
}

/// Write one reply frame: tag, little-endian `u32` length, body.
pub(crate) fn write_frame(tag: u8, body: &[u8]) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&[tag])?;
    stdout.write_all(&(body.len() as u32).to_le_bytes())?;
    stdout.write_all(body)?;
    stdout.flush()?;
    Ok(())
}

/// Write the reply to a document extraction.
fn write_reply(extracted: Extracted) -> anyhow::Result<()> {
    let (tag, body) = match extracted {
        Extracted::Text(text) => (b'T', text.into_bytes()),
        Extracted::Unsupported(reason) => (b'U', reason.as_bytes().to_vec()),
    };
    write_frame(tag, &body)
}

/// What a confined job produced.
#[derive(Debug)]
pub enum RunOutcome {
    /// The child answered: the reply's tag and its body, already framed and
    /// checked.
    Reply { tag: u8, body: Vec<u8> },
    /// The environment: the sandbox or the child's loader refused.
    Unavailable(String),
    /// The request itself: a crash, a kill, a timeout or a bad reply.
    Failed(&'static str),
}

/// Run one confined request for `profile` and collect its reply.
///
/// The request is written verbatim, so each job owns its own framing; the reply
/// comes back as its tag and body, which is what lets one transport carry text,
/// an encoded image and JSON.
pub fn run_request(
    profile: Profile,
    timeout: Duration,
    request: Vec<u8>,
    grants: crate::sandbox::Grants<'_>,
) -> RunOutcome {
    if let Status::Unavailable(why) = status(profile) {
        return RunOutcome::Unavailable(why);
    }
    let Some(invocation) = invocation(configured_requirement(), profile, grants) else {
        return RunOutcome::Unavailable("cannot resolve the sandbox worker".to_string());
    };

    // The rung the probe settled on: a request's child is started the way the
    // one that reported was, and the probe ran before any request could.
    let start = rung(profile);
    let run = match run_child(&invocation, Some(request), timeout, start) {
        Ok(run) => run,
        Err(error) => {
            return RunOutcome::Unavailable(format!("cannot start the sandbox worker: {error}"));
        }
    };
    if run.timed_out {
        tracing::warn!("extract-worker: no answer within {timeout:?}; killing it");
        return RunOutcome::Failed(reason::TIMED_OUT);
    }
    match verdict(run.exit_code, &run.stdout, &run.stderr) {
        Verdict::Reply { tag, body } => RunOutcome::Reply { tag, body },
        Verdict::Unavailable(why) => RunOutcome::Unavailable(why),
        Verdict::Failed(why) => RunOutcome::Failed(why),
    }
}

/// Extract one document in the child, or explain why that did not happen.
///
/// The caller decides what a failure means: an [`Outcome::Unavailable`] is the
/// environment, an [`Outcome::Failed`] is the document.
pub fn extract(plan: Plan, data: Vec<u8>) -> Outcome {
    // The switch is the admin's own choice, not an environment problem: the
    // document is skipped (and not retried forever) rather than failed.
    if !configured_requirement().enabled {
        return Outcome::Extracted(Extracted::Unsupported(reason::SANDBOX_OFF));
    }

    let mut request = Vec::with_capacity(MAGIC.len() + 1 + data.len());
    request.extend_from_slice(MAGIC);
    request.push(plan.tag());
    request.extend_from_slice(&data);

    match run_request(
        Profile::Documents,
        TIMEOUT,
        request,
        crate::sandbox::Grants::default(),
    ) {
        RunOutcome::Reply { tag: b'T', body } => match String::from_utf8(body) {
            Ok(text) => Outcome::Extracted(Extracted::Text(text)),
            Err(_) => Outcome::Failed(reason::FAILED),
        },
        RunOutcome::Reply { tag: b'U', body } => match std::str::from_utf8(&body) {
            Ok(text) => Outcome::Extracted(Extracted::Unsupported(reason::intern(text))),
            Err(_) => Outcome::Failed(reason::FAILED),
        },
        RunOutcome::Reply { .. } => Outcome::Failed(reason::FAILED),
        RunOutcome::Unavailable(why) => Outcome::Unavailable(why),
        RunOutcome::Failed(why) => Outcome::Failed(why),
    }
}

/// The confinement status of one profile, probed once and cached.
///
/// Per profile rather than per process: the media profile may execute ffmpeg
/// where the others may not, so the two reports answer different questions and
/// one must never stand in for the other.
pub fn status(profile: Profile) -> Status {
    let cache = STATUS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = cache.entry(profile).or_default();

    if let Some(status) = &entry.status {
        return status.clone();
    }
    if let Some(retry_at) = entry.retry_at
        && Instant::now() < retry_at
    {
        return Status::Unavailable(PROBE_PENDING.to_string());
    }

    let status = probe(profile);
    match &status.1 {
        Status::Ready(report) => {
            tracing::info!(
                profile = profile.as_str(),
                level = report.level().as_str(),
                detail = report.detail.as_str(),
                "sandbox confinement"
            );
            if report.level() < Level::Full {
                // Why the protection is missing, where the parent is the only
                // one that can say: a child whose token is not an AppContainer's
                // reports `files=open`, and the reason the launch produced no
                // container exists nowhere but here.
                match sandbox::container_shortfall() {
                    Some(reason) => tracing::warn!(
                        profile = profile.as_str(),
                        detail = report.detail.as_str(),
                        container = reason,
                        "the sandbox runs with less confinement than this host can give"
                    ),
                    None => tracing::warn!(
                        profile = profile.as_str(),
                        detail = report.detail.as_str(),
                        "the sandbox runs with less confinement than this host can give"
                    ),
                }
            }
            entry.rung = Some(status.0);
            entry.status = Some(status.1.clone());
        }
        Status::Unavailable(reason) => {
            tracing::error!(
                profile = profile.as_str(),
                reason = reason.as_str(),
                "the sandbox is not available; this feature will not run. Check the log line \
                 above, and see the Sandbox settings page: `sandbox.enabled` is the switch over \
                 these features, `sandbox.min_level` is the grade this host must reach, and a \
                 container may need the Landlock syscalls allowed."
            );
            entry.retry_at = Some(Instant::now() + UNAVAILABLE_RETRY);
        }
    }
    status.1
}

/// Detail for a probe that is waiting out [`UNAVAILABLE_RETRY`].
const PROBE_PENDING: &str = "the extraction sandbox is unavailable (a probe is pending)";

/// Point the worker at a specific binary instead of this process's own.
///
/// Only tests need it: Cargo builds the binary and the integration test in
/// separate places, and the test process cannot re-execute itself. Returns
/// whether *this* call set it — the value is process-global and set once, so a
/// test that deliberately points it at something that cannot run knows to
/// expect the unavailable path.
pub fn configure_executable(path: PathBuf) -> bool {
    EXECUTABLE.set(path).is_ok()
}

/// Set the sandbox requirement the server resolved from `[sandbox]`.
pub fn configure_requirement(requirement: Requirement) {
    let _ = REQUIREMENT.set(requirement);
}

fn configured_requirement() -> Requirement {
    REQUIREMENT.get().copied().unwrap_or_default()
}

#[derive(Default)]
struct Cache {
    status: Option<Status>,
    retry_at: Option<Instant>,
    /// The rung the probe last got a report from, for the requests that follow.
    rung: Option<Start>,
}

static STATUS: OnceLock<Mutex<HashMap<Profile, Cache>>> = OnceLock::new();
static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
static REQUIREMENT: OnceLock<Requirement> = OnceLock::new();
/// The configured media helper, which the media profile may execute.
static HELPER: OnceLock<PathBuf> = OnceLock::new();

/// Set the media helper the media profile may execute.
///
/// Process-global because every request for that profile has to be granted the
/// same binary, and the value comes from `storage.ffmpeg_path` at startup.
pub fn configure_helper(path: PathBuf) {
    let _ = HELPER.set(path);
}

/// The resolved media helper, when one was configured and found.
pub fn helper() -> Option<PathBuf> {
    HELPER.get().cloned()
}

/// Resolve a configured helper to an absolute path.
///
/// A grant is a rule about one file, so it cannot be written for the name
/// `ffmpeg`: the child's environment is cleared, and its working directory is
/// `/`, so a name would resolve nowhere. A configured command name is therefore
/// looked up on this process's own `PATH` once, at startup, and the absolute
/// path is what every later request names.
pub fn resolve_helper(configured: &str) -> PathBuf {
    let path = Path::new(configured);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path_var) {
            let candidate = directory.join(path);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

/// The grants a probe of `profile` runs under.
///
/// The media probe gets the helper and no source: a probe has no request, and
/// what it checks is that the helper can be executed at all under the profile.
fn probe_grants(profile: Profile) -> crate::sandbox::Grants<'static> {
    let helper = profile
        .runs_helper()
        .then(|| HELPER.get().map(|path| path.as_path()))
        .flatten();
    crate::sandbox::Grants {
        helper,
        source: None,
    }
}

/// The rung the probe settled on for `profile`, for the requests that follow.
fn rung(profile: Profile) -> Start {
    let Some(cache) = STATUS.get() else {
        return Start::Confined;
    };
    let cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .get(&profile)
        .and_then(|entry| entry.rung)
        .unwrap_or(Start::Confined)
}

/// Run the child once with `--selftest` and read its report.
///
/// Also answers which rung produced it, so a request is started the same way.
/// The answer is returned rather than recorded here: the caller holds the status
/// cache's lock, and a helper that took it again would deadlock.
fn probe(profile: Profile) -> (Start, Status) {
    let Some(invocation) = invocation(configured_requirement(), profile, probe_grants(profile))
    else {
        return (
            Start::Confined,
            Status::Unavailable("cannot resolve the sandbox worker".to_string()),
        );
    };

    // The chain, strongest first, and the *child's own report* is what accepts a
    // rung: a host whose container will not start still gets a confined child
    // under the token, and the report it prints says which of the two it is
    // (the child reads its own token rather than taking the parent's word). What
    // the parent must not do is decide from the launch having succeeded — the
    // failure this walks past is a child that is created and then dies in its
    // loader, which no creation error reports.
    let mut failures: Vec<String> = Vec::new();
    for &start in rungs() {
        match probe_on(&invocation, profile, start) {
            Ok(report) => return (start, accept(report)),
            Err(why) => failures.push(format!("{}: {why}", start.label())),
        }
    }

    (Start::Confined, Status::Unavailable(failures.join("; ")))
}

/// One rung's answer, or why it did not give one.
fn probe_on(invocation: &Invocation, profile: Profile, start: Start) -> Result<Report, String> {
    let run = run_child(invocation, None, PROBE_TIMEOUT, start)
        .map_err(|error| format!("cannot start the sandbox worker: {error}"))?;
    if let Verdict::Unavailable(why) = verdict(run.exit_code, &run.stdout, &run.stderr) {
        return Err(why);
    }

    let line = String::from_utf8_lossy(
        run.stdout
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default(),
    );
    let line = line.trim();
    // A child that never printed its report says why on stderr, and the probe is
    // the only place that can be read: `tracing` has no subscriber on this path,
    // so the summary is the whole diagnosis.
    let report = Report::parse(line)
        .ok_or_else(|| format!("unreadable self-test line {line:?}: {}", run.summary()))?;
    // A report for another profile is not this profile's answer: the two may
    // hold different powers (only media may execute a helper), so accepting one
    // for the other would be a decision made on the wrong facts.
    if report.profile != profile {
        return Err(format!(
            "the child reported profile={} for a profile={} probe",
            report.profile.as_str(),
            profile.as_str()
        ));
    }
    Ok(report)
}

/// Whether the report the probe got is one the requirement accepts.
///
/// Decided once, here, rather than left to each child: a host that cannot give
/// what the settings ask for is an environment problem the operator has to see
/// once at startup — with the reason — rather than one spawn per request that
/// ends in exit 125 and a warning nothing aggregates. The child keeps the same
/// check before it reads a request; this is the half that makes the shortfall
/// visible and stops the spawns.
fn accept(report: Report) -> Status {
    match configured_requirement().accepts(&report) {
        Ok(()) => Status::Ready(report),
        Err(refusal) => Status::Unavailable(format!(
            "the sandbox refused: {} — this host gives `{}` confinement: {}",
            refusal.reason(),
            report.level().as_str(),
            report.detail
        )),
    }
}

/// The invocation that starts the child, wrapped in the platform runner.
struct Invocation {
    program: OsString,
    args: Vec<OsString>,
}

/// Build the argv for one child, wrapping it in the platform's runner if it has
/// one (macOS, where only `sandbox-exec` can apply a Seatbelt profile).
fn invocation(
    requirement: Requirement,
    profile: Profile,
    grants: crate::sandbox::Grants<'_>,
) -> Option<Invocation> {
    let exe = EXECUTABLE
        .get()
        .cloned()
        .or_else(|| std::env::current_exe().ok())?;
    // The macOS profile names this path in a `(literal …)`, and Seatbelt matches
    // the path the filesystem resolved rather than the one that was typed, so a
    // binary reached through a symlinked directory has to be resolved before it
    // is written down — and before it is started, so the two agree. Windows is
    // left alone: `canonicalize` there returns a `\\?\` path, which is not what
    // this binary was invoked as.
    #[cfg(target_os = "macos")]
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);

    let mut args: Vec<OsString> = vec![
        OsString::from(SUBCOMMAND),
        OsString::from("--min-level"),
        OsString::from(requirement.min_level.as_str()),
        OsString::from("--profile"),
        OsString::from(profile.as_str()),
    ];
    if !requirement.enabled {
        args.push(OsString::from("--sandbox-off"));
    }
    // The child needs the granted paths on its own command line: Landlock and
    // Seatbelt rules have to be installed before the request is read, so they
    // cannot travel inside it.
    if let Some(helper) = grants.helper {
        args.push(OsString::from("--ffmpeg"));
        args.push(helper.as_os_str().to_os_string());
    }
    if let Some(source) = grants.source {
        args.push(OsString::from("--src"));
        args.push(source.as_os_str().to_os_string());
    }

    match sandbox::runner(&exe, profile, grants) {
        Some(runner) => {
            // The child has to know its files, network and process confinement
            // come from the wrapper rather than from its own protections.
            args.push(OsString::from("--seatbelt"));
            let mut wrapped: Vec<OsString> = std::iter::once(OsString::from(runner.program))
                .chain(runner.args.into_iter().map(OsString::from))
                .collect();
            wrapped.push(exe.into_os_string());
            wrapped.extend(args);
            let program = wrapped.remove(0);
            Some(Invocation {
                program,
                args: wrapped,
            })
        }
        None => Some(Invocation {
            program: exe.into_os_string(),
            args,
        }),
    }
}

/// A started child, whichever way the platform had to start it.
///
/// Windows needs `CreateProcessAsUser` to give the child a restricted token,
/// which `std::process::Command` cannot do; everywhere else the standard child
/// is used. The pipes and the wait are what this hides.
enum Child {
    #[cfg(not(windows))]
    Standard(std::process::Child),
    #[cfg(windows)]
    Windows(sandbox::WindowsChild),
}

impl Child {
    /// The child's exit code, or `None` while it is still running.
    ///
    /// A death by signal has no code; the negative signal number stands in for
    /// it, which no exit code can be confused with.
    fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        match self {
            #[cfg(not(windows))]
            Child::Standard(child) => {
                use std::os::unix::process::ExitStatusExt;
                Ok(child.try_wait()?.map(|status| match status.code() {
                    Some(code) => code,
                    // A death by signal has no code, so it is reported as the
                    // negative signal number: `-9` says the kernel killed it,
                    // `-6` says it aborted, and neither is a code the child
                    // itself produces.
                    None => -status.signal().unwrap_or(0),
                }))
            }
            #[cfg(windows)]
            Child::Windows(child) => child.try_wait(),
        }
    }

    fn kill(&mut self) {
        match self {
            #[cfg(not(windows))]
            Child::Standard(child) => {
                let _ = child.kill();
            }
            #[cfg(windows)]
            Child::Windows(child) => child.kill(),
        }
    }

    /// Kill the child and whatever it started.
    ///
    /// The child leads its own process group on unix ([`spawn_child`]), so the
    /// group is what gets the signal: killing the leader alone leaves a forked
    /// copy holding the protocol pipes, and the read after this would then wait
    /// on a pipe nobody will close. Windows needs nothing extra here — the
    /// active-process limit keeps the child alone, and the Job Object takes the
    /// rest when it is the parent's (see `sandbox::windows`).
    fn kill_tree(&mut self) {
        #[cfg(unix)]
        {
            // The only variant on unix; Windows starts a child another way and
            // reaches its whole tree through the Job Object instead.
            let Child::Standard(child) = self;
            // `killpg` on the group the child leads; the child's own pid is the
            // group id because it was made a group leader at spawn.
            unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
        }
        self.kill();
    }

    fn wait(&mut self) {
        match self {
            #[cfg(not(windows))]
            Child::Standard(child) => {
                let _ = child.wait();
            }
            #[cfg(windows)]
            Child::Windows(child) => child.wait(),
        }
    }

    /// Take the three protocol streams.
    fn pipes(&mut self) -> Pipes {
        match self {
            #[cfg(not(windows))]
            Child::Standard(child) => Pipes {
                stdin: child
                    .stdin
                    .take()
                    .map(|stdin| Box::new(stdin) as Box<dyn Write + Send>),
                stdout: child
                    .stdout
                    .take()
                    .map(|stdout| Box::new(stdout) as Box<dyn Read + Send>),
                stderr: child
                    .stderr
                    .take()
                    .map(|stderr| Box::new(stderr) as Box<dyn Read + Send>),
            },
            #[cfg(windows)]
            Child::Windows(child) => {
                let (stdin, stdout, stderr) = child.take_pipes();
                Pipes {
                    stdin: stdin.map(|stdin| Box::new(stdin) as Box<dyn Write + Send>),
                    stdout: stdout.map(|stdout| Box::new(stdout) as Box<dyn Read + Send>),
                    stderr: stderr.map(|stderr| Box::new(stderr) as Box<dyn Read + Send>),
                }
            }
        }
    }
}

/// The child's standard streams, boxed so both ways of starting one look alike.
struct Pipes {
    stdin: Option<Box<dyn Write + Send>>,
    stdout: Option<Box<dyn Read + Send>>,
    stderr: Option<Box<dyn Read + Send>>,
}

/// Start the child, with `--selftest` when the parent wants a report.
#[cfg(not(windows))]
fn spawn_child(invocation: &Invocation, selftest: bool, _start: Start) -> std::io::Result<Child> {
    let mut command = Command::new(&invocation.program);
    command.args(&invocation.args);
    if selftest {
        command.arg("--selftest");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A parser has no business reading the server's environment: it holds
        // the master secret, the storage keys and every path this deployment
        // resolved.
        .env_clear()
        .env("RUST_BACKTRACE", "0")
        // Nothing here is relative, and the server's own directory is not the
        // child's business.
        .current_dir(Path::new("/"));
    // The child leads a process group of its own, so a timeout can end what it
    // started and not only the child itself. macOS is where this matters: the
    // Seatbelt profile has to allow `process-fork` for the parsers' thread, so
    // a document that got code running can leave copies behind, and a copy that
    // holds the protocol pipes would keep this call waiting on a pipe that
    // never closes.
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map(Child::Standard)
}

/// Start the child with a restricted token where the system will make one.
///
/// The environment, the working directory and the handle inheritance are all
/// part of the creation call on Windows, so they are set inside `sandbox`.
#[cfg(windows)]
fn spawn_child(invocation: &Invocation, selftest: bool, start: Start) -> std::io::Result<Child> {
    let mut args = invocation.args.clone();
    if selftest {
        args.push(OsString::from("--selftest"));
    }
    match start {
        Start::Confined => sandbox::spawn(&invocation.program, &args),
        Start::TokenOnly => sandbox::spawn_token_only(&invocation.program, &args),
        Start::Plain => sandbox::spawn_unrestricted(&invocation.program, &args),
    }
    .map(Child::Windows)
}

/// Everything one child run produced.
struct Run {
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    timed_out: bool,
}

impl Run {
    /// One line saying why a run that should have reported did not.
    ///
    /// A loader that cannot map a library, a panic and the kernel each explain
    /// themselves on the child's stderr, which nothing else prints: the parent
    /// logs it at `debug`, and the paths that start a child to diagnose it —
    /// the probe, a CI step — have no subscriber for that to reach.
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.timed_out {
            parts.push("timed out".to_string());
        }
        parts.push(exit_line(self.exit_code));
        parts.push(format!("stdout={} bytes", self.stdout.len()));
        let stderr = self.stderr.trim();
        parts.push(if stderr.is_empty() {
            "stderr=empty".to_string()
        } else {
            format!("stderr={}", clipped(stderr, 400))
        });
        parts.join(", ")
    }
}

/// How the child stopped, in the vocabulary of the platform that stopped it.
///
/// A code below zero is a signal on unix — `-9` is the kernel, `-6` an abort —
/// and an `NTSTATUS` on Windows, where `0xC0000135` is the loader failing to
/// find a library.
fn exit_line(code: Option<i32>) -> String {
    match code {
        #[cfg(unix)]
        Some(code) if code < 0 => format!("killed by signal {}", -code),
        #[cfg(windows)]
        Some(code) if code < 0 => format!("exit=0x{:08X}", code as u32),
        Some(code) => format!("exit={code}"),
        None => "no exit status".to_string(),
    }
}

/// Cap a diagnostic at a length a log line can carry, on a character boundary.
fn clipped(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Start the child, hand it the request and collect what it says.
///
/// The three pipes are drained on their own threads: a parser that logs to
/// stderr while the parent waits for it to exit would otherwise fill a pipe and
/// deadlock both sides.
fn run_child(
    invocation: &Invocation,
    request: Option<Vec<u8>>,
    timeout: Duration,
    start: Start,
) -> std::io::Result<Run> {
    let mut child = spawn_child(invocation, request.is_none(), start)?;
    let pipes = child.pipes();

    let writer = match (request, pipes.stdin) {
        (Some(request), Some(mut stdin)) => Some(std::thread::spawn(move || {
            let _ = stdin.write_all(&request);
            // Dropping the handle closes the child's stdin, which is how it
            // knows the request is complete.
        })),
        _ => None,
    };

    let reader = std::thread::spawn(move || {
        let mut reply = Vec::new();
        if let Some(stdout) = pipes.stdout {
            let _ = stdout.take(MAX_REPLY_BYTES).read_to_end(&mut reply);
        }
        reply
    });

    let drain = std::thread::spawn(move || match pipes.stderr {
        Some(mut stderr) => read_diagnostics(&mut stderr),
        None => String::new(),
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(code)) => break Some(code),
            Ok(None) => {
                if Instant::now() >= deadline {
                    child.kill_tree();
                    child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                child.kill_tree();
                child.wait();
                break None;
            }
        }
    };

    // Bounded, because the child being gone is not the same as the pipes being
    // closed: see `PIPE_DRAIN_TIMEOUT`. A writer that never finishes is dropped
    // the same way — the request it was still sending is a request no reply
    // will be read for.
    let drained = Instant::now() + PIPE_DRAIN_TIMEOUT;
    if let Some(writer) = writer {
        let _ = join_bounded(writer, drained);
    }
    let stdout = join_bounded(reader, drained).unwrap_or_default();
    let stderr = join_bounded(drain, drained).unwrap_or_default();

    Ok(Run {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

/// Wait for a thread that should already be done, giving up at `deadline`.
///
/// `None` means the thread is still running and its handles were dropped with
/// it: the pipe it holds is the reason this exists, and the caller has a
/// fail-closed answer for the missing half (`stdout` empty is no reply,
/// `stderr` empty is no diagnostic).
fn join_bounded<T>(handle: JoinHandle<T>, deadline: Instant) -> Option<T> {
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    handle.join().ok()
}

/// Read a child's diagnostics up to [`MAX_DIAGNOSTIC_BYTES`], then drain.
///
/// Bytes rather than `read_to_string`: a parser's diagnostics are not required
/// to be UTF-8, and a partial read that failed to decode would throw away the
/// reason this is here for. Past the cap the bytes are still read and dropped,
/// so a child that writes more than the cap is never the side that blocks.
fn read_diagnostics(reader: &mut impl Read) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if let Some(room) = MAX_DIAGNOSTIC_BYTES.checked_sub(kept.len())
                    && room > 0
                {
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// What a finished child means, before a job interprets its reply tag.
#[derive(Debug)]
enum Verdict {
    /// The child answered: the reply's tag and body.
    Reply { tag: u8, body: Vec<u8> },
    /// The environment: the child would not run confined, or its loader failed.
    Unavailable(String),
    /// The request: a crash, a kill, a bad reply.
    Failed(&'static str),
}

/// Decide what a finished child means.
///
/// The order matters: a refusal or a runner failure is the *environment*, and
/// both are checked before the reply, because a child that never ran cannot
/// have written one.
fn verdict(exit_code: Option<i32>, stdout: &[u8], stderr: &str) -> Verdict {
    if exit_code == Some(EXIT_SANDBOX_UNAVAILABLE) || stderr.contains(SANDBOX_REFUSAL) {
        return Verdict::Unavailable(detail_or(stderr, "the sandbox refused to run"));
    }
    #[cfg(windows)]
    if exit_code == Some(STATUS_DLL_INIT_FAILED) {
        return Verdict::Unavailable(detail_or(stderr, "the sandbox worker could not initialize"));
    }
    // A process that stopped itself at its memory bound would have no reply to
    // write, but no platform does that any more: the kernel stops the child on
    // Linux (RLIMIT_AS) and Windows (the job's committed-memory cap), and macOS
    // takes the address-space limit. A memory kill therefore arrives as an exit
    // code the platform chose, and the request is retried as a failed one.
    for signature in runner_failure_signatures() {
        if stderr.to_ascii_lowercase().contains(signature) {
            return Verdict::Unavailable(detail_or(stderr, "the sandbox runner failed"));
        }
    }
    if !stderr.trim().is_empty() {
        // Clipped here as well as at the source: what a log line can carry is
        // smaller than what the parent is willing to hold, and the head is the
        // part that says why.
        tracing::debug!("extract-worker: {}", clipped(stderr.trim(), 400));
    }

    match (exit_code, parse_frame(stdout)) {
        (Some(0), Some((tag, body))) => Verdict::Reply { tag, body },
        _ => Verdict::Failed(reason::FAILED),
    }
}

/// Signatures a platform runner prints when it refuses its own profile.
///
/// A runner that fails before starting the command is not a document that
/// cannot be read: it is a sandbox the operator has to fix, and the two must
/// not be confused (the same distinction the DSH sandbox draws).
fn runner_failure_signatures() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &["sandbox-exec: "]
    }
    #[cfg(not(target_os = "macos"))]
    {
        &[]
    }
}

/// The child's own words when it has any, clipped to what a log line carries.
///
/// The refusal line carries the policy and the detail, and the runner failures
/// carry their own sentence; both are at the head, and the tail is the part a
/// document can fill.
fn detail_or(stderr: &str, fallback: &str) -> String {
    let detail = stderr.trim();
    if detail.is_empty() {
        fallback.to_string()
    } else {
        clipped(detail, 512)
    }
}

/// Read one reply frame into its tag and body.
///
/// The framing is the same for every job — tag, little-endian `u32` length,
/// body — so a reply from one profile can never be read as another's: the tag
/// says which job wrote it, and the caller checks that.
fn parse_frame(reply: &[u8]) -> Option<(u8, Vec<u8>)> {
    let (&tag, rest) = reply.split_first()?;
    if rest.len() < 4 {
        return None;
    }
    let length = u32::from_le_bytes(rest[..4].try_into().ok()?) as usize;
    // Trailing bytes would mean the two sides disagree about the framing.
    if rest.len() != 4 + length {
        return None;
    }
    Some((tag, rest[4..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The child's half of the protocol, without a sandbox: read a request from
    /// `reader`, write the reply to `writer`.
    fn serve(reader: &mut impl Read, writer: &mut impl Write) -> anyhow::Result<()> {
        let mut prologue = [0u8; 5];
        reader.read_exact(&mut prologue)?;
        assert_eq!(&prologue[..4], MAGIC);
        let plan = Plan::from_tag(prologue[4]).expect("known plan");
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;
        let (tag, body) = match crate::indexer::extract::extract(plan, data) {
            Extracted::Text(text) => (b'T', text.into_bytes()),
            Extracted::Unsupported(reason) => (b'U', reason.as_bytes().to_vec()),
        };
        writer.write_all(&[tag])?;
        writer.write_all(&(body.len() as u32).to_le_bytes())?;
        writer.write_all(&body)?;
        Ok(())
    }

    fn request(plan: Plan, data: &[u8]) -> Vec<u8> {
        let mut request = Vec::from(&MAGIC[..]);
        request.push(plan.tag());
        request.extend_from_slice(data);
        request
    }

    #[test]
    fn a_text_reply_round_trips() {
        let reply = {
            let mut out = Vec::new();
            let mut input = std::io::Cursor::new(request(Plan::Text, b"zebraquartz"));
            serve(&mut input, &mut out).expect("serve");
            out
        };
        assert_eq!(parse_frame(&reply), Some((b'T', b"zebraquartz".to_vec())));
    }

    /// A document goes through the same protocol, and its verdict comes back as
    /// the reason the parent records.
    #[test]
    fn a_refusal_reply_round_trips_as_its_reason() {
        let reply = {
            let mut out = Vec::new();
            let mut input = std::io::Cursor::new(request(
                Plan::Document(crate::indexer::extract::Document::Pdf),
                b"not a pdf",
            ));
            serve(&mut input, &mut out).expect("serve");
            out
        };
        assert_eq!(
            parse_frame(&reply),
            Some((b'U', reason::PARSE.as_bytes().to_vec()))
        );
    }

    /// The child sets the parser limits for itself; the parent never runs a
    /// parser, so `lib.rs` cannot do it for it.
    #[test]
    fn the_child_sets_the_parser_limits() {
        configure_parser_limits();
        assert_eq!(
            office_oxide::limits::max_text_chars(),
            MAX_INDEXED_CONTENT_BYTES
        );
    }

    /// A child that dies without reporting has to say so in one line: the load
    /// error, the panic or the signal is the whole diagnosis.
    #[test]
    fn a_silent_run_summarises_how_it_died() {
        let run = |exit_code, stderr: &str, timed_out| Run {
            exit_code,
            stdout: Vec::new(),
            stderr: stderr.to_string(),
            timed_out,
        };
        let killed = run(Some(-9), "  ", false).summary();
        #[cfg(unix)]
        {
            assert!(killed.contains("killed by signal 9"), "{killed}");
        }
        #[cfg(windows)]
        {
            assert!(killed.contains("exit=0xFFFFFFF7"), "{killed}");
        }
        assert!(killed.contains("stderr=empty"), "{killed}");
        assert!(killed.contains("stdout=0 bytes"), "{killed}");
        let aborted = run(Some(-6), "dyld: Library not loaded: /usr/lib/x", false).summary();
        assert!(aborted.contains("dyld: Library not loaded"), "{aborted}");
        assert!(run(Some(125), "", false).summary().contains("exit=125"));
        assert!(run(None, "", true).summary().contains("timed out"));
        assert!(run(None, "", false).summary().contains("no exit status"));
    }

    /// The child may print a page of parser noise; a log line carries a line.
    #[test]
    fn a_long_diagnostic_is_clipped_on_a_character_boundary() {
        assert_eq!(clipped("short", 10), "short");
        assert_eq!(clipped("abcdef", 3), "abc…");
        let wide = "é".repeat(4);
        assert_eq!(clipped(&wide, 5), "éé…", "no half character");
    }

    #[test]
    fn the_self_test_line_carries_the_report_and_the_parser_budget() {
        let line = selftest_line(&Report {
            profile: Profile::Documents,
            protections: sandbox::Protections {
                limits: true,
                files: true,
                network: true,
                process: true,
            },
            detail: "landlock_abi=6,seccomp=on".to_string(),
        });
        let report = Report::parse(&line).expect("the parent parses it");
        assert_eq!(report.level(), Level::Full);
        assert!(
            line.contains(&format!("text_chars={MAX_INDEXED_CONTENT_BYTES}")),
            "{line}"
        );
    }

    /// The framing is checked before any tag is believed: a truncated frame or
    /// one with trailing bytes is no reply at all, whatever it claims to be.
    #[test]
    fn a_truncated_or_foreign_reply_is_no_reply() {
        assert_eq!(parse_frame(&[]), None);
        assert_eq!(parse_frame(b"T"), None);
        assert_eq!(parse_frame(b"T\x05\x00\x00\x00ab"), None, "short body");
        assert_eq!(
            parse_frame(b"T\x01\x00\x00\x00ab"),
            None,
            "trailing bytes are a framing disagreement"
        );
        // The framing accepts any tag; which tags a job accepts is the job's
        // own decision, and the tests beside each job pin that.
        assert_eq!(parse_frame(b"?\x00\x00\x00\x00"), Some((b'?', Vec::new())));
        assert_eq!(
            parse_frame(b"T\x02\x00\x00\x00\xff\xfe"),
            Some((b'T', vec![0xff, 0xfe])),
            "the framing carries bytes, not text"
        );
    }

    #[test]
    fn a_refusal_is_an_environment_problem() {
        let outcome = verdict(
            Some(EXIT_SANDBOX_UNAVAILABLE),
            b"",
            "extract-worker: sandbox unavailable: partial (limits=off)",
        );
        assert!(matches!(outcome, Verdict::Unavailable(_)));
    }

    #[test]
    fn a_reply_needs_a_successful_exit() {
        let reply = {
            let mut out = Vec::new();
            let mut input = std::io::Cursor::new(request(Plan::Text, b"hello"));
            serve(&mut input, &mut out).expect("serve");
            out
        };
        assert!(matches!(
            verdict(Some(0), &reply, ""),
            Verdict::Reply { tag: b'T', .. }
        ));
        // Killed after writing: the bytes are there but the run did not finish.
        assert!(matches!(
            verdict(None, &reply, ""),
            Verdict::Failed(reason::FAILED)
        ));
        assert!(matches!(
            verdict(Some(101), &reply, "thread panicked"),
            Verdict::Failed(reason::FAILED)
        ));
    }

    #[test]
    fn a_silent_child_is_a_failed_document() {
        assert!(matches!(
            verdict(Some(134), b"", "fatal runtime error: stack overflow"),
            Verdict::Failed(reason::FAILED)
        ));
    }

    /// The self-test's parse step agrees with the parsers on this host. The
    /// confinement around it is what the probe reports; this is the half that
    /// says the packaged fixtures still yield the word they are looked for.
    #[test]
    fn the_self_test_parses_the_packaged_documents() {
        configure_parser_limits();
        assert_eq!(
            probe_parse(Profile::Documents, crate::sandbox::Grants::default()),
            "pdf-ok,docx-ok"
        );
    }

    #[test]
    fn an_unreadable_report_line_is_not_a_status() {
        assert!(Report::parse("NFX1-sandbox level=full").is_none());
        assert!(Report::parse("something else entirely").is_none());
    }

    /// What a document can make the child print is bounded, and the head is
    /// what survives: the reason is written first, the noise after it.
    #[test]
    fn diagnostics_are_capped_at_the_head() {
        let mut noise = vec![b'x'; MAX_DIAGNOSTIC_BYTES + 4096];
        noise[..4].copy_from_slice(b"head");
        let kept = read_diagnostics(&mut std::io::Cursor::new(noise));
        assert_eq!(kept.len(), MAX_DIAGNOSTIC_BYTES, "the cap is the cap");
        assert!(kept.starts_with("head"), "the head is what is kept");

        // A reader that is not UTF-8 does not lose the bytes that are.
        let kept = read_diagnostics(&mut std::io::Cursor::new(b"reason\xff\xfe".to_vec()));
        assert!(kept.starts_with("reason"));
    }

    /// A thread that never finishes is abandoned at its deadline rather than
    /// joined: this is what keeps a process holding the protocol pipes from
    /// holding the extraction permit with them.
    #[test]
    fn a_thread_that_never_finishes_is_abandoned() {
        let (release, held) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = held.recv();
            7u8
        });
        let started = Instant::now();
        assert_eq!(
            join_bounded(handle, Instant::now() + Duration::from_millis(50)),
            None
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline is what ends the wait"
        );
        drop(release);

        // A thread that is already done still hands its value over.
        let done = std::thread::spawn(|| 7u8);
        assert_eq!(
            join_bounded(done, Instant::now() + Duration::from_secs(5)),
            Some(7)
        );
    }
}
