//! The Seatbelt profile the extraction child runs under on macOS.
//!
//! macOS has no unprivileged seccomp and no Landlock: a profile is applied by
//! `/usr/bin/sandbox-exec`, so the *parent* wraps the child in it
//! ([`super::runner`]) and the child only clamps its own resources.
//!
//! The program path is hardcoded — the same choice Codex's sandbox makes — so a
//! `PATH` entry cannot substitute a different program for the real one.
//!
//! The profile is deny-by-default, which is what makes it worth more than the
//! write-denial profiles in circulation: a socket is refused by `(deny default)`
//! unless something allows it, and nothing here does.
//!
//! # What this cannot close
//!
//! `process-fork` is allowed because Seatbelt charges the parsers' thread to it,
//! so the process layer here is an *exec* bound: a document that got code running
//! could still make copies of this process. Each copy inherits the profile and
//! the resource limits — the address-space limit with them — so the copies are
//! confined, but nothing bounds how many there are. The child says so (`fork=`
//! in its report), which is the note the settings page puts beside the process
//! item rather than a second, stronger grade.
//!
//! # The media profile's helper
//!
//! The one profile that starts a program has a grant for it — `process-exec` as a
//! `(literal …)` for the helper and for the interpreter a script helper names, and
//! `file-read*`/`file-map-executable` for the helper, its directory and the trees
//! a packaged one loads from — plus `file-write-data` on `/dev/null`, because the
//! helper's standard streams are set to it and an open for write is not the read
//! grant the base profile carries.
//!
//! Two things had to be right for the helper to start, and each looked like the
//! other from the outside: the spawn was refused with the same
//! `media-unavailable(Operation not permitted)` for both. The streams first —
//! `/dev/null` for write, which the profile did not grant, so the spawn died in
//! its own stdio setup before the exec. Then the path: Seatbelt matches the path
//! the filesystem *resolved*, and a Homebrew helper is reached through
//! `/opt/homebrew/bin/ffmpeg` while it is checked as
//! `/opt/homebrew/Cellar/ffmpeg/<version>/bin/ffmpeg`, so a literal written from
//! the typed path granted nothing — measured in the sandbox's own log as
//! `deny(1) process-exec* /opt/homebrew/Cellar/ffmpeg/9.0.1_1/bin/ffmpeg` for a
//! profile whose literal was the short path. [`resolved`] is what writes the
//! literals now, and the child's own image has been resolved by its caller for
//! the same reason all along. A synthetic profile with the same grants was what
//! ruled out the grant itself in between.

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
/// Allowed, and why:
///
/// * `process-exec` for this binary and nothing else. The `exec` the runner
///   performs to start the child is itself checked, so `(deny default)` refuses
///   to start anything without this grant — `sandbox-exec: execvp() of …
///   failed: Operation not permitted` is what its absence looks like. A
///   `(literal …)` here is narrower than a bare `(allow process-exec)`: no
///   second program can be started, and re-running this binary cannot escape
///   the profile, because a sandboxed process passes its own on to its
///   children.
/// * `process-fork` — the parsers run their work on a thread, and Seatbelt
///   charges that to `process-fork`, so denying it would break them. A fork
///   only duplicates this process: the copy inherits the profile, so it can
///   still `exec` nothing but this binary.
/// * `signal (target self)` — the runtime's own bookkeeping.
/// * `sysctl-read` — the allocator and the runtime read a few sysctl values
///   while starting up.
/// * `file-read*` of the root directory itself, which is what a process does
///   about a directory it is sitting in: the child's working directory is `/`
///   (`super::super::worker` sets it so nothing is relative), and the runtime
///   reads it on the way up. Denying it is fatal rather than inconvenient —
///   Seatbelt aborts the process with `SIGABRT` before it can say anything, so
///   the failure reads as a silent child. Codex carries the same grant with the
///   same reason ("Allow processes to get their current working directory"),
///   and it was isolated on macOS 26 by toggling one rule at a time: with only
///   this grant added to an otherwise identical deny-default profile, the
///   process starts.
/// * `file-read*` under the system library paths and for this binary, and
///   `file-map-executable` for the same paths: macOS binds symbols lazily and
///   maps an executable image with a permission of its own, so the first call
///   into an unresolved symbol, and the mapping of the binary itself, both send
///   the loader back to a file. Without them the child dies before it reads a
///   byte of its request.
/// * `/dev/null` and `/dev/urandom`: the runtime keeps the first for the
///   standard streams the parent set up, and `getentropy` may fall back to the
///   second.
///
/// Everything else — every write, every socket, every other path — is denied.
/// Reading the root directory lists it and grants nothing beneath it: `/etc`,
/// `/Users` and every other path stay denied, which is what the child's own
/// measurement of the files item checks.
///
/// The media profile is the one addition: it may execute the configured helper
/// and read the helper's image and the scratch source the parent wrote. The
/// `process-exec` grant is a literal, so it reaches exactly that one file — of
/// the path the kernel resolves, which is what [`resolved`] is for.
///
/// What a helper needs *beyond* its own image is read and map, never execute: a
/// packaged ffmpeg links against its own tree and dyld will not start it without
/// those paths, so the helper's own directory and the trees the package managers
/// install into are granted `file-read*` and `file-map-executable`. That is the
/// same shape as the Linux grant — libraries are readable, and the only program
/// that may be started is still the helper the parent named.
pub(super) fn profile_text(exe: &Path, profile: Profile, grants: Grants<'_>) -> String {
    let exe = escape(&exe.to_string_lossy());
    let mut extra = String::new();
    if profile.runs_helper() {
        // A grant is written for one path, so a path that cannot be stated as
        // one is not granted at all: a relative helper (the shipped default is
        // the bare name `ffmpeg`, and a `PATH` lookup that failed leaves it
        // relative) resolves against the child's directory, which is not where
        // it was configured, and the profile matches the path the kernel
        // resolved. Refusing the grant is the same outcome as today's useless
        // literal, without the rule that followed it — see `granted_directory`.
        if let Some(path) = grants.helper.filter(|path| is_grantable(path)) {
            let helper_path = resolved(path);
            let helper = escape(&helper_path.to_string_lossy());
            extra.push_str(&format!(
                " (allow process-exec (literal \"{helper}\")) \
                 (allow file-read* (literal \"{helper}\")) \
                 (allow file-map-executable (literal \"{helper}\"))"
            ));
            if let Some(directory) = granted_directory(&helper_path) {
                let directory = escape(&directory.to_string_lossy());
                extra.push_str(&format!(
                    " (allow file-read* file-map-executable (subpath \"{directory}\"))"
                ));
            }
            for tree in HELPER_LIBRARY_TREES {
                extra.push_str(&format!(
                    " (allow file-read* file-map-executable (subpath \"{tree}\"))"
                ));
            }
            // The helper's streams are set to `/dev/null`, and that is an open
            // for *write*: the read grant the base profile carries is not enough,
            // and without this the spawn fails with `Operation not permitted`
            // before the helper's own code ever runs — which is what it was
            // doing, measured as `parse=media-unavailable(EPERM)` while the same
            // program was readable (`helper=allowed`). Linux grants this device
            // read *and* write to the same profile, which is where the shape
            // comes from.
            extra.push_str(" (allow file-write-data (literal \"/dev/null\"))");
            // A helper that is a script needs its own interpreter started before
            // its code runs. That path is a literal too — and the resolved one:
            // `/bin/sh` is a shim on this platform that re-execs `/bin/bash`.
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
    // The dyld shared cache lives in the cryptex on Apple Silicon; on Intel
    // that path does not exist and the rule grants nothing.
    format!(
        "(version 1) (deny default) (allow process-exec (literal \"{exe}\")){extra} \
         (allow process-fork) (allow signal (target self)) (allow sysctl-read) \
         (allow file-read* file-test-existence (literal \"/\") (subpath \"/usr/lib\") \
         (subpath \"/System/Library\") (subpath \"/System/Volumes/Preboot/Cryptexes/OS\") \
         (literal \"/dev/null\") (literal \"/dev/urandom\") (literal \"{exe}\")) \
         (allow file-map-executable (subpath \"/usr/lib\") (subpath \"/System/Library\") \
         (subpath \"/System/Volumes/Preboot/Cryptexes/OS\") (literal \"{exe}\"))"
    )
}

/// The path as the kernel resolves it, for a `(literal …)` form.
///
/// Seatbelt matches the path the filesystem resolved, not the one that was typed:
/// a Homebrew helper is reached through `/opt/homebrew/bin/ffmpeg` and checked as
/// `/opt/homebrew/Cellar/ffmpeg/<version>/bin/ffmpeg`, so a literal written from
/// the typed path grants nothing at all — measured as
/// `deny(1) process-exec* /opt/homebrew/Cellar/ffmpeg/9.0.1_1/bin/ffmpeg` for a
/// profile whose literal was the short path. The child's own image is resolved by
/// its caller for the same reason. A path that cannot be resolved is left as it
/// was: the grant then fails the way the exec does, with the same path in the
/// report.
fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Escape a path for a `(literal "…")` form.
///
/// Paths are data here, never policy text: a directory named `") (allow` must
/// not be able to rewrite the profile.
///
/// `\` and `"` are the *complete* set of characters that can end a Scheme string
/// literal early — a raw newline or tab inside `"…"` is an ordinary character —
/// and both are escaped here, with the backslash first so that a path ending in
/// one cannot escape the quote that closes it. Escaping the whitespace controls
/// as `\n`/`\t` would not be a safety improvement but a correctness bug: R5RS
/// does not define those escapes, so the grant would name a path containing a
/// backslash and match nothing. Paths that carry a control character are refused
/// outright instead, by [`is_grantable`].
fn escape(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Whether a configured path can be written into the profile as a rule.
///
/// Three ways it cannot, and each would produce a rule that does not mean what it
/// says rather than no rule at all:
///
/// * it is not absolute, so the profile's own path matching (which is against
///   what the kernel resolved) cannot agree with it;
/// * it carries a control character, which the profile text would pass through
///   into a string literal whose reader this side cannot verify;
/// * it is empty, which is a prefix of every path.
///
/// Refusing the grant is fail-closed: the helper does not start, `parse=` says
/// so, and the media profile reports itself unavailable rather than confined by
/// a rule nobody meant.
fn is_grantable(path: &Path) -> bool {
    path.is_absolute()
        && !path.as_os_str().is_empty()
        && !path
            .to_string_lossy()
            .chars()
            .any(|character| character.is_control())
}

/// The directory a media grant may name, when naming one is what was meant.
///
/// The grant is a `(subpath …)`, which is a prefix match, so the paths that
/// prefix everything are the ones that must never be written: `""` — which is
/// what `Path::parent` gives for a bare command name, the shipped
/// `storage.ffmpeg_path` default when its `PATH` lookup failed — and `/`, which
/// is what it gives for a helper at the root. Either would hand the media child
/// the whole filesystem to read and map; measured as `(subpath "")` for a
/// helper configured as `ffmpeg` on a host whose `PATH` did not resolve it.
fn granted_directory(helper: &Path) -> Option<&Path> {
    let directory = helper.parent()?;
    if directory.as_os_str().is_empty() || directory == Path::new("/") {
        return None;
    }
    Some(directory)
}

/// The trees a helper installed by a package manager keeps its libraries in.
///
/// Homebrew on both architectures and MacPorts. Granted `file-read*` and
/// `file-map-executable` and never `process-exec`: dyld has to map a library to
/// start the helper at all, and starting a *program* out of one of these trees
/// is still refused, which is what keeps the process grant a literal.
const HELPER_LIBRARY_TREES: &[&str] = &["/opt/homebrew", "/usr/local", "/opt/local"];

/// Clamp the child's own resources. The rest of the confinement comes from the
/// runner.
///
/// The address-space limit is stated against the space this process already has
/// mapped, which is what makes Darwin accept it at all (see
/// `super::macos::mapped_address_space`). There is no fallback: the oldest
/// macOS this build supports takes the limit, and a host that refused it would
/// report the limits item as missing rather than pretend a weaker bound is one.
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
        // The widest this platform's media grants get, and wider than "the
        // helper and its loader": a packaged build links against its own tree, so
        // the package-manager trees are readable and mappable by the helper. Said
        // out loud because the profile text is the only other place it appears,
        // and an admin does not read the profile text.
        if grants.helper.is_some_and(|path| is_grantable(path)) {
            detail.push("helper_scope=trees".to_string());
        }
        // `(allow process-fork)` is in every profile — the parsers' thread needs
        // it — and the media profile additionally starts the helper by copying
        // this process. Nothing on this platform bounds how many such copies
        // there may be: the CPU limit is per process, so it bounds each copy and
        // not their number. The parent's wall-clock timeout and the process group
        // it kills are the bound, and the report says so instead of letting the
        // absence of a kernel one be inferred from a note about `fork`.
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
        assert!(profile.contains("(allow process-fork)"));
        // The child's working directory is `/`: a process that cannot read the
        // directory it sits in is aborted, not refused, so this grant is
        // load-bearing rather than cosmetic.
        assert!(profile.contains("(allow file-read* file-test-existence (literal \"/\")"));
        // The loader gets the same paths to read and to map executable, and
        // nothing else.
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
        // Starting a program is granted for this binary alone: the runner's own
        // `exec` needs it, and a bare `(allow process-exec)` would let a
        // document parser start anything it can read.
        assert!(profile.contains("(allow process-exec (literal \"/opt/nanofile/nanofile\"))"));
        assert!(!profile.contains("(allow process-exec)"));
        // Reading and mapping executable are granted for the same paths, so the
        // loader can map what it is allowed to read and nothing else.
        assert!(profile.contains(
            "(allow file-map-executable (subpath \"/usr/lib\") (subpath \"/System/Library\") \
             (subpath \"/System/Volumes/Preboot/Cryptexes/OS\") \
             (literal \"/opt/nanofile/nanofile\"))"
        ));
    }

    /// A degenerate helper path must not become a grant that covers everything.
    ///
    /// `(subpath …)` is a prefix match, so `""` — what `Path::parent` gives for
    /// the shipped `ffmpeg` default when its `PATH` lookup failed — and `/` are
    /// the whole filesystem. Neither may reach the profile text, and a relative
    /// helper may not reach it at all: the profile matches what the kernel
    /// resolved, so a rule written from a relative path grants nothing while
    /// looking like it grants something.
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

        assert!(text("/opt/homebrew/bin/ffmpeg").contains("(subpath \"/opt/homebrew/bin\")"));
        // The two prefixes of everything.
        for degenerate in ["ffmpeg", "/ffmpeg", ""] {
            let profile = text(degenerate);
            assert!(
                !profile.contains("(subpath \"\")") && !profile.contains("(subpath \"/\")"),
                "{degenerate} must not widen the profile: {profile}"
            );
        }
        assert_eq!(granted_directory(Path::new("ffmpeg")), None);
        assert_eq!(granted_directory(Path::new("/ffmpeg")), None);
        assert_eq!(granted_directory(Path::new("")), None);
        assert_eq!(
            granted_directory(Path::new("/opt/homebrew/bin/ffmpeg")),
            Some(Path::new("/opt/homebrew/bin"))
        );
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

        // The path's own quotes are escaped wherever the path is written (it
        // appears once per grant), so it cannot close a literal early and turn
        // what follows into a form of its own.
        assert!(injected.matches(r#"\""#).count() >= 2);
        assert_eq!(
            unescaped(&injected),
            unescaped(&clean),
            "the path adds no unescaped quote to the profile: {injected}"
        );
    }

    /// The media profile is the one that may execute the configured helper, and
    /// it may read the helper, the scratch source it was handed and the trees a
    /// packaged helper keeps its libraries in — but `process-exec` stays a
    /// literal, so starting a *program* reaches the one file the parent named.
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
        // The helper's own directory and the package-manager trees are readable
        // and mappable — a dynamically linked helper cannot start without them —
        // and never executable: `process-exec` has no `subpath` anywhere.
        assert!(
            media.contains("(allow file-read* file-map-executable (subpath \"/usr/bin\"))"),
            "{media}"
        );
        for tree in HELPER_LIBRARY_TREES {
            assert!(
                media.contains(&format!(
                    "(allow file-read* file-map-executable (subpath \"{tree}\"))"
                )),
                "{tree} is granted for the loader: {media}"
            );
        }
        assert!(
            !media.contains("process-exec (subpath"),
            "no directory may be executable: {media}"
        );
        // The one write this profile has is the helper's standard streams, which
        // are set to `/dev/null`: an open for write is not the read the base
        // profile grants, and a spawn without it dies before the exec.
        assert!(media.contains("(allow file-write-data (literal \"/dev/null\"))"));
        assert_eq!(
            media.matches("file-write").count(),
            1,
            "the only write is /dev/null: {media}"
        );
        assert!(!media.contains("network"));
    }

    /// A helper that is a script names its interpreter on its `#!` line, and that
    /// interpreter is the only other program the media profile may start. The
    /// documents profile names neither.
    ///
    /// The literal is the *resolved* interpreter: a script that names `/bin/sh`
    /// is started through whatever that resolves to here, and the sandbox checks
    /// the resolved path.
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

    /// A helper reached through a symlink is granted under the path the kernel
    /// resolves, not the one that was typed.
    ///
    /// A packaged helper is reached that way (`/opt/homebrew/bin/ffmpeg` →
    /// `…/Cellar/ffmpeg/<version>/bin/ffmpeg`) and Seatbelt checks the resolved
    /// path, so a literal written from the typed one grants nothing: the exec is
    /// refused while the same binary is readable, which is what CI measured as
    /// `helper=allowed,…,parse=media-unavailable(EPERM)`.
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

    /// The runner is what confines files, the network and process creation, so
    /// the child must be told that those protections are not its own to
    /// establish.
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
