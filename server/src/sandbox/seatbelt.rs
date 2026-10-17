//! The Seatbelt profile the extraction child runs under on macOS.
//!
//! macOS has no unprivileged seccomp and no Landlock: `/usr/bin/sandbox-exec` applies the
//! profile, so the *parent* wraps the child in it ([`super::runner`]). The program path is
//! hardcoded so a `PATH` entry cannot substitute another program, and the profile is
//! deny-by-default: a socket is refused unless something allows it.
//!
//! # What this cannot close
//!
//! The media profile must fork, so its process layer is an *exec* bound: a helper that got
//! code running can still copy this process. Each copy inherits the profile and the
//! limits, but nothing bounds how many there are — the child says `fork=`, which is why
//! media never has the process item. The parsers deny the fork outright, and the probe's
//! `threads=`/`parse=` are what say they survive without it.
//!
//! # The media profile's helper
//!
//! `process-exec` is a `(literal …)` for the helper and the interpreter a script names;
//! read/map cover the helper, the libraries beside it and, only when it lives there, the
//! trees a packaged build loads from; `file-write-data` on `/dev/null` covers its streams.
//! A literal written from a Homebrew short path grants nothing, because Seatbelt matches
//! the resolved path. [`resolved`] writes the literals now.

#[cfg(target_os = "macos")]
use super::Protections;
use super::{Grants, Profile};
use std::path::{Path, PathBuf};

/// Hardcoded path to the Seatbelt runner.
pub(super) const PROGRAM: &str = "/usr/bin/sandbox-exec";

/// Arguments that wrap `exe` in [`profile_text`], with `--` separating them from
/// the command.
pub(super) fn args(exe: &Path, profile: Profile, grants: Grants<'_>) -> Vec<String> {
    vec![
        "-p".to_string(),
        profile_text(exe, profile, grants),
        "--".to_string(),
    ]
}

/// The profile the child runs under.
///
/// Allowed: `process-exec` for this binary alone, because the runner's own `exec` is
/// checked and a `(literal …)` is narrower than `(allow process-exec)`; `process-fork` for
/// media only, which copies the process to start the helper; `signal (target self)`;
/// `sysctl-read` of a pinned list, because an unfiltered read answers `kern.procargs2` —
/// another same-uid process's argv and environment, which is what the cleared environment
/// keeps away; `file-read*` of the root directory, because the child's working directory is
/// `/` and denying it aborts the process; read and `file-map-executable` under the system
/// library paths and for this binary, which the loader maps and resolves through; and
/// `/dev/null` and `/dev/urandom` for the streams and `getentropy`.
///
/// Everything else is denied, and reading root grants nothing beneath it. The media
/// addition is a literal `process-exec` for the helper and read/map for its libraries.
pub(super) fn profile_text(exe: &Path, profile: Profile, grants: Grants<'_>) -> String {
    let exe = escape(&exe.to_string_lossy());
    let mut extra = String::new();
    if profile.runs_helper() {
        // A grant is written for one path, so a relative helper is not granted at all: the shipped
        // default `ffmpeg` resolves against the child's directory, not where it was configured,
        // and the profile matches what the kernel resolved.
        if let Some(path) = grants.helper.filter(|path| is_grantable(path)) {
            let helper_path = resolved(path);
            let helper = escape(&helper_path.to_string_lossy());
            extra.push_str(&format!(
                " (allow process-exec (literal \"{helper}\")) \
                 (allow file-read* (literal \"{helper}\")) \
                 (allow file-map-executable (literal \"{helper}\"))"
            ));
            // The libraries beside the helper, one file at a time: a `(subpath …)` on the directory
            // would grant every file in it.
            for library in helper_libraries(&helper_path) {
                let library = escape(&library.to_string_lossy());
                extra.push_str(&format!(
                    " (allow file-read* file-map-executable (literal \"{library}\"))"
                ));
            }
            // A package manager's tree only when the helper lives in it: granting
            // `/opt/homebrew` to a `/usr/bin/ffmpeg` helper would open `/opt/homebrew/var`
            // for nothing.
            for tree in HELPER_LIBRARY_TREES {
                if helper_path.starts_with(Path::new(tree)) {
                    extra.push_str(&format!(
                        " (allow file-read* file-map-executable (subpath \"{tree}\"))"
                    ));
                }
            }
            // The streams are set to `/dev/null`, which is an open for *write*: without it
            // the spawn fails with `Operation not permitted` before the helper's code runs.
            // Linux grants the same device read and write.
            extra.push_str(" (allow file-write-data (literal \"/dev/null\"))");
            // A script helper needs its interpreter started first, and that path is a resolved
            // literal too: `/bin/sh` is a shim that re-execs `/bin/bash`.
            if let Some(interpreter) = super::shebang_interpreter(path)
                && is_grantable(&interpreter)
            {
                let interpreter = escape(&resolved(&interpreter).to_string_lossy());
                extra.push_str(&format!(
                    " (allow process-exec (literal \"{interpreter}\")) \
                     (allow file-read* (literal \"{interpreter}\")) \
                     (allow file-map-executable (literal \"{interpreter}\"))"
                ));
            }
        }
        if let Some(source) = grants.source.filter(|path| is_grantable(path)) {
            let source = escape(&resolved(source).to_string_lossy());
            extra.push_str(&format!(" (allow file-read* (literal \"{source}\"))"));
        }
    }
    // The fork is the one process operation the parsers deny. Whether they can is measured:
    // `threads=` and `parse=` say so. Media is the exception — it starts the helper.
    let fork = if profile.runs_helper() {
        " (allow process-fork)"
    } else {
        ""
    };
    // The dyld shared cache lives in the cryptex on Apple Silicon; on Intel
    // that path does not exist and the rule grants nothing.
    format!(
        "(version 1) (deny default) (allow process-exec (literal \"{exe}\")){extra}{fork} \
         (allow signal (target self)) {sysctl} \
         (allow file-read* file-test-existence (literal \"/\") (subpath \"/usr/lib\") \
         (subpath \"/System/Library\") (subpath \"/System/Volumes/Preboot/Cryptexes/OS\") \
         (literal \"/dev/null\") (literal \"/dev/urandom\") (literal \"{exe}\")) \
         (allow file-map-executable (subpath \"/usr/lib\") (subpath \"/System/Library\") \
         (subpath \"/System/Volumes/Preboot/Cryptexes/OS\") (literal \"{exe}\"))",
        sysctl = sysctl_read_rule()
    )
}

/// The `sysctl-read` rule, with the names the runtime reads.
///
/// A bare allow answers `kern.procargs2`, another same-uid process's argv *and
/// environment*, so a parser with code execution could recover the server's secrets. The
/// list is Codex's, without its operations.
fn sysctl_read_rule() -> String {
    const NAMES: &[&str] = &[
        "hw.activecpu",
        "hw.busfrequency_compat",
        "hw.byteorder",
        "hw.cacheconfig",
        "hw.cachelinesize_compat",
        "hw.cpufamily",
        "hw.cpufrequency_compat",
        "hw.cputype",
        "hw.l1dcachesize_compat",
        "hw.l1icachesize_compat",
        "hw.l2cachesize_compat",
        "hw.l3cachesize_compat",
        "hw.logicalcpu_max",
        "hw.machine",
        "hw.model",
        "hw.memsize",
        "hw.ncpu",
        "hw.nperflevels",
        "hw.packages",
        "hw.pagesize_compat",
        "hw.pagesize",
        "hw.physicalcpu",
        "hw.physicalcpu_max",
        "hw.logicalcpu",
        "hw.cpufrequency",
        "hw.tbfrequency_compat",
        "hw.vectorunit",
        "machdep.cpu.brand_string",
        "kern.argmax",
        "kern.hostname",
        "kern.maxfilesperproc",
        "kern.maxproc",
        "kern.osproductversion",
        "kern.osrelease",
        "kern.ostype",
        "kern.osvariant_status",
        "kern.osversion",
        "kern.secure_kernel",
        "kern.sysv.semmns",
        "kern.usrstack64",
        "kern.version",
        "sysctl.proc_cputype",
        "vm.loadavg",
    ];
    const PREFIXES: &[&str] = &[
        "hw.optional.arm.",
        "hw.optional.armv8_",
        "hw.perflevel",
        "kern.proc.pgrp.",
        "kern.proc.pid.",
        "net.routetable.",
    ];

    let mut rule = String::from("(allow sysctl-read");
    for name in NAMES {
        rule.push_str(&format!(" (sysctl-name \"{name}\")"));
    }
    for prefix in PREFIXES {
        rule.push_str(&format!(" (sysctl-name-prefix \"{prefix}\")"));
    }
    rule.push(')');
    rule
}

/// The shared libraries beside the helper, linked by a `@loader_path`-relative path.
///
/// `*.dylib` and `*.so*` are granted one file at a time; an unlistable directory yields
/// nothing, and the helper says what it needs through `parse=` rather than widening to a
/// directory.
fn helper_libraries(helper: &Path) -> Vec<PathBuf> {
    let Some(directory) = helper.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut libraries: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".dylib") || name.contains(".so"))
        })
        .collect();
    libraries.sort();
    libraries
}

/// The path as the kernel resolves it, for a `(literal …)` form.
///
/// Seatbelt matches the resolved path, not the typed one: a Homebrew helper is checked as
/// `/opt/homebrew/Cellar/ffmpeg/<version>/bin/ffmpeg`, so a literal from the short path
/// grants nothing. A path that cannot be resolved is left as it was, and the grant then
/// fails the way the exec does.
fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Escape a path for a `(literal "…")` form.
///
/// Paths are data, never policy text: a directory named `") (allow` must not rewrite the
/// profile. `\` and `"` are the complete set that can end a Scheme string early, and the
/// backslash is escaped first. Escaping whitespace as `\n`/`\t` would be a correctness bug
/// — R5RS does not define them — so a path carrying a control character is refused
/// outright by [`is_grantable`].
fn escape(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Whether a configured path can be written into the profile as a rule.
///
/// Three ways it cannot: it is not absolute, so path matching cannot agree with it; it
/// carries a control character the profile text cannot vouch for; or it is empty, the
/// prefix of every path. Refusing is fail-closed: the helper does not start, `parse=`
/// says so, and the media profile reports itself unavailable.
fn is_grantable(path: &Path) -> bool {
    path.is_absolute()
        && !path.as_os_str().is_empty()
        && !path
            .to_string_lossy()
            .chars()
            .any(|character| character.is_control())
}

/// The trees a package-manager helper keeps its libraries in: Homebrew on both
/// architectures and MacPorts, granted read/map and never `process-exec`, and **only when
/// the helper lives inside** — a `/usr/bin` helper has no business reading
/// `/opt/homebrew/var`.
const HELPER_LIBRARY_TREES: &[&str] = &["/opt/homebrew", "/usr/local", "/opt/local"];

/// Clamp the child's own resources; the rest comes from the runner.
///
/// The address-space limit is stated against what is already mapped, which is what makes
/// Darwin accept it (see `super::macos::mapped_address_space`); a host that refused it
/// reports the limits item missing rather than a weaker bound.
#[cfg(target_os = "macos")]
pub(super) fn confine(profile: Profile, grants: Grants<'_>) -> (Protections, Vec<String>) {
    let mut protections = Protections::default();
    let mut detail = Vec::new();

    let limits = super::clamp_resources();
    if limits.enforced(false) {
        protections.limits = true;
    }
    detail.push(format!("limits={}", limits.detail));
    if profile.runs_helper() {
        // How wide the media grants are, in one token: `trees` when the helper lives in a
        // package-manager tree, `libs` otherwise (helper, interpreter, files beside it).
        if let Some(path) = grants.helper.filter(|path| is_grantable(path)) {
            let resolved = resolved(path);
            let scope = if HELPER_LIBRARY_TREES
                .iter()
                .any(|tree| resolved.starts_with(Path::new(tree)))
            {
                "trees"
            } else {
                "libs"
            };
            detail.push(format!("helper_scope={scope}"));
        }
        // Media keeps `(allow process-fork)` because it starts the helper by copying this
        // process. Nothing here bounds how many copies: the CPU limit is per process, and no
        // operation refuses the `setsid`/`setpgid` a copy would call to leave the group. The
        // parent's timeout is what is left, and the report says so.
        detail.push("media_process=unbounded".to_string());
    }
    (protections, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runner_path_is_hardcoded() {
        // A `PATH` entry must not be able to substitute a different program for
        // the one that applies the profile.
        assert_eq!(PROGRAM, "/usr/bin/sandbox-exec");
    }

    #[test]
    fn the_profile_denies_by_default_and_grants_no_write_or_network() {
        let profile = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Documents,
            Grants::default(),
        );
        assert!(profile.starts_with("(version 1) (deny default)"));
        // A parser may not copy the process; media is the profile that keeps the fork.
        assert!(!profile.contains("(allow process-fork)"));
        let media = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Media,
            Grants {
                helper: Some(Path::new("/usr/bin/ffmpeg")),
                source: None,
            },
        );
        assert!(
            media.contains("(allow process-fork)"),
            "the media profile starts the helper by copying: {media}"
        );
        // The child's working directory is `/`: a process that cannot read it is aborted, not
        // refused.
        assert!(profile.contains("(allow file-read* file-test-existence (literal \"/\")"));
        for path in [
            "/usr/lib",
            "/System/Library",
            "/System/Volumes/Preboot/Cryptexes/OS",
        ] {
            assert!(
                profile.contains(&format!("(subpath \"{path}\")")),
                "{path} is granted: {profile}"
            );
        }
        assert!(profile.contains("(literal \"/opt/nanofile/nanofile\")"));
        // Nothing may grant a write, a socket, or a mach service.
        for forbidden in ["file-write", "network", "mach-lookup", "iokit"] {
            assert!(
                !profile.contains(forbidden),
                "the profile must not mention {forbidden}: {profile}"
            );
        }
        // Starting a program is granted for this binary alone: a bare `(allow process-exec)` would
        // let a parser start anything it can read.
        assert!(profile.contains("(allow process-exec (literal \"/opt/nanofile/nanofile\"))"));
        assert!(!profile.contains("(allow process-exec)"));
        assert!(profile.contains(
            "(allow file-map-executable (subpath \"/usr/lib\") (subpath \"/System/Library\") \
             (subpath \"/System/Volumes/Preboot/Cryptexes/OS\") \
             (literal \"/opt/nanofile/nanofile\"))"
        ));
    }

    /// The sysctl grant names what it allows.
    ///
    /// A bare `(allow sysctl-read)` answers `kern.procargs2`, another same-uid process's argv
    /// **and environment**: the parent clears the child's environment because it holds the
    /// master secret, and an unfiltered sysctl would hand those values back. The rule carries
    /// a filter, and the name that matters is not in it.
    #[test]
    fn the_sysctl_grant_is_filtered_and_excludes_the_process_environment() {
        let profile = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Documents,
            Grants::default(),
        );
        assert!(profile.contains("(allow sysctl-read (sysctl-name "));
        assert!(
            !profile.contains("(allow sysctl-read)"),
            "the grant must not be unfiltered: {profile}"
        );
        assert!(
            !profile.contains("kern.procargs2"),
            "another process's environment must not be readable: {profile}"
        );
        for name in ["hw.ncpu", "hw.memsize", "kern.argmax"] {
            assert!(
                profile.contains(&format!("(sysctl-name \"{name}\")")),
                "{name} must stay readable: {profile}"
            );
        }
    }

    /// A packaged helper's libraries are granted one file at a time, beside it —
    /// not the directory they sit in.
    #[test]
    fn the_libraries_beside_a_helper_are_granted_one_by_one() {
        let directory = std::env::temp_dir().join(format!(
            "nanofile-seatbelt-libs-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("create");
        let helper = directory.join("ffmpeg");
        std::fs::write(&helper, b"#!/bin/sh\n").expect("write helper");
        std::fs::write(directory.join("libavcodec.61.dylib"), b"x").expect("write library");
        std::fs::write(directory.join("libavutil.so"), b"x").expect("write library");
        std::fs::write(directory.join("secret.txt"), b"secret").expect("write secret");

        let found = helper_libraries(&helper);
        let names: Vec<String> = found
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
            .collect();
        assert!(
            names.contains(&"libavcodec.61.dylib".to_string()),
            "{names:?}"
        );
        assert!(names.contains(&"libavutil.so".to_string()), "{names:?}");
        assert!(!names.contains(&"secret.txt".to_string()), "{names:?}");
        assert!(!names.contains(&"ffmpeg".to_string()), "{names:?}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A degenerate helper path must not become a grant that covers everything.
    ///
    /// `(subpath …)` is a prefix match, so `""` — what `Path::parent` gives for a bare
    /// `ffmpeg` whose lookup failed — and `/` are the whole filesystem; neither reaches the
    /// profile text, and a relative helper may not either.
    ///
    /// Unix-only, with the three tests below: Windows answers `is_absolute() == false` for
    /// every one of these paths. Linux still runs them, which covers the text builder.
    #[cfg(unix)]
    #[test]
    fn a_degenerate_helper_path_grants_nothing() {
        let text = |helper: &str| {
            profile_text(
                Path::new("/opt/nanofile/nanofile"),
                Profile::Media,
                Grants {
                    helper: Some(Path::new(helper)),
                    source: None,
                },
            )
        };

        // A helper inside a package-manager tree grants the tree, never the directory it sits in.
        let homebrew = text("/opt/homebrew/bin/ffmpeg");
        assert!(homebrew.contains("(subpath \"/opt/homebrew\")"));
        assert!(
            !homebrew.contains("(subpath \"/opt/homebrew/bin\")"),
            "the helper's own directory must not be a grant: {homebrew}"
        );
        let system = text("/usr/bin/ffmpeg");
        for tree in HELPER_LIBRARY_TREES {
            assert!(
                !system.contains(&format!("(subpath \"{tree}\")")),
                "{tree} must not be granted for a helper outside it: {system}"
            );
        }
        for degenerate in ["ffmpeg", "/ffmpeg", ""] {
            let profile = text(degenerate);
            assert!(
                !profile.contains("(subpath \"\")") && !profile.contains("(subpath \"/\")"),
                "{degenerate} must not widen the profile: {profile}"
            );
        }
        assert!(!is_grantable(Path::new("ffmpeg")));
        assert!(!is_grantable(Path::new("/tmp/with\nnewline")));
        assert!(is_grantable(Path::new("/usr/bin/ffmpeg")));
    }

    /// A path is data, never policy text: every quote it brings is escaped, so
    /// it stays inside the literal it was written into.
    #[test]
    fn a_path_cannot_rewrite_the_profile() {
        let unescaped =
            |profile: &str| profile.matches('"').count() - profile.matches(r#"\""#).count();
        let clean = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Documents,
            Grants::default(),
        );
        let injected = profile_text(
            Path::new("/tmp/\") (allow file-write*) \"/"),
            Profile::Documents,
            Grants::default(),
        );

        assert!(injected.matches(r#"\""#).count() >= 2);
        assert_eq!(
            unescaped(&injected),
            unescaped(&clean),
            "the path adds no unescaped quote to the profile: {injected}"
        );
    }

    /// The media profile may execute the helper, read it, the scratch source and a packaged
    /// helper's library trees — but `process-exec` stays a literal, so starting a program
    /// reaches the one file the parent named. Unix-only.
    #[cfg(unix)]
    #[test]
    fn only_the_media_profile_may_execute_the_helper() {
        let helper = Path::new("/usr/bin/ffmpeg");
        let source = Path::new("/data/tmp/media_thumbs/x.bin");
        let grants = Grants {
            helper: Some(helper),
            source: Some(source),
        };

        let document = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Documents,
            grants,
        );
        assert!(
            !document.contains("ffmpeg") && !document.contains("media_thumbs"),
            "a documents profile must not reach the helper or the source: {document}"
        );
        assert!(!document.contains("file-write"));

        let media = profile_text(Path::new("/opt/nanofile/nanofile"), Profile::Media, grants);
        assert!(media.contains("(allow process-exec (literal \"/usr/bin/ffmpeg\"))"));
        assert!(media.contains("(allow file-read* (literal \"/usr/bin/ffmpeg\"))"));
        assert!(media.contains("(allow file-read* (literal \"/data/tmp/media_thumbs/x.bin\"))"));
        assert!(
            media.contains("(allow file-map-executable (literal \"/usr/bin/ffmpeg\"))"),
            "{media}"
        );
        // The helper's directory is not a grant, nor is a tree it does not live in: `process-exec`
        // has no `subpath`, and `file-read*` reaches one literal at a time.
        assert!(
            !media.contains("(subpath \"/usr/bin\")"),
            "the helper's directory must not be a grant: {media}"
        );
        for tree in HELPER_LIBRARY_TREES {
            assert!(
                !media.contains(&format!("(subpath \"{tree}\")")),
                "{tree} must not be granted for a helper outside it: {media}"
            );
        }
        assert!(
            !media.contains("process-exec (subpath"),
            "no directory may be executable: {media}"
        );
        // The one write is the helper's streams on `/dev/null`: an open for write is not the read
        // the base profile grants.
        assert!(media.contains("(allow file-write-data (literal \"/dev/null\"))"));
        assert_eq!(
            media.matches("file-write").count(),
            1,
            "the only write is /dev/null: {media}"
        );
        assert!(!media.contains("network"));
    }

    /// A script helper names its interpreter on its `#!` line, and that interpreter is the
    /// only other program media may start. The literal is the *resolved* one: the sandbox
    /// checks the path `/bin/sh` resolves to.
    ///
    /// Unix-only: `/bin/sh` is not a path this side of the platform can grant.
    #[cfg(unix)]
    #[test]
    fn a_script_helper_names_the_one_other_program_that_may_run() {
        let path = std::env::temp_dir().join(format!(
            "nanofile-seatbelt-{}-{:?}.sh",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("write");
        let grants = Grants {
            helper: Some(path.as_path()),
            source: None,
        };

        let interpreter = resolved(Path::new("/bin/sh"));
        let interpreter = escape(&interpreter.to_string_lossy());
        let media = profile_text(Path::new("/opt/nanofile/nanofile"), Profile::Media, grants);
        assert!(
            media.contains(&format!("(allow process-exec (literal \"{interpreter}\"))")),
            "{media}"
        );
        assert!(media.contains(&format!("(allow file-read* (literal \"{interpreter}\"))")));
        assert!(
            !media.contains("process-exec (subpath"),
            "the interpreter is a literal, not a tree: {media}"
        );

        let document = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Documents,
            grants,
        );
        assert!(
            !document.contains(&interpreter),
            "a documents profile starts nothing: {document}"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A helper reached through a symlink is granted under the resolved path.
    ///
    /// `/opt/homebrew/bin/ffmpeg` is checked as `…/Cellar/ffmpeg/<version>/bin/ffmpeg`, so a
    /// literal from the typed path grants nothing: the exec is refused while the binary is
    /// readable.
    #[cfg(unix)]
    #[test]
    fn a_helper_is_granted_under_the_path_the_kernel_resolves() {
        let dir = std::env::temp_dir().join(format!(
            "nanofile-resolve-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let real = dir.join("real-helper");
        std::fs::write(&real, b"#!/bin/sh\nexit 0\n").expect("write");
        let link = dir.join("helper-link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let media = profile_text(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Media,
            Grants {
                helper: Some(link.as_path()),
                source: None,
            },
        );
        let real = escape(
            &std::fs::canonicalize(&real)
                .expect("canonical")
                .to_string_lossy(),
        );
        assert!(
            media.contains(&format!("(allow process-exec (literal \"{real}\"))")),
            "{media}"
        );
        assert!(
            !media.contains(&format!(
                "process-exec (literal \"{}\")",
                escape(&link.to_string_lossy())
            )),
            "the typed path grants nothing: {media}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The runner is what confines files, the network and process creation, so the child must
    /// be told those protections are not its own.
    #[test]
    fn the_arguments_wrap_the_child_in_the_profile() {
        let args = args(
            Path::new("/opt/nanofile/nanofile"),
            Profile::Documents,
            Grants::default(),
        );
        assert_eq!(args[0], "-p");
        assert_eq!(args[2], "--");
        assert!(args[1].starts_with("(version 1)"));
    }
}
