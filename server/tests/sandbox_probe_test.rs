//! What each profile's startup report says on this host.
//!
//! The typed half comes from `Report`, the mechanism tokens from the profile
//! that installed them, and this is the one place either is asserted — on the
//! path the server walks at startup. No fixture is built, so the worker's
//! process-global executable is left alone.
//!
//! The probe measures under the master switch alone
//! (`common::sandbox::configure_probe_requirement`): these assertions are about
//! the mechanisms a host installs, not about a grade some setting asked for. On
//! Linux and macOS the platform installs everything; on Windows the launch asks
//! for its container and reports what came back.
//!
//! It is also where `skipped0`/`skipped1` are read: on Windows each walk step
//! that was tried and did not report leaves its reason, so a host that fell back
//! cannot be read as a host that never tried. `skipped1` only exists for a walk
//! that started at the container, and `skipped0` is the first step that was
//! passed over — `Start::Plain` where the container was not attempted or was
//! already given up.

mod common;

#[cfg(windows)]
use std::time::{Duration, Instant};

use common::sandbox::{
    configure_media_probe_requirement, configure_probe_requirement, fact, has, helper, helper_runs,
    probe, with_requirement, without_ffmpeg,
};
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
#[cfg(target_os = "linux")]
fn bare<'a>(report: &'a Report, prefix: &str) -> Option<&'a str> {
    report
        .detail
        .split(',')
        .find(|entry| entry.starts_with(prefix))
}

/// Whether a value is digits.
#[cfg(target_os = "linux")]
fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

/// What every profile on every platform has to say.
///
/// The resource bound is the one item with no alternative: a profile that could
/// not establish it reports `limits=off`, and this is where that is caught
/// wherever it happens. The other items are asserted on the platform that
/// installs them.
fn the_common_answer(profile: Profile, report: &Report, line: &str) {
    assert_eq!(report.profile, profile, "{line}");
    assert!(report.protections.limits, "{line}");
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
    let (report, line) = with_requirement(configure_probe_requirement(), || {
        probe(Profile::Documents, None)
    });

    the_common_answer(Profile::Documents, &report, &line);
    // The work is the assertion that does not move: the child read every document
    // whole, whatever creation this host could give it.
    assert_eq!(
        report.job,
        Some(JobVerdict::Ok),
        "the bounded worker must still read whole documents: {line}"
    );
    the_grade_answer(&report);
    #[cfg(unix)]
    {
        assert!(report.protections.network, "{line}");
        assert!(
            report.protections.process,
            "the parser profiles deny the fork: {line}"
        );
    }
    the_platform_answer(Profile::Documents, &report);
}

/// The images profile decodes and encodes, which the documents profile says
/// nothing about: it is the other child every thumbnail and avatar runs as.
#[test]
fn the_images_report_shows_a_worker_that_still_decodes() {
    let (report, line) = with_requirement(configure_probe_requirement(), || {
        probe(Profile::Images, None)
    });

    the_common_answer(Profile::Images, &report, &line);
    // The reply is checked against the pixels it was given, so this is a decode
    // and a resize rather than an echo.
    assert_eq!(
        report.job,
        Some(JobVerdict::Ok),
        "the bounded worker must still decode an image: {line}"
    );
    the_grade_answer(&report);
    #[cfg(unix)]
    {
        assert!(report.protections.network, "{line}");
        assert!(
            report.protections.process,
            "the image profile denies the fork: {line}"
        );
    }
    the_platform_answer(Profile::Images, &report);
}

/// The media profile starts the helper it is granted, and the helper decodes.
#[test]
fn the_media_report_shows_a_helper_that_still_decodes() {
    if !helper_runs() {
        without_ffmpeg();
        return;
    }

    // The media profile needs its own requirement: it can never reach what the
    // parser profiles grade, so the shipped minimum would refuse it and skip the
    // assertion that matters.
    let helper = helper();
    #[cfg(windows)]
    let started = Instant::now();
    let (report, line) = with_requirement(configure_media_probe_requirement(), || {
        probe(Profile::Media, Some(helper))
    });

    the_common_answer(Profile::Media, &report, &line);
    the_grade_answer(&report);
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

/// The shape a host that refuses the container produces: the bound and the token.
///
/// This is the case the whole redesign is about. The container is the only
/// mechanism that can add the files layer on that platform, creating one needs a
/// registry profile and package access on the worker's own image, and a host
/// sandbox that confines every command it runs refuses exactly that. The feature
/// runs anyway — on the Job Object alone, which is what keeps the server up —
/// and the report says which layer is missing rather than the feature being
/// disabled over it.
///
/// Which of the two shapes a runner produces is the runner's answer, so this
/// asserts the one that is not the container's when that is what came back, and
/// leaves the container's own assertions to the other cases.
#[cfg(windows)]
#[test]
fn the_documents_profile_reports_the_bound_without_a_container() {
    let (report, line) = with_requirement(common::sandbox::configure_probe_requirement(), || {
        probe(Profile::Documents, None)
    });

    the_common_answer(Profile::Documents, &report, &line);
    assert!(report.protections.limits, "{line}");
    if has(&report, "container=appcontainer") {
        // This host gave a container. Every launch asks for one, so this is the
        // shape a runner that *can* create it produces, and the platform answer
        // is where it is checked.
        the_platform_answer(Profile::Documents, &report);
        return;
    }

    assert_eq!(report.level(), Level::Limits, "{line}");
    assert!(
        !report.protections.files && !report.protections.network,
        "the container was refused, so neither of those can be there: {line}"
    );
    // Which creation below the container the host settled for is the host's
    // business — `plain` where a restrained child starts, `unrestricted` where
    // even that dies in the loader — and both are reported rather than assumed.
    assert!(
        matches!(value(&report, "rung"), "plain" | "unrestricted"),
        "{line}"
    );
    // The bound is still the parent's job object, and the child reads it back.
    assert!(value(&report, "job").starts_with("memory"), "{line}");
    // The job the profile exists to do still happens: a bound is not a refusal.
    assert_eq!(
        report.job,
        Some(JobVerdict::Ok),
        "the bounded worker must still read documents: {line}"
    );
    the_platform_answer(Profile::Documents, &report);
}

/// What grade the profile graded, and why that is the right one here.
///
/// The two platforms that install the files layer grade `full` (media `partial`,
/// which cannot carry the process item), and that is asserted as it always was.
/// A Windows host that can start neither a container nor a restrained child gives
/// the resource bound alone, and saying so is the point of this branch: the
/// feature runs, its bound is in place, and the grade is the honest one.
fn the_grade_answer(report: &Report) {
    #[cfg(not(windows))]
    {
        let expected = if report.profile.runs_helper() {
            Level::Partial
        } else {
            Level::Full
        };
        assert_eq!(report.level(), expected, "{}", report.detail);
    }
    #[cfg(windows)]
    {
        assert!(report.level() >= Level::Limits, "{}", report.detail);
        // The two go together: the files layer is the container's, and a report
        // that claims it without one would have the page advertise a protection
        // this host does not have.
        assert_eq!(
            report.protections.files,
            has(report, "container=appcontainer"),
            "{}",
            report.detail
        );
    }
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

/// The creation the parent settled on, and what it bought.
///
/// Two shapes reach this file on Windows, and which one is the host's answer:
/// the container where one could be created, and the bound under the job alone
/// where it could not — which is what a host sandbox that confines every command
/// it runs produces, and what this repository's CI runner gives. Both shapes have
/// to carry the job object, the restricted token or the tokenless creation it
/// fell back to, and the mitigations; only the container adds the files and
/// network items.
#[cfg(windows)]
fn the_platform_answer(profile: Profile, report: &Report) {
    let rung = value(report, "rung");
    assert!(
        matches!(rung, "container" | "plain" | "unrestricted"),
        "{rung}: {}",
        report.detail
    );

    // The job is the parent's, named at creation, and the child reads its limits
    // back from it: the value is the memory bound, `,parent` is the mark of that
    // arrangement, and `limits-missing` is what a job without them would report.
    // This is the item no creation may be without.
    let job = value(report, "job");
    assert!(job.starts_with("memory"), "job={job}");
    assert!(has(report, "parent"), "{}", report.detail);

    // The count moves with the Windows build; zero would mean the API is not
    // working at all.
    let mitigations = value(report, "mitigations");
    assert!(
        mitigations.parse::<u32>().is_ok_and(|count| count > 0),
        "mitigations={mitigations}"
    );

    // The one fact every launch has to report: which token it got, and what the
    // container did or did not add.
    let token = value(report, "token");
    assert!(
        matches!(token, "restricted" | "unrestricted"),
        "token={token}: {}",
        report.detail
    );
    // The rung and the token are one answer: a container is what makes a low-box
    // token, and the walk only reaches the tokenless creation by giving the token
    // up.
    match rung {
        "container" => assert_eq!(token, "restricted", "{}", report.detail),
        "plain" => assert_eq!(token, "restricted", "{}", report.detail),
        "unrestricted" => assert_eq!(token, "unrestricted", "{}", report.detail),
        other => panic!("unknown creation {other}: {}", report.detail),
    }
    assert!(
        matches!(value(report, "lpac"), "on" | "off"),
        "{}",
        report.detail
    );
    // The integrity level is a property of the token, not a second opinion about
    // the rung: a launch that kept this process's own token runs at the level the
    // server runs at, and only a token the container or the restriction lowered
    // grades below it. `high` is therefore the honest value for the tokenless
    // creation, and it is checked against the token rather than listed beside it.
    let il = value(report, "il");
    match token {
        "unrestricted" => assert!(
            matches!(il, "medium" | "high"),
            "il={il} with this process's own token: {}",
            report.detail
        ),
        _ => assert!(
            matches!(il, "untrusted" | "low" | "medium"),
            "il={il} with a restricted token: {}",
            report.detail
        ),
    }

    // Which creations were passed over, in the order the walk tried them. Nothing
    // below the container can be reached without the container having been given
    // up first, so a second reason without a first is a broken record.
    let skipped0 = fact(report, "skipped0").unwrap_or_default();
    let skipped1 = fact(report, "skipped1").unwrap_or_default();
    assert!(
        skipped1.is_empty() || !skipped0.is_empty(),
        "a walk records its second refusal only after its first: {}",
        report.detail
    );

    match rung {
        "container" => {
            // The container answered, so nothing was passed over to reach it.
            // The refusals of an earlier launch are not carried into this
            // report: they belong to the walk that produced them.
            assert!(skipped0.is_empty(), "{}", report.detail);
            assert!(has(report, "container=appcontainer"), "{}", report.detail);
            // The far edge of the files layer: the system tree stays readable
            // through the packages a low-box process is checked against.
            assert!(
                matches!(value(report, "system"), "readable" | "denied" | "absent"),
                "{}",
                report.detail
            );
            // Where a write from the container lands: `store` means it resolved
            // into the container's own redirected store, `user` means it did not.
            assert!(
                matches!(
                    value(report, "writes"),
                    "store" | "user" | "denied" | "unmeasured"
                ),
                "{}",
                report.detail
            );
            // The near edge: the parent's own file carries no ACE for the package
            // the container is checked against, and unlike a write a read cannot
            // be redirected into the container's store.
            assert!(has(report, "files=measured-denied"), "{}", report.detail);
            if profile.runs_helper() {
                assert!(has(report, "helper_scope=libs"), "{}", report.detail);
            }
            // The opt-out is the profile's own decision, and the media profile
            // makes it by measuring: a helper that cannot run under the less
            // privileged container is measured again in the plain one rather
            // than reported as an environment problem. So a media report that
            // says the helper did not run must not also claim the opt-out.
            if profile.runs_helper()
                && value(report, "lpac") == "on"
                && let Some(parse) = fact(report, "parse")
            {
                assert!(
                    !parse.starts_with("media-unavailable") && !parse.starts_with("media-failed"),
                    "a media report under the opt-out must have been measured in the plain \
                     container first: {}",
                    report.detail
                );
            }
        }
        "plain" | "unrestricted" => {
            // No container was delivered. What was tried and passed over has to
            // say so: a fallback that leaves no trace is how a host ends up
            // running weaker creations than an operator thinks.
            assert!(
                !skipped0.is_empty(),
                "a container that was attempted and not delivered has to leave its reason in \
                 the report: {}",
                report.detail
            );
            assert!(
                skipped0.contains("every protection"),
                "only the container can be passed over first: {}",
                report.detail
            );
            if rung == "unrestricted" {
                assert!(
                    skipped1.contains("token and a job"),
                    "the launch that gave the token up has to say why: {}",
                    report.detail
                );
            } else {
                // The plain creation answered: nothing below it was tried.
                assert!(skipped1.is_empty(), "{}", report.detail);
            }
            assert!(!has(report, "container=appcontainer"), "{}", report.detail);
        }
        other => panic!("unknown creation {other}: {}", report.detail),
    }
}
