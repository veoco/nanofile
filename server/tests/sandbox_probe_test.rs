//! What each profile's startup report says on this host.
//!
//! The typed half comes from `Report`, the mechanism tokens from the profile
//! that installed them, and this is the one place either is asserted — on the
//! path the server walks at startup. No fixture is built, so the worker's
//! process-global executable is left alone.

mod common;

#[cfg(windows)]
use std::time::{Duration, Instant};

use common::sandbox::{fact, has, helper, helper_runs, probe, without_ffmpeg};
use server::sandbox::{JobVerdict, Level, Profile, Report};

/// A fact that has to be there, or a panic carrying the whole line.
fn value<'a>(report: &'a Report, key: &str) -> &'a str {
    fact(report, key).unwrap_or_else(|| panic!("no {key} in: {}", report.detail))
}

/// A bare token of the fact list, matched by its prefix.
///
/// The resource clamps are one fact carrying a list of its own
/// (`limits=as…,cpu…,nofile…,fsize…m,core0`), so they are looked up as tokens
/// rather than as `key=value`.
fn bare<'a>(report: &'a Report, prefix: &str) -> Option<&'a str> {
    report
        .detail
        .split(',')
        .find(|entry| entry.starts_with(prefix))
}

/// Whether a value is digits.
fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

/// What every profile on every platform has to say.
fn the_common_answer(profile: Profile, report: &Report, line: &str) {
    assert_eq!(report.profile, profile, "{line}");
    // The grade needs both: a host that denies the network and multiplies
    // processes but lets a parser read every path is not confined at all.
    assert!(report.protections.limits, "{line}");
    assert!(report.protections.files, "{line}");
    // Thread creation is what the fork was said to be needed for, so this is the
    // assertion that decides it: a profile that broke the parsers reports it.
    assert!(has(report, "threads=ok"), "{line}");
    assert!(has(report, "measure=thorough"), "{line}");
    // The child sets the Office text budget for itself; the parent never parses
    // and so cannot set it for it.
    assert!(line.contains("text_chars=8388608"), "{line}");
}

/// The documents profile parses every structured format it carries, whole.
#[test]
fn the_documents_report_shows_a_worker_that_still_reads() {
    let (report, line) = probe(Profile::Documents, None);

    the_common_answer(Profile::Documents, &report, &line);
    assert_eq!(report.level(), Level::Full, "{line}");
    assert!(report.protections.network, "{line}");
    // The verdict is the whole text of each document, not a word of it: a parser
    // that stopped early still contains the words the fixtures are made of.
    assert_eq!(
        report.job,
        Some(JobVerdict::Ok),
        "the confined worker must still read whole documents: {line}"
    );
    #[cfg(unix)]
    assert!(
        report.protections.process,
        "the parser profiles deny the fork: {line}"
    );
    the_platform_answer(Profile::Documents, &report);
}

/// The images profile decodes and encodes, which the documents profile says
/// nothing about: it is the other child every thumbnail and avatar runs as.
#[test]
fn the_images_report_shows_a_worker_that_still_decodes() {
    let (report, line) = probe(Profile::Images, None);

    the_common_answer(Profile::Images, &report, &line);
    assert_eq!(report.level(), Level::Full, "{line}");
    assert!(report.protections.network, "{line}");
    // The reply is checked against the pixels it was given, so this is a decode
    // and a resize rather than an echo.
    assert_eq!(
        report.job,
        Some(JobVerdict::Ok),
        "the confined worker must still decode an image: {line}"
    );
    #[cfg(unix)]
    assert!(
        report.protections.process,
        "the image profile denies the fork: {line}"
    );
    the_platform_answer(Profile::Images, &report);
}

/// The media profile starts the helper it is granted, and the helper decodes.
#[test]
fn the_media_report_shows_a_helper_that_still_decodes() {
    if !helper_runs() {
        without_ffmpeg();
        return;
    }

    #[cfg(windows)]
    let started = Instant::now();
    let (report, line) = probe(Profile::Media, Some(helper()));

    the_common_answer(Profile::Media, &report, &line);
    assert_eq!(report.level(), Level::Partial, "{line}");
    // It cannot carry the process item: it starts a program by definition, and
    // every way of starting one copies the process first.
    assert!(
        !report.protections.process,
        "the media profile must not claim the process item: {line}"
    );
    assert!(has(&report, "helper=allowed"), "{line}");
    // The helper reads the file the parent handed it and answers with an encoded
    // frame — not with its version, which says nothing about the work.
    assert_eq!(
        report.job,
        Some(JobVerdict::Ok),
        "the confined helper must decode a frame: {line}"
    );
    #[cfg(windows)]
    {
        // A grant written on a directory re-imposes inheritance on everything
        // under it, which takes minutes rather than seconds.
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(60),
            "the media probe took {elapsed:?}"
        );
    }
    the_platform_answer(Profile::Media, &report);
}

/// The layers Linux installs, and what they bound.
#[cfg(target_os = "linux")]
fn the_platform_answer(profile: Profile, report: &Report) {
    assert!(has(report, "no_new_privs=on"), "{}", report.detail);
    assert!(has(report, "dumpable=off"), "{}", report.detail);
    // A server started as root must not hand root to a parser.
    assert!(has(report, "caps=empty"), "{}", report.detail);
    assert!(has(report, "pdeathsig=on"), "{}", report.detail);
    // The calls Landlock cannot govern are the filter's job, and a kernel
    // without seccomp is the one case where they are not covered.
    assert!(has(report, "ll_gaps=closed"), "{}", report.detail);
    assert!(has(report, "files=measured-denied"), "{}", report.detail);
    // The two groups Landlock only has from a given ABI, so which one this
    // kernel took is a fact about the host rather than a claim.
    for key in ["mdwe", "landlock_net", "landlock_scoped"] {
        assert!(
            matches!(value(report, key), "on" | "off"),
            "{key}: {}",
            report.detail
        );
    }
    for key in ["seccomp", "landlock_abi"] {
        assert!(is_number(value(report, key)), "{key}: {}", report.detail);
    }
    // Five clamps in one fact: the address space, the CPU pair, the descriptor
    // count, the file-size cap and the core dump. The values are policy; that
    // each clamp is there is what the report claims.
    for clamp in ["limits=as", "cpu", "nofile", "fsize", "core"] {
        let token =
            bare(report, clamp).unwrap_or_else(|| panic!("no {clamp} clamp in: {}", report.detail));
        assert!(
            token[clamp.len()..].starts_with(|c: char| c.is_ascii_digit()),
            "{token} carries no number"
        );
    }
    match profile {
        Profile::Documents | Profile::Images => {
            assert!(has(report, "fork=denied"), "{}", report.detail);
            // The exec probe needs a fork to try `execve` in, and the fork is
            // refused, so the item stands unmeasured rather than unclaimed.
            assert!(has(report, "exec=unmeasured"), "{}", report.detail);
        }
        Profile::Media => {
            assert!(has(report, "fork=open"), "{}", report.detail);
            // Starting a program other than the helper is what stays refused:
            // the grant names files, never a directory.
            assert!(has(report, "exec=denied"), "{}", report.detail);
            assert!(has(report, "metadata=open"), "{}", report.detail);
            assert!(has(report, "helper_scope=libs"), "{}", report.detail);
            // Nothing bounds how many copies of the helper there may be, and the
            // report says so instead of claiming the item it cannot have.
            assert!(has(report, "media_process=unbounded"), "{}", report.detail);
            let grants = value(report, "helper_grants");
            assert!(
                grants.parse::<usize>().is_ok_and(|count| count > 0),
                "helper_grants={grants}"
            );
        }
    }
}

/// The profile the parent's runner applies, and the residuals it leaves.
#[cfg(target_os = "macos")]
fn the_platform_answer(profile: Profile, report: &Report) {
    // Without this token a Seatbelt profile that silently did nothing would be
    // measured as if the child had confined itself.
    assert!(has(report, "runner=external"), "{}", report.detail);
    // The worker is dynamically linked, so its profile has to grant the system
    // trees the loader reads: reported as the residual it is, never as a missing
    // protection.
    assert!(
        matches!(value(report, "system"), "readable" | "denied" | "absent"),
        "{}",
        report.detail
    );
    match profile {
        Profile::Documents => {
            assert!(has(report, "fork=denied"), "{}", report.detail);
            assert!(
                matches!(value(report, "exec"), "denied" | "unmeasured"),
                "{}",
                report.detail
            );
            // The fork is refused, so the exec probe has no copy to run in: the
            // item is measured as far as this platform can measure it.
            assert!(
                matches!(value(report, "process"), "unmeasured" | "measured-denied"),
                "{}",
                report.detail
            );
        }
        Profile::Images => {}
        Profile::Media => {
            assert!(has(report, "fork=open"), "{}", report.detail);
            assert!(has(report, "exec=denied"), "{}", report.detail);
            // The same exec attempted by hand in that child.
            assert!(has(report, "hand=ok"), "{}", report.detail);
            // `trees` when the helper lives in a package-manager tree (the
            // libraries it links against are then readable), `libs` when not.
            assert!(
                matches!(value(report, "helper_scope"), "trees" | "libs"),
                "{}",
                report.detail
            );
            assert!(has(report, "media_process=unbounded"), "{}", report.detail);
        }
    }
}

/// The creation the parent settled on, and the token the child read back.
#[cfg(windows)]
fn the_platform_answer(profile: Profile, report: &Report) {
    // The ladder tries the strongest creation first and reports the one it
    // settled for, so the rung and the token beside it are one answer: a
    // container with the restricted token and one without are two different
    // creations, and the grade above is what says a container was reached at
    // all — a host that fell to the token alone could not have `files` denied.
    let rung = value(report, "rung");
    assert!(
        matches!(rung, "container" | "container-only" | "token" | "plain"),
        "{rung}: {}",
        report.detail
    );
    let token = value(report, "token");
    match rung {
        "container" => assert_eq!(token, "restricted", "{}", report.detail),
        "container-only" => assert_eq!(token, "unrestricted", "{}", report.detail),
        _ => {}
    }
    assert!(has(report, "container=appcontainer"), "{}", report.detail);
    // The job is the parent's, named at creation, and the child reads its limits
    // back from it: the value is the memory bound, `,parent` is the mark of that
    // arrangement, and `limits-missing` is what a job without them would report.
    let job = value(report, "job");
    assert!(job.starts_with("memory"), "job={job}");
    assert!(has(report, "parent"), "{}", report.detail);
    // Which creation was asked for and got: `off` is what Windows 10 answers,
    // where the attribute does not exist.
    assert!(
        matches!(value(report, "lpac"), "on" | "off"),
        "{}",
        report.detail
    );
    // Where a write from the container lands: `store` means it resolved into the
    // container's own redirected store, `user` means it did not.
    assert!(
        matches!(
            value(report, "writes"),
            "store" | "user" | "denied" | "unmeasured"
        ),
        "{}",
        report.detail
    );
    // The integrity level the container sets, asserted as a shape: a label this
    // side sets is one the token-only fallback cannot survive.
    assert!(
        matches!(value(report, "il"), "untrusted" | "low" | "medium"),
        "{}",
        report.detail
    );
    // The far edge of the files layer: the system tree stays readable through
    // the packages a low-box process is checked against.
    assert!(
        matches!(value(report, "system"), "readable" | "denied" | "absent"),
        "{}",
        report.detail
    );
    // The count moves with the Windows build; zero would mean the API is not
    // working at all.
    let mitigations = value(report, "mitigations");
    assert!(
        mitigations.parse::<u32>().is_ok_and(|count| count > 0),
        "mitigations={mitigations}"
    );
    // The near edge: the parent's own file carries no ACE for the package the
    // container is checked against, and unlike a write a read cannot be
    // redirected into the container's store.
    assert!(has(report, "files=measured-denied"), "{}", report.detail);
    if profile.runs_helper() {
        assert!(has(report, "helper_scope=libs"), "{}", report.detail);
    }
}
