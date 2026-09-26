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
//! a packaged one loads from. What CI measures today is that the child can *read*
//! the program (`helper=allowed`) and that the exec comes back `Operation not
//! permitted`, for a system binary and for a packaged ffmpeg alike, and with
//! either the library's `posix_spawn` path or `fork`+`exec`. The grant is written
//! as narrowly as it can be, so what is left is not the grant: it is what a
//! Seatbelt profile applied to *this* process allows a second generation to do.
//! Until that is settled, media thumbnails on macOS are off, and the report and
//! the settings page say so rather than claiming a helper that never ran.

#[cfg(target_os = "macos")]
use super::Protections;
use super::{Grants, Profile};
use std::path::Path;

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
/// `process-exec` grant is a literal, so it reaches exactly that one file.
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
        if let Some(path) = grants.helper {
            let helper = escape(&path.to_string_lossy());
            extra.push_str(&format!(
                " (allow process-exec (literal \"{helper}\")) \
                 (allow file-read* (literal \"{helper}\")) \
                 (allow file-map-executable (literal \"{helper}\"))"
            ));
            if let Some(directory) = path.parent() {
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
            // A helper that is a script needs its own interpreter started before
            // its code runs. That path is a literal too, so the grant is the
            // helper, its interpreter and the trees a helper loads from — no
            // directory anywhere is executable.
            if let Some(interpreter) = super::shebang_interpreter(path) {
                let interpreter = escape(&interpreter.to_string_lossy());
                extra.push_str(&format!(
                    " (allow process-exec (literal \"{interpreter}\")) \
                     (allow file-read* (literal \"{interpreter}\")) \
                     (allow file-map-executable (literal \"{interpreter}\"))"
                ));
            }
        }
        if let Some(source) = grants.source {
            let source = escape(&source.to_string_lossy());
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

/// Escape a path for a `(literal "…")` form.
///
/// Paths are data here, never policy text: a directory named `") (allow` must
/// not be able to rewrite the profile.
fn escape(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
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
pub(super) fn confine() -> (Protections, Vec<String>) {
    let mut protections = Protections::default();
    let mut detail = Vec::new();

    let limits = super::clamp_resources();
    if limits.enforced(false) {
        protections.limits = true;
    }
    detail.push(format!("limits={}", limits.detail));
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
        assert!(!media.contains("file-write"));
        assert!(!media.contains("network"));
    }

    /// A helper that is a script names its interpreter on its `#!` line, and that
    /// interpreter is the only other program the media profile may start. The
    /// documents profile names neither.
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

        let media = profile_text(Path::new("/opt/nanofile/nanofile"), Profile::Media, grants);
        assert!(
            media.contains("(allow process-exec (literal \"/bin/sh\"))"),
            "{media}"
        );
        assert!(media.contains("(allow file-read* (literal \"/bin/sh\"))"));
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
            !document.contains("/bin/sh"),
            "a documents profile starts nothing: {document}"
        );

        let _ = std::fs::remove_file(&path);
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
