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
//! Request on stdin: `NFX1`, one byte naming the [`Plan`], then the file's bytes to
//! end of file. Reply on stdout: `T` or `U`, a little-endian `u32` length, then that
//! many bytes of UTF-8 — the text, or the reason the document is not indexable.
//! `--selftest` replaces the request with a report line and no reply; `--probe`
//! runs the startup probe and exits non-zero when the worker is unavailable.
//!
//! # Why a process, not a thread
//!
//! A memory limit, a file boundary and a CPU limit are all per-process, and a
//! parser that overflows its stack or aborts on a failed allocation takes its own
//! process down instead of the server. The panic boundary in
//! `crate::indexer::extract::guard` covers panics only; this covers everything a
//! panic cannot.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
// `Path` is used on every platform by `resolve_helper`, which resolves the
// configured helper before the platform-specific spawn ever sees it.
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::indexer::extract::{Extracted, MAX_INDEXED_CONTENT_BYTES, Plan, reason};
use crate::sandbox::{self, JobVerdict, Profile, Report, Requirement};

/// The subcommand that turns this binary into the worker.
pub const SUBCOMMAND: &str = "extract-worker";

/// Magic that opens a request and the self-test report.
pub(crate) const MAGIC: &[u8; 4] = b"NFX1";

/// Wall-clock ceiling for one document.
///
/// The child also carries its own CPU limit, derived from this same budget so the two cannot
/// disagree, so this only has to catch a hang.
const TIMEOUT: Duration =
    Duration::from_secs(crate::sandbox::profile_wall_seconds(Profile::Documents));

/// Wall-clock ceiling for the startup self-test.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Wall-clock ceiling for one image request: decode, resize, encode.
pub const IMAGE_TIMEOUT: Duration =
    Duration::from_secs(crate::sandbox::profile_wall_seconds(Profile::Images));

/// Wall-clock ceiling for one media request: ffmpeg plus the resize of the frame.
/// Longer than the others because ffmpeg seeks a whole container first.
pub const MEDIA_TIMEOUT: Duration =
    Duration::from_secs(crate::sandbox::profile_wall_seconds(Profile::Media));

/// Most bytes of a reply the parent will read.
const MAX_REPLY_BYTES: u64 = (MAX_INDEXED_CONTENT_BYTES + (1 << 20)) as u64;

/// Most bytes of the child's *diagnostics* the parent will hold.
///
/// The reply is capped and so is the request, and stderr would otherwise be the
/// one unbounded channel. The head is where the reason is; the rest is drained and
/// dropped so the child never blocks on a full pipe.
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

/// How long the parent waits for the protocol pipes after the child is gone.
///
/// What is left to read is at most one pipe buffer, unless a process the child
/// started holds the pipes open past its death; an unbounded read would hold this
/// call and the caller's extraction permit for as long as that process lives. What
/// was not read by the deadline is abandoned, which is fail-closed: no reply is a
/// failed document.
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// How long an unavailable sandbox is left alone before it is probed again.
const UNAVAILABLE_RETRY: Duration = Duration::from_secs(300);

/// `STATUS_DLL_INIT_FAILED`, which Windows gives a process whose loader could
/// not finish.
///
/// Nothing of the document has been read at that point — the child confines itself
/// before it reads its request — so this is the environment, and the document is a
/// retryable failure rather than a verdict on its bytes.
#[cfg(windows)]
const STATUS_DLL_INIT_FAILED: i32 = 0xC000_0142u32 as i32;

/// Exit code the child uses when it will not run unconfined.
///
/// Outside the range a shell reserves for "not executable" and "not found".
const EXIT_SANDBOX_UNAVAILABLE: i32 = 125;

/// The prefix of the child's own refusal, which the parent recognises when the
/// exit code alone is not enough (a runner in between may rewrite it).
const SANDBOX_REFUSAL: &str = "extract-worker: sandbox unavailable";

/// Which creation a child is started with.
///
/// Windows is the only platform with more than one shape, strongest first: the
/// AppContainer with a restricted token, the restricted token alone, and neither.
/// Every one names the same job object, so a host that refuses a layer loses that
/// layer and not the feature. The container is asked for first, and each refusal is
/// remembered so it is not repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Whatever this host can give: the runner on macOS, and on Windows the
    /// container with the restricted token.
    Confined,
    /// The restricted token and the job object, with no container.
    #[cfg(windows)]
    Plain,
    /// The job object alone: this process's own token, which is what a host that
    /// cannot start a restrained child at all still allows. The bound holds.
    #[cfg(windows)]
    Unrestricted,
}

impl Start {
    /// How a creation is named in the log and in the report.
    ///
    /// One word, whitespace-free: a space in a reason would reach the settings page
    /// as `with_every_protection`.
    fn label(self) -> &'static str {
        match self {
            Start::Confined => "confined",
            #[cfg(windows)]
            Start::Plain => "plain",
            #[cfg(windows)]
            Start::Unrestricted => "unrestricted",
        }
    }

    /// The token the report carries for the creation that answered.
    ///
    /// The child measures the container itself (`container=appcontainer` or
    /// `files=open`), but only the parent knows it never asked for one.
    #[cfg(windows)]
    fn token(self) -> &'static str {
        match self {
            Start::Confined => "container",
            Start::Plain => "plain",
            Start::Unrestricted => "unrestricted",
        }
    }

    /// The creation below this one, where the platform has one; `None` ends the
    /// walk.
    fn weaker(self) -> Option<Self> {
        match self {
            Start::Confined => {
                #[cfg(windows)]
                {
                    Some(Start::Plain)
                }
                #[cfg(not(windows))]
                {
                    None
                }
            }
            #[cfg(windows)]
            Start::Plain => Some(Start::Unrestricted),
            #[cfg(windows)]
            Start::Unrestricted => None,
        }
    }
}

/// The facts about one launch that only the parent can see, appended to its report.
///
/// The child measures its own token, limits and refusals; only the parent knows
/// which creation answered, why the strongest did not, and which container flavour
/// the kernel accepted. Putting them on the report puts them where the Sandbox page
/// reads them, and `Report::notes` renders them.
fn with_parent_facts(mut report: Report, start: Start) -> Report {
    #[cfg(windows)]
    {
        report.detail.push_str(&format!(",rung={}", start.token()));
        report.detail.push_str(&format!(
            ",lpac={}",
            if crate::sandbox::lpac() { "on" } else { "off" }
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = start;
    }
    if let Some(reason) = crate::sandbox::container_shortfall() {
        report
            .detail
            .push_str(&format!(",container_refused={reason}"));
    }
    report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    Selftest,
    Extract,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct External {
    /// The parent wrapped this process in the platform's runner (macOS
    /// `sandbox-exec`); the layers are not its own to establish.
    pub runner: bool,
}

#[derive(Debug)]
pub enum Outcome {
    Extracted(Extracted),
    /// The child could not be run, or would not run without confinement. An
    /// environment problem: the document is a *failure* the backfill retries.
    Unavailable(String),
    /// The child ran and did not answer — a crash, a kill or a timeout. A
    /// property of the document: it is skipped rather than retried forever.
    Failed(&'static str),
}

#[derive(Debug, Clone)]
pub enum Status {
    Ready(Report),
    /// The child could not be run, or reported less confinement than the
    /// policy accepts. The string is what to log.
    Unavailable(String),
}

pub fn run(
    job: Job,
    external: External,
    profile: Profile,
    requirement: Requirement,
    grants: crate::sandbox::Grants<'_>,
    probe: Option<&Path>,
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
        probe,
    );
    // The token claim is not repeated: Windows reads its own token back, and no
    // other platform starts a child with one.

    if job == Job::Selftest {
        // The report says what was installed; this says the installed thing can
        // still do its work. Both come from the one measurement.
        let (items, verdict) = probe_parse(profile, grants);
        report.detail.push_str(&format!(",parse={items}"));
        report.job = Some(verdict);
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
/// `extract-worker --probe` calls this. It starts the child exactly as the server
/// does, the platform runner included, which is what CI asserts on.
pub fn probe_report(profile: Profile) -> anyhow::Result<()> {
    match status(profile) {
        Status::Ready(report) => {
            // `probe` already appended the parent's half, so this line, the log
            // and the settings page carry the same report.
            println!("{} text_chars={MAX_INDEXED_CONTENT_BYTES}", report.line());
            Ok(())
        }
        Status::Unavailable(reason) => {
            anyhow::bail!("the extraction worker is not available: {reason}")
        }
    }
}

pub fn configure_parser_limits() {
    crate::indexer::extract::configure_limits();
}

/// The self-test line: the confinement report plus the parser limits this process
/// set for itself.
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
/// One whitespace-free verdict per item, beside the profile's own [`JobVerdict`]:
/// what a caller can act on is "this confined worker cannot do its job", and *why*
/// belongs to the job's own tests.
fn probe_parse(profile: Profile, grants: crate::sandbox::Grants<'_>) -> (String, JobVerdict) {
    match profile {
        Profile::Documents => document_verdicts(),
        Profile::Images => crate::sandbox::jobs::images::probe(),
        Profile::Media => crate::sandbox::jobs::media::probe(grants),
    }
}

/// Parse the probe documents, in the confined child.
///
/// Every way a parser can decline reads as `unsupported`. What separates `ok` from
/// `text-differs` is the *whole* text: a parser that stopped early, or a limit that
/// cut it short, returned something containing the fixture's words but is not doing
/// its job.
fn document_verdicts() -> (String, JobVerdict) {
    let mut overall = JobVerdict::Ok;
    let items = crate::indexer::extract::PROBE_DOCUMENTS
        .iter()
        .map(|document| {
            let (verdict, job) =
                match crate::indexer::extract::extract(document.plan, document.bytes.to_vec()) {
                    crate::indexer::extract::Extracted::Text(text)
                        if text.trim() == document.expected =>
                    {
                        ("ok", JobVerdict::Ok)
                    }
                    crate::indexer::extract::Extracted::Text(text) if text.trim().is_empty() => {
                        ("no-text", JobVerdict::Empty)
                    }
                    crate::indexer::extract::Extracted::Text(_) => {
                        ("text-differs", JobVerdict::Different)
                    }
                    crate::indexer::extract::Extracted::Unsupported(_) => {
                        ("unsupported", JobVerdict::Unsupported)
                    }
                };
            // The answer is the first item that is not `ok`.
            if overall == JobVerdict::Ok {
                overall = job;
            }
            format!("{}-{verdict}", document.name)
        })
        .collect::<Vec<_>>()
        .join(",");
    (items, overall)
}

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

fn write_reply(extracted: Extracted) -> anyhow::Result<()> {
    let (tag, body) = match extracted {
        Extracted::Text(text) => (b'T', text.into_bytes()),
        Extracted::Unsupported(reason) => (b'U', reason.as_bytes().to_vec()),
    };
    write_frame(tag, &body)
}

#[derive(Debug)]
pub enum RunOutcome {
    /// The reply's tag and body, already framed and checked.
    Reply {
        tag: u8,
        body: Vec<u8>,
    },
    /// The environment: the sandbox or the child's loader refused.
    Unavailable(String),
    Failed(&'static str),
}

/// Run one confined request for `profile` and collect its reply.
///
/// The request is written verbatim, so each job owns its framing; the reply comes
/// back as a tag and body, which lets one transport carry text, an image and JSON.
pub fn run_request(
    profile: Profile,
    timeout: Duration,
    request: Vec<u8>,
    grants: crate::sandbox::Grants<'_>,
) -> RunOutcome {
    if let Status::Unavailable(why) = status(profile) {
        return RunOutcome::Unavailable(why);
    }
    // A request's child has no probe file: the files item was measured once, and
    // the child claims it from the mechanism it installed.
    let Some(invocation) = invocation(configured_requirement(), profile, grants, None) else {
        return RunOutcome::Unavailable("cannot resolve the sandbox worker".to_string());
    };

    // The creation the probe settled on. The reason record starts here, because a
    // request's launch is a sequence of its own.
    crate::sandbox::clear_container_shortfall();
    let start = rung(profile);
    let run = match run_child(&invocation, Some(request), timeout, start, profile, grants) {
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
        Verdict::Unavailable(why) => {
            // The probe said ready; a request that finds otherwise is news about
            // the host. Drop the answer so the next call re-probes.
            mark_unavailable(profile);
            RunOutcome::Unavailable(why)
        }
        Verdict::Failed(why) => RunOutcome::Failed(why),
    }
}

/// Extract one document in the child, or explain why that did not happen.
///
/// The caller decides what a failure means: [`Outcome::Unavailable`] is the
/// environment, [`Outcome::Failed`] the document.
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
/// Per profile rather than per process: the media profile may execute ffmpeg where
/// the others may not, so one report must never stand in for the other.
///
/// The probe runs **without** the cache lock: it starts children and may try a
/// second creation, and holding the lock would block every other `status` call on a
/// request path. Two threads that race both probe and store the same answer, which
/// is cheaper than serialising every caller behind one slow launch.
///
/// An answer belongs to the requirement it was measured under. A probe that runs
/// across a settings change is returned but never stored — the change cleared the
/// cache, and publishing would put the old answer back.
pub fn status(profile: Profile) -> Status {
    let cache = STATUS.get_or_init(|| Mutex::new(HashMap::new()));
    let requirement = configured_requirement();

    {
        let cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = cache
            .get(&profile)
            .filter(|entry| entry.requirement == requirement);
        if let Some(status) = entry.and_then(|entry| entry.status.clone()) {
            return status;
        }
        if let Some(retry_at) = entry.and_then(|entry| entry.retry_at)
            && Instant::now() < retry_at
        {
            return Status::Unavailable(PROBE_PENDING.to_string());
        }
    }

    let (start, status) = probe(profile);

    if configured_requirement() != requirement {
        return status;
    }

    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = cache.entry(profile).or_default();
    entry.requirement = requirement;
    match &status {
        Status::Ready(report) => {
            // Logged one field apiece: a log reader greps for the missing layer.
            tracing::info!(
                profile = profile.as_str(),
                limits = report.protections.limits,
                files = report.protections.files,
                network = report.protections.network,
                process = report.protections.process,
                detail = report.detail.as_str(),
                "sandbox confinement"
            );
            if !report.protections.complete() {
                // Why the protection is missing, where only the parent can say: a
                // child whose token is not an AppContainer's reports `files=open`.
                // Read from the report, the cached copy, not from the record, which
                // belongs to the last launch of any kind.
                if let Some(reason) = sandbox::container_refusal(&report.detail) {
                    tracing::warn!(
                        profile = profile.as_str(),
                        detail = report.detail.as_str(),
                        container = reason,
                        "the sandbox runs with less confinement than this host can give"
                    );
                } else {
                    tracing::warn!(
                        profile = profile.as_str(),
                        detail = report.detail.as_str(),
                        "the sandbox runs with less confinement than this host can give"
                    );
                }
            }
            entry.rung = Some(start);
            entry.status = Some(status.clone());
            entry.retry_at = None;
        }
        Status::Unavailable(reason) => {
            tracing::error!(
                profile = profile.as_str(),
                reason = reason.as_str(),
                "the sandbox is not available; this feature will not run. Check the log line \
                 above, and see the Sandbox settings page: `sandbox.enabled` is the switch over \
                 these features, and a container may need the Landlock syscalls allowed."
            );
            entry.retry_at = Some(Instant::now() + UNAVAILABLE_RETRY);
        }
    }
    status
}

/// Mark one profile's cached answer stale after a request's child refused.
///
/// The probe ran before any request, so this is news about the host. Dropping
/// `Ready` and starting the retry window means the next call re-probes, and the
/// requests in between are refused without spawning a doomed child.
fn mark_unavailable(profile: Profile) {
    let Some(cache) = STATUS.get() else {
        return;
    };
    // Read before the cache lock: the requirement slot has its own, and this
    // keeps the two from ever being held at once.
    let requirement = configured_requirement();
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = cache.entry(profile).or_default();
    entry.requirement = requirement;
    entry.status = None;
    entry.retry_at = Some(Instant::now() + UNAVAILABLE_RETRY);
}

/// Detail for a probe that is waiting out `UNAVAILABLE_RETRY`.
const PROBE_PENDING: &str = "the extraction sandbox is unavailable (a probe is pending)";

/// The report a profile can produce on this host, whatever the settings say.
///
/// For an admin who switched the sandbox off and still needs to see what the host
/// *would* give. Measured once and cached: it is a property of the host.
///
/// **Measured, not judged**: the caller ([`page_answer`]) decides what it means. A
/// profile whose items are in place and whose job still failed has answered the
/// question, and the items are how an admin tells "the host cannot confine this"
/// from "the helper cannot run".
///
/// A *failure* is cached only for `UNAVAILABLE_RETRY`: a probe that could not run
/// says nothing about the host.
pub fn capability(profile: Profile) -> Result<Report, String> {
    let cache = CAPABILITY.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match cache.get(&profile) {
            Some(Measured::Report { report }) => return Ok(report.clone()),
            Some(Measured::Failed { why, retry_at }) if Instant::now() < *retry_at => {
                return Err(why.clone());
            }
            _ => {}
        }
    }

    // The requirement the probe runs under is not the request path's: the switch
    // is what the page is asking *about*, so the measurement runs with it on.
    let measured = measure_capability(profile);
    // Nothing below this line judges the report: a measured answer is cached for
    // the process's life even when the profile would not run on it.
    let cached = match &measured {
        Ok(report) => Measured::Report {
            report: report.clone(),
        },
        Err(why) => Measured::Failed {
            why: why.clone(),
            retry_at: Instant::now() + UNAVAILABLE_RETRY,
        },
    };
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(profile, cached);
    measured
}

enum Measured {
    Report { report: Report },
    Failed { why: String, retry_at: Instant },
}

/// What the settings page shows for one profile: the host's answer, and what the
/// host makes of it.
///
/// Both come from the one probe [`capability`] ran — the host half is
/// [`Requirement::accepts`] with the switch *on* plus the profile's job verdict —
/// so the page costs no second child.
///
/// The judgement lives *here* rather than in the probe, so a measured report
/// survives it: a refused profile is a band with rows and a reason. Only
/// [`PageAnswer::Unavailable`], a probe that never answered, has no items. A caller
/// that wants the request path's decision asks [`status`] instead.
pub enum PageAnswer {
    Measured {
        report: Report,
        /// Whether this profile would run if the master switch were on.
        host_runs: bool,
        host_reason: Option<String>,
    },
    Unavailable(String),
}

/// The page's answer for one profile.
///
/// Measured under `Requirement::new(true)`: the switch is the page's other input.
pub fn page_answer(profile: Profile) -> PageAnswer {
    match capability(profile) {
        Ok(report) => match host_verdict(&report) {
            Ok(()) => PageAnswer::Measured {
                report,
                host_runs: true,
                host_reason: None,
            },
            Err(why) => PageAnswer::Measured {
                report,
                host_runs: false,
                host_reason: Some(why),
            },
        },
        Err(why) => PageAnswer::Unavailable(why),
    }
}

/// Whether the host alone can run one profile, and why not when it cannot.
///
/// The master switch is left out: this is the half that belongs to the machine.
fn host_verdict(report: &Report) -> Result<(), String> {
    if let Some(why) = job_refusal(report) {
        return Err(why);
    }
    match Requirement::new(true).accepts(report) {
        Ok(()) => Ok(()),
        Err(refusal) => Err(refusal.reason()),
    }
}

/// Why a profile that passed every item may still not serve requests.
///
/// The items cannot say whether the program the profile exists to run still works.
/// Media's whole job is one program: a profile that installed everything and failed
/// its frame decode would start a doomed helper on every request and 404. No helper
/// configured is `Nothing`, a state an admin chose. Documents and images are not
/// gated on their verdict, and their outcome is recorded per file.
fn job_refusal(report: &Report) -> Option<String> {
    (report.profile.runs_helper() && report.job == Some(JobVerdict::Failed))
        .then(|| format!("the media profile cannot decode a frame: {}", report.detail))
}

/// The report this host gives one profile, with no judgement of it.
///
/// The probe runs with the switch on: `limits` is the floor and every layer above is
/// measured as established or refused. What it means is [`page_answer`]'s question.
fn measure_capability(profile: Profile) -> Result<Report, String> {
    probe_raw(Requirement::new(true), profile).1
}

/// Point the worker at a specific binary instead of this process's own.
///
/// For tests: Cargo builds the binary and the integration test separately, and the
/// test process cannot re-execute itself. Returns whether *this* call set it, since
/// the value is process-global and set once.
pub fn configure_executable(path: PathBuf) -> bool {
    EXECUTABLE.set(path).is_ok()
}

/// Whether a worker binary has already been named.
///
/// The value is process-global and set once, so the caller that must win is the one
/// that went first.
pub fn executable_is_configured() -> bool {
    EXECUTABLE.get().is_some()
}

/// Set the sandbox requirement the server resolved from `[sandbox]`.
///
/// Called at startup and on every Sandbox save. A change to the switch changes what
/// a probe *means*, so the cached answers are dropped with it.
pub fn configure_requirement(requirement: Requirement) {
    *requirement_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = requirement;
    invalidate();
    // A fresh configuration deserves a fresh attempt: the give-up is this
    // process's memory of one refusal, not a verdict about the host.
    CONTAINER_GIVEN_UP.store(false, std::sync::atomic::Ordering::Relaxed);
    // The token record is cleared with the container's: both are one process's
    // memory of a refusal.
    TOKEN_GIVEN_UP.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Drop the cached answers about this host.
///
/// Their inputs — the master switch and the media helper — can change, so the next
/// request and page visit see a fresh measurement.
pub fn invalidate() {
    if let Some(cache) = STATUS.get() {
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
    if let Some(cache) = CAPABILITY.get() {
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

fn requirement_slot() -> &'static Mutex<Requirement> {
    REQUIREMENT.get_or_init(|| Mutex::new(Requirement::default()))
}

/// Whether a launch is still to attempt this platform's container.
///
/// A host whose container reported nothing is not asked again for the life of the
/// process: the refusal is a property of the environment, not of one launch. The
/// settings hook clears the record when the configuration asks again.
///
/// Not `#[cfg(windows)]`: the rule is a decision, and keeping it compiled everywhere
/// is what lets its test run on every runner instead of only on Windows.
static CONTAINER_GIVEN_UP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether a launch has given up the restricted *token* as well.
///
/// The same shape as a container that never came back: one attempt, then the
/// creation below it. What stays is the job object.
static TOKEN_GIVEN_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a launch may still attempt this platform's container.
///
/// Only the Windows request path calls it, but it is compiled and tested everywhere
/// so a mistake cannot hide behind `#[cfg]`.
// `allow` rather than `expect`: the unit test below is what keeps it used under
// `--all-targets`, so an expectation would be unfulfilled where CI lints.
#[cfg_attr(not(windows), allow(dead_code))]
fn container_wanted() -> bool {
    !CONTAINER_GIVEN_UP.load(std::sync::atomic::Ordering::Relaxed)
}

fn give_up_container() {
    CONTAINER_GIVEN_UP.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn give_up_token() {
    TOKEN_GIVEN_UP.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn configured_requirement() -> Requirement {
    *requirement_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct Cache {
    /// The requirement this answer was measured under; `Requirement::default` is
    /// the shipped setting.
    requirement: Requirement,
    status: Option<Status>,
    retry_at: Option<Instant>,
    rung: Option<Start>,
}

static STATUS: OnceLock<Mutex<HashMap<Profile, Cache>>> = OnceLock::new();
/// What each profile measured on this host, whatever the settings say.
static CAPABILITY: OnceLock<Mutex<HashMap<Profile, Measured>>> = OnceLock::new();
static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
static REQUIREMENT: OnceLock<Mutex<Requirement>> = OnceLock::new();
/// The configured media helper, which the media profile may execute.
///
/// Re-set whenever the setting is saved, so a `Mutex` rather than a `OnceLock`.
static HELPER: OnceLock<Mutex<Option<&'static Path>>> = OnceLock::new();

/// Set the media helper the media profile may execute.
///
/// Saving a new `storage.ffmpeg_path` re-points every later grant, and the cached
/// answers measured against the old helper are dropped with it ([`invalidate`]),
/// including whether the media profile gave the opt-out up for it
/// ([`crate::sandbox::forget_plain_container`]).
///
/// An empty path clears the helper: the hook fires on any save, and an admin who
/// empties the setting is saying there is none.
///
/// The path is leaked into the process's lifetime: a child's command line and a
/// Landlock rule are built from a `&'static Path` that must outlive the child.
pub fn configure_helper(path: PathBuf) {
    let slot = HELPER.get_or_init(|| Mutex::new(None));
    let mut installed = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if path.as_os_str().is_empty() {
        *installed = None;
    } else {
        let leaked: &'static Path = Box::leak(path.into_boxed_path());
        *installed = Some(leaked);
    }
    drop(installed);
    crate::sandbox::forget_plain_container();
    invalidate();
}

/// The resolved media helper, when one was configured and found.
pub fn helper() -> Option<&'static Path> {
    let slot = HELPER.get_or_init(|| Mutex::new(None));
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Resolve a configured helper to an absolute path.
///
/// A grant is a rule about one file, so it cannot name `ffmpeg`: the child's
/// environment is cleared and its working directory is `/`, so a bare name resolves
/// nowhere.
///
/// On Windows the bare name resolves through the shell's executable search
/// (`resolve_command`: `PATHEXT`/`.exe` and the image directory), because a manual
/// `is_file()` walk never matches `ffmpeg.exe`. The unix branch walks `PATH`, which
/// needs no extension.
pub fn resolve_helper(configured: &str) -> PathBuf {
    let path = Path::new(configured);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    #[cfg(windows)]
    {
        if let Some(resolved) = resolve_command(configured) {
            return resolved;
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(path_var) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&path_var) {
                let candidate = directory.join(path);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    path.to_path_buf()
}

/// Resolve a bare command name to its full path the way the shell finds an
/// executable.
///
/// `SearchPathW` appends only the extension it is handed, never the shell's
/// `PATHEXT` list, so a bare `ffmpeg` would miss `ffmpeg.exe`. Each `PATHEXT` entry
/// is tried, `.EXE` last so the usual case resolves when `PATHEXT` is unset; then
/// the running image's directory and its `bin/`. Windows-only: the unix walk in
/// [`resolve_helper`] needs no extension.
///
/// It lives here so media grants and the service preflight resolve a name through
/// the same call; the two disagreeing let a bare `ffmpeg` look configured while
/// every thumbnail returned 404.
#[cfg(windows)]
fn resolve_command(name: &str) -> Option<PathBuf> {
    let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    // `.COM;.EXE;.BAT;.CMD` is the usual `PATHEXT`; a host that has it unset
    // still gets `.EXE` from the chain, so `ffmpeg` always finds `ffmpeg.exe`.
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    let extensions = pathext
        .split(';')
        .filter(|extension| extension.starts_with('.'));
    for extension in extensions.chain([".EXE"]) {
        let extension_w: Vec<u16> = extension.encode_utf16().chain(std::iter::once(0)).collect();
        if let Some(resolved) = search_path(&name_w, &extension_w) {
            return Some(resolved);
        }
    }

    // Beside the running image and its `bin/`.
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate in [
            dir.join(name),
            dir.join(format!("{name}.exe")),
            dir.join("bin").join(name),
            dir.join("bin").join(format!("{name}.exe")),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// One `SearchPathW` lookup, with `extension` appended when `name` has none.
/// `None` when the search fails or answers with a relative path.
#[cfg(windows)]
fn search_path(name: &[u16], extension: &[u16]) -> Option<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::SearchPathW;

    let mut buffer = vec![0u16; 1024];
    let mut found = unsafe {
        SearchPathW(
            std::ptr::null(),
            name.as_ptr(),
            extension.as_ptr(),
            buffer.len() as u32,
            buffer.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    // The call reports the length it needs (null included) when the buffer is
    // too small; grow once and retry rather than silently miss a long path.
    if found != 0 && found as usize > buffer.len() {
        buffer = vec![0u16; found as usize];
        found = unsafe {
            SearchPathW(
                std::ptr::null(),
                name.as_ptr(),
                extension.as_ptr(),
                buffer.len() as u32,
                buffer.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
    }
    if found == 0 {
        return None;
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    let resolved = PathBuf::from(String::from_utf16_lossy(&buffer[..end]));
    // Guard against a relative result the call could in theory produce.
    resolved.is_absolute().then_some(resolved)
}

/// The grants a probe of `profile` runs under.
///
/// The media probe gets the helper *and* a source: its job starts by reading the
/// file the parent wrote, so nothing to read would measure half of it.
fn probe_grants<'a>(profile: Profile, source: Option<&'a Path>) -> crate::sandbox::Grants<'a> {
    crate::sandbox::Grants {
        helper: if profile.runs_helper() {
            helper()
        } else {
            None
        },
        source,
    }
}

/// The creation the probe settled on for `profile`.
///
/// Windows answers `Plain` whenever the container is not wanted; every other
/// platform has one creation.
fn rung(profile: Profile) -> Start {
    /// The creation a profile's cached answer was measured in, when the probe
    /// ran. An entry with no answer never settled anything.
    fn cached(profile: Profile) -> Option<Start> {
        STATUS
            .get()?
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&profile)
            .and_then(|entry| entry.rung)
    }

    // When the probe never settled one: the container only where the platform has
    // one and the configuration still wants it.
    #[cfg(windows)]
    let unsettled = if container_wanted() {
        Start::Confined
    } else if TOKEN_GIVEN_UP.load(std::sync::atomic::Ordering::Relaxed) {
        Start::Unrestricted
    } else {
        Start::Plain
    };
    #[cfg(not(windows))]
    let unsettled = Start::Confined;

    // The probe's record wins while it exists: a host that gave the container up
    // must not be asked again, even for a profile the probe never reached.
    cached(profile).unwrap_or(unsettled)
}

/// A private file the files layer's measurement must refuse to read.
///
/// It lives in the server user's own temporary directory, which no profile grants
/// and which on Windows carries no ACE for the container, so the refusal is the
/// sandbox rather than a missing file. A container cannot redirect a read, so this
/// is the *read* side of the files layer. Dropped, and the file deleted, when the
/// probe is over.
struct DenyProbe {
    path: PathBuf,
}

impl DenyProbe {
    /// Create one, or `None` when the temporary directory will not take it:
    /// the platform's own candidates answer in that case.
    fn create() -> Option<Self> {
        let path =
            std::env::temp_dir().join(format!("nanofile-sandbox-probe-{}", uuid::Uuid::new_v4()));
        // `create_new` refuses an existing path (or symlink) rather than following
        // it.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Some(Self { path }),
            Err(_) => None,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DenyProbe {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The media a probe hands the media child, in a file it may read.
///
/// `ffmpeg` seeks a container to find a frame, so a probe measuring the job rather
/// than `-version` needs a file, created and deleted like [`DenyProbe`]. A
/// temporary directory that will not take it leaves no source, and the report says
/// `media-no-source`.
struct SourceProbe {
    path: PathBuf,
}

impl SourceProbe {
    fn create() -> Option<Self> {
        let path =
            std::env::temp_dir().join(format!("nanofile-media-probe-{}.mp4", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        file.write_all(crate::sandbox::jobs::media::PROBE_MEDIA)
            .ok()?;
        file.flush().ok()?;
        // On macOS the profile must name the resolved file, not the typed path (see
        // `media::granted_source`).
        Some(Self {
            path: crate::sandbox::jobs::media::granted_source(&path),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SourceProbe {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The source a probe of `profile` is handed, when its job reads one.
fn source_probe(profile: Profile) -> Option<SourceProbe> {
    if profile.runs_helper() {
        SourceProbe::create()
    } else {
        None
    }
}

/// Start the child in the creation this configuration wants, falling back once
/// on Windows when the container reports nothing.
///
/// A creation is accepted by the *child's own report*, never by the launch having
/// succeeded: a child created and then dead in its loader reports no creation
/// error. Only the container is given up; the job object stays. The reason travels
/// as `skipped0=<creation>:<reason>`, one entry per creation passed over, in order.
///
/// The answer is returned rather than recorded: the caller holds the status cache's
/// lock, and taking it again would deadlock.
fn probe(profile: Profile) -> (Start, Status) {
    probe_under(configured_requirement(), profile)
}

/// One probe under a stated requirement, judged the way the request path judges
/// it.
///
/// The requirement is a parameter so the page can ask what a *different*
/// configuration would give without changing what the request path is judged by.
fn probe_under(requirement: Requirement, profile: Profile) -> (Start, Status) {
    match probe_raw(requirement, profile) {
        (start, Ok(report)) => (start, accept(report, requirement)),
        (start, Err(why)) => (start, Status::Unavailable(why)),
    }
}

/// One probe under a stated requirement, with no judgement of the report.
///
/// The same walk as [`probe_under`]; the answer is the report, so a caller with its
/// own question can ask it about the measurement instead of a refusal.
fn probe_raw(requirement: Requirement, profile: Profile) -> (Start, Result<Report, String>) {
    let probe_file = DenyProbe::create();
    let source_file = source_probe(profile);
    let grants = probe_grants(profile, source_file.as_ref().map(SourceProbe::path));
    let Some(invocation) = invocation(
        requirement,
        profile,
        grants,
        probe_file.as_ref().map(DenyProbe::path),
    ) else {
        return (
            Start::Confined,
            Err("cannot resolve the sandbox worker".to_string()),
        );
    };

    crate::sandbox::clear_container_shortfall();

    // The container where the platform has one and the host has not refused it;
    // everything below is the walk down from here.
    #[cfg(windows)]
    let first = if container_wanted() {
        Start::Confined
    } else if TOKEN_GIVEN_UP.load(std::sync::atomic::Ordering::Relaxed) {
        Start::Unrestricted
    } else {
        Start::Plain
    };
    #[cfg(not(windows))]
    let first = Start::Confined;

    let mut creation = first;
    // One reason per creation passed over: a fallback that leaves no trace is how a
    // host silently runs weaker creations than an operator thinks. The label and the
    // reason are whitespace-free, so each entry stays one field.
    let mut skipped: Vec<String> = Vec::new();

    loop {
        match probe_on(&invocation, profile, grants, creation) {
            Ok(report) => {
                let mut report = with_parent_facts(report, creation);
                for (index, reason) in skipped.iter().enumerate() {
                    report.detail.push_str(&format!(",skipped{index}={reason}"));
                }
                return (creation, Ok(report));
            }
            Err(why) => {
                // Logged too: the operator wants the reason, not only the layer
                // left behind.
                tracing::warn!(
                    profile = profile.as_str(),
                    creation = creation.label(),
                    reason = why.as_str(),
                    "the sandbox worker did not report on this creation"
                );
                let Some(weaker) = creation.weaker() else {
                    return (creation, Err(format!("{}: {why}", creation.label())));
                };
                // Remembered for the process's life: a request must not pay for a
                // creation the probe found dead. The settings hook clears it.
                match creation {
                    Start::Confined => {
                        give_up_container();
                        tracing::warn!(
                            profile = profile.as_str(),
                            reason = why.as_str(),
                            "the AppContainer was given up for this process; the worker runs \
                             with its job and its token, and the report says files=open"
                        );
                    }
                    #[cfg(windows)]
                    Start::Plain => {
                        give_up_token();
                        tracing::warn!(
                            profile = profile.as_str(),
                            reason = why.as_str(),
                            "the restricted token was given up for this process as well; the \
                             worker runs in its job under this process's own token"
                        );
                    }
                    #[cfg(windows)]
                    Start::Unrestricted => {}
                }
                skipped.push(format!("{}: {why}", creation.label()));
                creation = weaker;
            }
        }
    }
}

/// The one launch's answer, or why it did not give one.
///
/// On Windows a launch is retried once without the less privileged container when
/// the child asked for it produced no report: the attribute is accepted at creation,
/// so a container the loader cannot start in succeeds at `CreateProcess` and dies
/// afterwards. The retry keeps the container and gives up only the opt-out.
///
/// The retry is owed to the *shape* of the failure, not its occurrence: a timeout
/// says nothing about the container flavour, and giving the opt-out up for it would
/// make the confinement depend on machine load ([`opt_out_suspect`]).
///
/// The media profile can fail *with* an answer: its report says the helper did not
/// run, and that report is measured again in the plain container before it is
/// believed (`media_in_plain_container`).
fn probe_on(
    invocation: &Invocation,
    profile: Profile,
    grants: crate::sandbox::Grants<'_>,
    start: Start,
) -> Result<Report, String> {
    #[cfg(not(target_os = "windows"))]
    {
        probe_once(invocation, profile, grants, start).map_err(|failure| failure.why)
    }
    #[cfg(target_os = "windows")]
    {
        match probe_once(invocation, profile, grants, start) {
            Ok(report) => media_in_plain_container(invocation, profile, grants, start, report),
            Err(failure) => {
                if !failure.opt_out_suspect || !crate::sandbox::lpac_attempted() {
                    return Err(failure.why);
                }
                tracing::warn!(
                    "extract-worker: the worker did not report under the less privileged \
                     container ({}); starting it in the plain one",
                    failure.why
                );
                crate::sandbox::disable_lpac();
                probe_once(invocation, profile, grants, start).map_err(|failure| failure.why)
            }
        }
    }
}

struct NoReport {
    why: String,
    /// Whether the failure is the shape an unreadable image leaves behind, and worth
    /// a second creation without the opt-out.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    opt_out_suspect: bool,
}

/// Whether a launch that produced no report is worth repeating without the
/// opt-out.
///
/// A child created and dead before it said anything, and one that could not be
/// created at all, are both "nothing ran inside the container" — what an unreadable
/// image looks like from here. A timeout, a refusal on the child's own report and an
/// unreadable report are all evidence about something else.
fn opt_out_suspect(run: &Run) -> bool {
    if run.timed_out {
        return false;
    }
    // The child's own refusal is printed on stderr and exits with the child's own
    // code: it ran inside the container and reported on itself.
    if run.stderr.trim_start().starts_with(SANDBOX_REFUSAL) {
        return false;
    }
    run.exit_code.is_some() && run.stdout.is_empty()
}

/// Measure a media report that says the helper could not run again in the plain
/// container.
///
/// The helper is granted through `ALL APPLICATION PACKAGES` on hosts whose tree
/// carries that ACE, and the opt-out makes the low-box check ignore exactly that
/// principal. So a media report that the helper did not run has the container
/// flavour as its first suspect: the profile asks for the plain one and measures
/// again rather than reporting an environment problem it has not tried to remove.
///
/// One retry, and only for this profile: the report it returns is kept, so
/// `lpac=off` and the second launch's `parse=` travel together.
#[cfg(target_os = "windows")]
fn media_in_plain_container(
    invocation: &Invocation,
    profile: Profile,
    grants: crate::sandbox::Grants<'_>,
    start: Start,
    report: Report,
) -> Result<Report, String> {
    if !media_needs_the_plain_container(
        profile,
        crate::sandbox::lpac(),
        crate::sandbox::plain_container(profile),
        report.job,
    ) {
        return Ok(report);
    }
    tracing::warn!(
        detail = report.detail.as_str(),
        "extract-worker: the media helper did not run inside the less privileged container; \
         measuring the media profile in the plain one"
    );
    crate::sandbox::use_plain_container(profile);
    probe_once(invocation, profile, grants, start).map_err(|failure| failure.why)
}

/// Whether a report says the media profile needs the plain container.
///
/// All four facts are needed: the profile runs a helper, the opt-out was applied (a
/// plain launch would only loop), the profile is not already plain, and the child's
/// verdict is that the helper failed — not that none is configured.
#[cfg(any(target_os = "windows", test))]
fn media_needs_the_plain_container(
    profile: Profile,
    opt_out_applied: bool,
    already_plain: bool,
    job: Option<JobVerdict>,
) -> bool {
    profile.runs_helper() && opt_out_applied && !already_plain && job == Some(JobVerdict::Failed)
}

fn probe_once(
    invocation: &Invocation,
    profile: Profile,
    grants: crate::sandbox::Grants<'_>,
    start: Start,
) -> Result<Report, NoReport> {
    let run =
        run_child(invocation, None, PROBE_TIMEOUT, start, profile, grants).map_err(|error| {
            NoReport {
                why: format!("cannot start the sandbox worker: {error}"),
                // Nothing was created at all, which is as close to "the container
                // refused it" as a creation can get.
                opt_out_suspect: true,
            }
        })?;
    if let Verdict::Unavailable(why) = verdict(run.exit_code, &run.stdout, &run.stderr) {
        // The summary rides along: a child dead in its loader writes nothing, and its
        // exit code is then the only thing that tells the failures apart.
        return Err(NoReport {
            why: format!("{why} [{}]", run.summary()),
            opt_out_suspect: opt_out_suspect(&run),
        });
    }

    let line = String::from_utf8_lossy(
        run.stdout
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default(),
    );
    let line = line.trim();
    // The probe is the only place stderr can be read: `tracing` has no subscriber
    // on this path.
    let report = Report::parse(line).ok_or_else(|| NoReport {
        why: format!("unreadable self-test line {line:?}: {}", run.summary()),
        opt_out_suspect: opt_out_suspect(&run),
    })?;
    // Another profile's report is not this profile's answer: only media may execute
    // a helper.
    if report.profile != profile {
        return Err(NoReport {
            why: format!(
                "the child reported profile={} for a profile={} probe",
                report.profile.as_str(),
                profile.as_str()
            ),
            opt_out_suspect: false,
        });
    }
    Ok(report)
}

/// Whether the report the probe got is one `requirement` accepts.
///
/// Decided once, here: a host that cannot give what the settings ask for is an
/// environment problem the operator sees once at startup with the reason, rather
/// than one spawn per request ending in exit 125. The child keeps the same check
/// before it reads a request.
///
/// This is the *request path's* judgement; the page asks [`capability`], which
/// measures with the switch *on*. [`host_verdict`] is the page's half.
fn accept(report: Report, requirement: Requirement) -> Status {
    if let Some(why) = job_refusal(&report) {
        return Status::Unavailable(why);
    }
    match requirement.accepts(&report) {
        Ok(()) => Status::Ready(report),
        Err(refusal) => Status::Unavailable(format!(
            "the sandbox refused: {} — this host gives {}: {}",
            refusal.reason(),
            report.protections.summary(),
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
    probe: Option<&Path>,
) -> Option<Invocation> {
    let exe = EXECUTABLE
        .get()
        .cloned()
        .or_else(|| std::env::current_exe().ok())?;
    // Seatbelt matches the resolved path rather than the typed one, so a binary
    // behind a symlinked directory is resolved before it is granted and started.
    // Windows is left alone: `canonicalize` there returns a `\\?\` path.
    #[cfg(target_os = "macos")]
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);

    let mut args: Vec<OsString> = vec![
        OsString::from(SUBCOMMAND),
        OsString::from("--profile"),
        OsString::from(profile.as_str()),
    ];
    if !requirement.enabled {
        args.push(OsString::from("--sandbox-off"));
    }
    // Landlock and Seatbelt rules must be installed before the request is read, so
    // the grants travel on the command line.
    if let Some(helper) = grants.helper {
        args.push(OsString::from("--ffmpeg"));
        args.push(helper.as_os_str().to_os_string());
    }
    if let Some(source) = grants.source {
        args.push(OsString::from("--src"));
        args.push(source.as_os_str().to_os_string());
    }
    // The file the files layer must refuse; only the once-per-process probe has one.
    if let Some(probe) = probe {
        args.push(OsString::from("--probe-path"));
        args.push(probe.as_os_str().to_os_string());
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
/// Windows needs `CreateProcessAsUser` for a restricted token, which
/// `std::process::Command` cannot do; everywhere else the standard child is used.
enum Child {
    #[cfg(not(windows))]
    Standard(std::process::Child),
    #[cfg(windows)]
    Windows(sandbox::WindowsChild),
}

impl Child {
    /// The child's exit code, or `None` while it is still running. A death by
    /// signal is the negative signal number.
    fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        match self {
            #[cfg(not(windows))]
            Child::Standard(child) => {
                use std::os::unix::process::ExitStatusExt;
                Ok(child.try_wait()?.map(|status| match status.code() {
                    Some(code) => code,
                    // `-9` is the kernel, `-6` an abort; neither is a code the
                    // child itself produces.
                    None => -status.signal().unwrap_or(0),
                }))
            }
            #[cfg(windows)]
            Child::Windows(child) => child.try_wait(),
        }
    }

    /// Kill the child and whatever it started.
    ///
    /// The child leads its own process group ([`spawn_child`]), so the group is what
    /// gets the signal: killing the leader alone leaves a forked copy holding the
    /// pipes. Windows reaches the tree through the Job Object. The group reaches the
    /// whole tree on Linux only: a macOS copy can leave it.
    fn kill_tree(&mut self) {
        match self {
            #[cfg(not(windows))]
            Child::Standard(child) => {
                // `killpg` on the group the child leads. On Linux that is the whole
                // tree: the child's filter refuses `setsid`/`setpgid`, so a copy
                // cannot leave it. On macOS nothing refuses those calls, so a copy
                // that makes one leaves the group and this signal.`
                unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
                let _ = child.kill();
            }
            #[cfg(windows)]
            Child::Windows(child) => child.kill_tree(),
        }
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
fn spawn_child(
    invocation: &Invocation,
    selftest: bool,
    _start: Start,
    _profile: Profile,
    _grants: crate::sandbox::Grants<'_>,
) -> std::io::Result<Child> {
    let mut command = Command::new(&invocation.program);
    command.args(&invocation.args);
    if selftest {
        command.arg("--selftest");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The server's environment holds the master secret and every resolved path.
        .env_clear()
        .env("RUST_BACKTRACE", "0")
        // Nothing here is relative, and the server's own directory is not the
        // child's business.
        .current_dir(Path::new("/"));
    // The child leads its own group, so a timeout can end what it started. macOS's
    // media profile is where this matters: a helper that got code running can leave
    // copies holding the pipes. Parser profiles deny the fork.
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map(Child::Standard)
}

/// Start the child in its container, job and restricted token, or in the job and
/// token alone when the container is not wanted or was given up.
///
/// The environment, working directory and handle inheritance are set inside
/// `sandbox` on Windows, where they are part of the creation call.
#[cfg(windows)]
fn spawn_child(
    invocation: &Invocation,
    selftest: bool,
    start: Start,
    profile: Profile,
    grants: crate::sandbox::Grants<'_>,
) -> std::io::Result<Child> {
    let mut args = invocation.args.clone();
    if selftest {
        args.push(OsString::from("--selftest"));
    }
    match start {
        Start::Confined => sandbox::spawn(&invocation.program, &args, profile, grants),
        Start::Plain => sandbox::spawn_token_only(&invocation.program, &args, profile, grants),
        Start::Unrestricted => {
            sandbox::spawn_unrestricted(&invocation.program, &args, profile, grants)
        }
    }
    .map(Child::Windows)
}

struct Run {
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    timed_out: bool,
}

impl Run {
    /// One line saying why a run that should have reported did not: the child's
    /// stderr, which the parent logs at `debug` but the probe can print.
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
/// A code below zero is a signal on unix; on Windows it is an `NTSTATUS`, where
/// `0xC0000135` is the loader failing to find a library.
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
/// The three pipes are drained on their own threads, or a parser that logs while
/// the parent waits would fill a pipe and deadlock both sides.
fn run_child(
    invocation: &Invocation,
    request: Option<Vec<u8>>,
    timeout: Duration,
    start: Start,
    profile: Profile,
    grants: crate::sandbox::Grants<'_>,
) -> std::io::Result<Run> {
    let mut child = spawn_child(invocation, request.is_none(), start, profile, grants)?;
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

    // The group is ended before the pipes are drained, whichever way the child
    // stopped: a child that exits 0 while a copy holds the pipes would otherwise
    // leave that copy running past the request. An empty group answers `ESRCH`, and
    // a child already gone answers an error, neither of which is a failure here.
    child.kill_tree();

    // Bounded: the child being gone is not the pipes being closed. A writer that
    // never finishes is dropped the same way.
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
/// `None` means the thread still runs and its handles were dropped with it; the
/// caller's fail-closed answer is an empty `stdout` (no reply) or `stderr`.
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
/// Bytes rather than `read_to_string`: diagnostics are not required to be UTF-8.
/// Past the cap the bytes are still read and dropped, so the child never blocks.
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
    Reply {
        tag: u8,
        body: Vec<u8>,
    },
    /// The environment: the child would not run confined, or its loader failed.
    Unavailable(String),
    Failed(&'static str),
}

/// Decide what a finished child means.
///
/// A refusal or a runner failure is the *environment* and is checked before the
/// reply, because a child that never ran cannot have written one.
fn verdict(exit_code: Option<i32>, stdout: &[u8], stderr: &str) -> Verdict {
    // The exit code is what a runner passes through; the text is the belt for one
    // that rewrote it, read only when nothing came back on the reply stream and the
    // refusal is the child's first line. Otherwise document bytes echoed into a
    // diagnostic could mark the profile unavailable.
    let refusal_is_the_childs =
        stdout.is_empty() && stderr.trim_start().starts_with(SANDBOX_REFUSAL);
    if exit_code == Some(EXIT_SANDBOX_UNAVAILABLE) || refusal_is_the_childs {
        return Verdict::Unavailable(detail_or(stderr, "the sandbox refused to run"));
    }
    #[cfg(windows)]
    if exit_code == Some(STATUS_DLL_INIT_FAILED) {
        return Verdict::Unavailable(detail_or(stderr, "the sandbox worker could not initialize"));
    }
    // No platform stops a child at its own memory bound: Linux (RLIMIT_AS),
    // Windows (the job's committed-memory cap) and macOS all let the kernel do it. A
    // memory kill therefore arrives as an exit code, and the request is retried.
    for signature in runner_failure_signatures() {
        if stderr.to_ascii_lowercase().contains(signature) {
            return Verdict::Unavailable(detail_or(stderr, "the sandbox runner failed"));
        }
    }
    if !stderr.trim().is_empty() {
        // Clipped again: a log line carries less than the parent holds, and the head
        // is what says why.
        tracing::debug!("extract-worker: {}", clipped(stderr.trim(), 400));
    }

    match (exit_code, parse_frame(stdout)) {
        (Some(0), Some((tag, body))) => Verdict::Reply { tag, body },
        _ => Verdict::Failed(reason::FAILED),
    }
}

/// Signatures a platform runner prints when it refuses its own profile.
///
/// A runner that fails before starting the command is a sandbox the operator has to
/// fix, not a document that cannot be read.
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

/// The child's own words, clipped to what a log line carries. The reason is at the
/// head; the tail is what a document can fill.
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
/// The framing is the same for every job — tag, little-endian `u32` length, body —
/// and the tag says which job wrote it, so the caller can reject another's reply.
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

    struct Launched {
        program: String,
        args: Vec<String>,
    }

    impl Launched {
        /// The arguments the child itself is started with: whatever the platform
        /// runner needs and the path to the binary come in front of these.
        fn child(&self) -> Vec<&str> {
            let at = self
                .args
                .iter()
                .position(|arg| arg == SUBCOMMAND)
                .expect("the child is named on the command line");
            self.args[at..].iter().map(String::as_str).collect()
        }

        fn flags(&self) -> Vec<&str> {
            self.child()
                .into_iter()
                .filter(|arg| arg.starts_with("--"))
                .collect()
        }
    }

    fn launched(
        requirement: Requirement,
        profile: Profile,
        grants: sandbox::Grants<'_>,
        probe: Option<&Path>,
    ) -> Launched {
        let invocation = invocation(requirement, profile, grants, probe)
            .expect("this process has a path to itself");
        Launched {
            program: invocation.program.to_string_lossy().into_owned(),
            args: invocation
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// The platform switch reaches the one decision it is about.
    ///
    /// Windows-only in outcome: a launch asks for the AppContainer first, and a host
    /// that refused it is not asked again, so a launch inside a host sandbox gets
    /// its bound and token instead of dying in a loader on every request. A save
    /// clears the record, so it is one more attempt, not a verdict.
    ///
    /// Compiled and run everywhere on purpose: only the *outcome* is Windows -
    /// specific, and that part is asserted in the probe test under `#[cfg(windows)]`.
    #[test]
    fn only_a_host_that_refused_the_container_is_spared_a_second_attempt() {
        assert!(
            container_wanted(),
            "a launch asks for the container until the host refuses it"
        );

        // One refusal is this process's answer for every launch that follows,
        // until a settings save asks again.
        give_up_container();
        assert!(
            !container_wanted(),
            "a container the host refused must not be asked for again"
        );

        configure_requirement(Requirement::new(true));
        assert!(
            container_wanted(),
            "a settings save is worth one more attempt"
        );

        // The token record takes the same shape. Without it a host that cannot start
        // a child under a restricted token has nowhere left to go.
        give_up_token();
        assert!(
            TOKEN_GIVEN_UP.load(std::sync::atomic::Ordering::Relaxed),
            "a refused token has to be remembered, or every request retries it"
        );
        configure_requirement(Requirement::new(true));
        assert!(
            !TOKEN_GIVEN_UP.load(std::sync::atomic::Ordering::Relaxed),
            "a settings save is worth one more attempt"
        );

        // The walk itself ends: the last creation has nothing below it, which is
        // what makes the loop terminate rather than retry forever.
        #[cfg(windows)]
        assert_eq!(Start::Unrestricted.weaker(), None);
        #[cfg(windows)]
        assert_eq!(Start::Plain.weaker(), Some(Start::Unrestricted));
        #[cfg(windows)]
        assert_eq!(Start::Confined.weaker(), Some(Start::Plain));
        #[cfg(not(windows))]
        assert_eq!(Start::Confined.weaker(), None);
    }

    /// The child starts with the policy the parent resolved and nothing else: a flag
    /// leaked in would change what it installs, and a missing one would silently
    /// remove a grant.
    #[test]
    fn the_child_is_started_with_the_policy_the_parent_resolved() {
        let launched = launched(
            Requirement::new(true),
            Profile::Documents,
            sandbox::Grants::default(),
            None,
        );
        let child = launched.child();
        assert_eq!(child[0], SUBCOMMAND);
        assert_eq!(&child[1..3], ["--profile", "documents"]);
        let flags = launched.flags();
        assert!(flags.contains(&"--profile"));
        for absent in [
            "--ffmpeg",
            "--src",
            "--probe-path",
            "--sandbox-off",
            "--min-level",
            "--windows-container",
        ] {
            assert!(
                !flags.contains(&absent),
                "{absent} names nothing here: {flags:?}"
            );
        }
    }

    /// The child judges its own report by the same rule the parent used.
    #[test]
    fn the_switch_travels_to_the_child() {
        let plain = launched(
            Requirement::new(false),
            Profile::Documents,
            sandbox::Grants::default(),
            None,
        );
        let child = plain.child();
        assert_eq!(&child[1..3], ["--profile", "documents"]);
        let flags = plain.flags();
        assert!(
            flags.contains(&"--sandbox-off"),
            "the child has to know the switch is off: {flags:?}"
        );
        assert!(
            !flags.contains(&"--windows-container"),
            "the container is a platform decision now, not a flag: {flags:?}"
        );
    }

    /// The paths the child may reach, installed before the request is read. The probe
    /// file is how the files layer is measured at all.
    #[test]
    fn the_grants_are_named_once_and_only_when_they_exist() {
        let helper = Path::new("/opt/nanofile/ffmpeg");
        let source = Path::new("/var/tmp/nanofile/scratch.bin");
        let probe = Path::new("/var/tmp/nanofile/probe");
        let launched = launched(
            Requirement::new(true),
            Profile::Media,
            sandbox::Grants {
                helper: Some(helper),
                source: Some(source),
            },
            Some(probe),
        );

        let child = launched.child();
        for (flag, value) in [
            ("--ffmpeg", helper),
            ("--src", source),
            ("--probe-path", probe),
        ] {
            assert_eq!(
                child.iter().filter(|arg| **arg == flag).count(),
                1,
                "{flag} must be named exactly once: {child:?}"
            );
            let at = child
                .iter()
                .position(|arg| *arg == flag)
                .expect("just counted");
            assert_eq!(Path::new(child[at + 1]), value, "{flag} names its value");
        }
    }

    /// The platform runner is the parent's wrapper, not the child's own protection:
    /// a child that thought otherwise would refuse for lacking the layers.
    #[test]
    fn only_the_platform_with_a_runner_wraps_the_child() {
        let launched = launched(
            Requirement::new(true),
            Profile::Documents,
            sandbox::Grants::default(),
            None,
        );

        #[cfg(target_os = "macos")]
        {
            assert_eq!(launched.program, "/usr/bin/sandbox-exec");
            assert_eq!(launched.args[0], "-p");
            assert_eq!(
                launched.args[2], "--",
                "the runner needs its own separator before the child"
            );
            assert!(
                launched.flags().contains(&"--seatbelt"),
                "the child has to know its confinement came from the runner: {:?}",
                launched.flags()
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_ne!(
                launched.program, "/usr/bin/sandbox-exec",
                "only macOS needs a runner"
            );
            assert!(
                !launched.flags().contains(&"--seatbelt"),
                "no runner is applied here, so the child is not told one was: {:?}",
                launched.flags()
            );
        }
    }

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
            job: None,
            detail: "landlock_abi=6,seccomp=on".to_string(),
        });
        let report = Report::parse(&line).expect("the parent parses it");
        assert!(report.protections.complete());
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

    /// The text clause is the belt for a runner that rewrote the exit code, read only
    /// when nothing came back on the reply stream and the refusal is the first line.
    #[test]
    fn a_document_cannot_write_the_refusal_the_parent_looks_for() {
        assert!(matches!(
            verdict(
                Some(101),
                b"",
                "parse failed near extract-worker: sandbox unavailable",
            ),
            Verdict::Failed(reason::FAILED)
        ));
        // A child that answered is not overridden by its stderr.
        assert!(matches!(
            verdict(
                Some(0),
                b"U\x05\x00\x00\x00hello",
                "extract-worker: sandbox unavailable",
            ),
            Verdict::Reply { tag: b'U', .. }
        ));
        // With no exit code to read, the child's own first line is still the
        // environment.
        assert!(matches!(
            verdict(None, b"", "extract-worker: sandbox unavailable: partial"),
            Verdict::Unavailable(_)
        ));
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

    /// Every packaged fixture still yields its whole text; the confinement around it
    /// is what the probe reports.
    #[test]
    fn the_self_test_parses_the_packaged_documents() {
        configure_parser_limits();
        assert_eq!(
            probe_parse(Profile::Documents, crate::sandbox::Grants::default()),
            ("pdf-ok,docx-ok,xlsx-ok,pptx-ok".to_string(), JobVerdict::Ok)
        );
    }

    /// A media profile that cannot decode is not spawned for; no helper at all is a
    /// state, not a failure; and a document's verdict never gates.
    #[test]
    fn a_media_job_that_failed_is_not_ready() {
        let media = |job: Option<JobVerdict>, detail: &str| Report {
            profile: Profile::Media,
            protections: sandbox::Protections {
                limits: true,
                files: true,
                network: true,
                process: false,
            },
            job,
            detail: detail.to_string(),
        };

        for detail in [
            "parse=media-failed(exit_status:_1)",
            "parse=media-unavailable(Access_is_denied)",
        ] {
            assert!(
                matches!(
                    accept(
                        media(Some(JobVerdict::Failed), detail),
                        Requirement::new(true)
                    ),
                    Status::Unavailable(_)
                ),
                "{detail} must not be ready"
            );
        }

        // No helper configured: the page says so, and requests have nothing to
        // spawn rather than a broken helper to spawn.
        assert!(matches!(
            accept(
                media(Some(JobVerdict::NoHelper), "parse=media-no-helper"),
                Requirement::new(true),
            ),
            Status::Ready(_)
        ));
        assert!(matches!(
            accept(
                media(Some(JobVerdict::Ok), "parse=media-ok(2333)"),
                Requirement::new(true),
            ),
            Status::Ready(_)
        ));

        let mut documents = media(Some(JobVerdict::Failed), "parse=pdf-failed,docx-ok");
        documents.profile = Profile::Documents;
        assert!(
            matches!(accept(documents, Requirement::new(true)), Status::Ready(_)),
            "a document's verdict is recorded per file, not by refusing the profile"
        );
    }

    #[test]
    fn an_unreadable_report_line_is_not_a_status() {
        assert!(Report::parse("NFX1-sandbox limits=on").is_none());
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

    /// Abandoned at its deadline rather than joined, so a process holding the pipes
    /// cannot hold the extraction permit with them.
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

    /// A saved helper replaces the startup one, and the answers measured against
    /// the old one are dropped with it.
    #[test]
    fn the_helper_and_the_cached_answers_move_together() {
        configure_helper(PathBuf::from("/nonexistent/first-ffmpeg"));
        assert_eq!(helper(), Some(Path::new("/nonexistent/first-ffmpeg")));

        configure_helper(PathBuf::from("/nonexistent/second-ffmpeg"));
        assert_eq!(
            helper(),
            Some(Path::new("/nonexistent/second-ffmpeg")),
            "a saved path must replace the one the process started with"
        );

        // Seed both caches, then drop them the way a settings change does.
        let status = STATUS.get_or_init(|| Mutex::new(HashMap::new()));
        status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(Profile::Images)
            .or_default()
            .retry_at = Some(Instant::now() + Duration::from_secs(60));
        let capability = CAPABILITY.get_or_init(|| Mutex::new(HashMap::new()));
        capability
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                Profile::Images,
                Measured::Failed {
                    why: "seeded".to_string(),
                    retry_at: Instant::now() + Duration::from_secs(60),
                },
            );

        invalidate();
        assert!(
            status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "a probe measured against the old helper must not stand"
        );
        assert!(
            capability
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "the Sandbox page must re-measure after the helper changes"
        );
    }

    /// A settings save with no helper configured names the empty path, which is
    /// not a grant: installing it would refuse every media rule that follows.
    #[test]
    fn an_empty_helper_is_not_installed() {
        configure_helper(PathBuf::new());
        assert_ne!(
            helper().map(|path| path.as_os_str().is_empty()),
            Some(true),
            "the empty path must not become the media grant"
        );
    }

    /// The probe file the files layer has to refuse is the parent's, and it is
    /// the one candidate that measures the same thing on every platform.
    #[test]
    fn the_deny_probe_is_private_and_removed() {
        let probe = DenyProbe::create().expect("the temporary directory takes a file");
        let path = probe.path().to_path_buf();
        assert!(path.exists(), "the child is given a file that is there");
        assert!(
            path.starts_with(std::env::temp_dir()),
            "the probe lives where no profile grants: {}",
            path.display()
        );

        // A second probe is a second path, and the first is still there: a
        // predictable name is a name another local user could leave something at.
        let second = DenyProbe::create().expect("a second file");
        let second_path = second.path().to_path_buf();
        assert_ne!(second_path, path);
        assert!(path.exists());

        drop(second);
        assert!(
            !second_path.exists(),
            "a dropped probe takes its file with it"
        );
        drop(probe);
        assert!(
            !path.exists(),
            "the file must not outlive the probe that named it on the child's command line"
        );
    }

    /// A bare command name resolves to a real, absolute path, so the media grant can
    /// name a file: on Windows `cmd` → `…\cmd.exe`, with the extension appended.
    #[test]
    fn a_bare_command_name_resolves_to_a_path_that_exists() {
        // A name every host this ships on has, and one whose executable carries
        // an extension on Windows (`cmd.exe`) but not on unix (`sh`).
        #[cfg(windows)]
        let resolved = resolve_helper("cmd");
        #[cfg(not(windows))]
        let resolved = resolve_helper("sh");
        assert!(
            resolved.is_absolute() && resolved.is_file(),
            "a bare command name must resolve to a real, absolute path: {}",
            resolved.display()
        );
    }

    fn finished(exit_code: Option<i32>, stdout: &[u8], stderr: &str, timed_out: bool) -> Run {
        Run {
            exit_code,
            stdout: stdout.to_vec(),
            stderr: stderr.to_string(),
            timed_out,
        }
    }

    /// The opt-out is given up for a container the loader could not start in, and
    /// nothing else: a timeout says nothing about the flavour, and a child that
    /// refused on its own report ran inside it.
    #[test]
    fn only_a_child_that_never_ran_makes_the_opt_out_the_suspect() {
        // Created, died in its loader, said nothing: the shape the attribute leaves
        // behind. Spelled out because the test runs everywhere.
        assert!(opt_out_suspect(&finished(
            Some(0xC000_0142u32 as i32),
            b"",
            "",
            false
        )));
        // The timeout the probe is allowed: no evidence about the container.
        assert!(!opt_out_suspect(&finished(None, b"", "", true)));
        // The child's own refusal: it ran and reported on itself.
        assert!(!opt_out_suspect(&finished(
            Some(EXIT_SANDBOX_UNAVAILABLE),
            b"",
            &format!("{SANDBOX_REFUSAL}: this host cannot bound a child"),
            false,
        )));
        // Lines the parent could not read: the child still ran.
        assert!(!opt_out_suspect(&finished(
            Some(0),
            b"not a report",
            "",
            false
        )));
        // A child that reported and then died: nothing to re-ask.
        assert!(!opt_out_suspect(&finished(
            Some(1),
            b"NFS2-sandbox profile=media limits=on files=denied \
              network=denied process=open detail=x",
            "",
            false,
        )));
    }

    /// The opt-out is given up only for the media profile's own helper failure: a
    /// missing fact would turn the retry into a loop or a wrong answer.
    #[test]
    fn only_a_failed_media_helper_selects_the_plain_container() {
        assert!(media_needs_the_plain_container(
            Profile::Media,
            true,
            false,
            Some(JobVerdict::Failed)
        ));
        // No opt-out applied: there is nothing to give up.
        assert!(!media_needs_the_plain_container(
            Profile::Media,
            false,
            false,
            Some(JobVerdict::Failed)
        ));
        // Already plain: measuring again would measure the same launch.
        assert!(!media_needs_the_plain_container(
            Profile::Media,
            true,
            true,
            Some(JobVerdict::Failed)
        ));
        // A profile that runs no program has no helper to fail.
        for profile in [Profile::Documents, Profile::Images] {
            assert!(!media_needs_the_plain_container(
                profile,
                true,
                false,
                Some(JobVerdict::Failed)
            ));
        }
        // "No helper configured" is a state the operator chose, not a fault.
        for job in [
            None,
            Some(JobVerdict::Ok),
            Some(JobVerdict::NoHelper),
            Some(JobVerdict::Nothing),
            Some(JobVerdict::Empty),
            Some(JobVerdict::Different),
            Some(JobVerdict::Unsupported),
        ] {
            assert!(
                !media_needs_the_plain_container(Profile::Media, true, false, job),
                "{job:?} is not a helper that failed to run"
            );
        }
    }
}
