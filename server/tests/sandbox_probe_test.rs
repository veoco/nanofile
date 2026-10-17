//! What each profile's startup report says on this host.
//!
//! The typed facts come from `Report` and the mechanism tokens from the profile that installed
//! them; no fixture is built, so the worker's process-global executable is left alone. The probe
//! measures under the master switch alone, and Windows walk steps that were passed over leave
//! their name and reason in `skipped0`/`skipped1`/`skipped2`; each test goes through
//! `with_requirement`, which clears both records, so the whole ladder is asserted.

mod common;

#[cfg(windows)]
use std::time::{Duration, Instant};

use common::sandbox::{
    configure_media_probe_requirement, configure_probe_requirement, fact, has, helper, helper_runs,
    probe, with_requirement, without_ffmpeg,
};
use server::sandbox::{JobVerdict, Profile, Report};

fn value<'a>(report: &'a Report, key: &str) -> &'a str {
    fact(report, key).unwrap_or_else(|| panic!("no {key} in: {}", report.detail))
}

/// A bare token of the fact list, matched by its prefix.
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

#[cfg(target_os = "linux")]
fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

/// What every profile on every platform has to say.
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
    the_items_answer(&report);
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
    the_items_answer(&report);
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
    the_items_answer(&report);
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

/// The shape a host that refuses the container produces: the bound alone. The feature runs on
/// the Job Object alone, and the report names the missing layer rather than disabling the
/// feature. Which shape a runner produces is the runner's answer; the container's own case
/// asserts the rest.
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

    assert!(report.protections.limits, "{line}");
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

/// The items the profile has: both files-layer platforms carry every parser item, and media has
/// all but `process` (it starts the helper by copying the process). A Windows host that can
/// start neither a container nor a restrained child gives the resource bound alone, and the item
/// list says so.
fn the_items_answer(report: &Report) {
    #[cfg(not(windows))]
    {
        assert!(report.protections.limits, "{}", report.detail);
        assert!(report.protections.files, "{}", report.detail);
        assert!(report.protections.network, "{}", report.detail);
        assert_eq!(
            report.protections.process,
            !report.profile.runs_helper(),
            "the media profile starts the helper by definition: {}",
            report.detail
        );
    }
    #[cfg(windows)]
    {
        assert!(report.protections.limits, "{}", report.detail);
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

/// The creation the parent settled on. Two shapes reach this file: the container where one could
/// be created, and the bound under the job alone where a host sandbox refuses it. Both carry the
/// job object, the token (or its absence) and the mitigations; only the container adds files and
/// network.
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

    // Which creations were passed over, in the order the walk tried them. The
    // report has to hand back a ladder — the walk starts at the container every
    // time this file probes, because the fixture installs its requirement first —
    // and each entry has to be a name and a reason: a step that does not say why
    // it stepped down is a fallback nothing can be read off.
    let skipped = [
        fact(report, "skipped0").unwrap_or_default(),
        fact(report, "skipped1").unwrap_or_default(),
        fact(report, "skipped2").unwrap_or_default(),
    ];
    // The creation an entry names, or the whole entry when it names one wrongly.
    fn step(entry: &str) -> &str {
        entry
            .split_once(':')
            .map_or(entry, |(creation, _)| creation)
    }
    // A run that never reached the walk, a walk that jumped a creation, and a
    // walk that went below the last creation this platform has all fail here.
    let tried: Vec<&str> = skipped
        .iter()
        .filter(|entry| !entry.is_empty())
        .map(|entry| step(entry))
        .collect();
    let expected = match rung {
        "container" => &[][..],
        "plain" => &["confined"][..],
        _ => &["confined", "plain"][..],
    };
    assert_eq!(
        tried, expected,
        "the walk steps down one creation at a time: {}",
        report.detail
    );
    // The reason is what the creation said when it did not report, so an entry
    // that is only a name is a walk that forgot why it stepped down.
    assert!(
        skipped.iter().all(|entry| {
            entry
                .split_once(':')
                .is_none_or(|(_, reason)| !reason.is_empty())
        }),
        "a creation that was passed over has to say why: {}",
        report.detail
    );

    match rung {
        "container" => {
            // The container answered, so nothing was passed over to reach it.
            // The refusals of an earlier launch are not carried into this
            // report: they belong to the walk that produced them.
            assert!(skipped[0].is_empty(), "{}", report.detail);
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
                !skipped[0].is_empty(),
                "a container that was attempted and not delivered has to leave its reason in \
                 the report: {}",
                report.detail
            );
            if rung == "unrestricted" {
                // The token was given up as well, so the step below the plain
                // creation has to be in the record too.
                assert!(
                    !skipped[1].is_empty(),
                    "the launch that gave the token up has to say why: {}",
                    report.detail
                );
            } else {
                // The plain creation answered: nothing below it was tried.
                assert!(skipped[1].is_empty(), "{}", report.detail);
            }
            // The tokenless creation is the last this platform can start a child
            // with, so nothing may have been passed over below it.
            assert!(skipped[2].is_empty(), "{}", report.detail);
            assert!(!has(report, "container=appcontainer"), "{}", report.detail);
        }
        other => panic!("unknown creation {other}: {}", report.detail),
    }
}
