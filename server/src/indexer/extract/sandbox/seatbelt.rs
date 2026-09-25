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

#[cfg(target_os = "macos")]
use super::Layers;
use std::path::Path;

/// Hardcoded path to the Seatbelt runner.
pub(super) const PROGRAM: &str = "/usr/bin/sandbox-exec";

/// Arguments that wrap `exe` in [`profile`], with `--` separating them from the
/// command.
pub(super) fn args(exe: &Path) -> Vec<String> {
    vec!["-p".to_string(), profile(exe), "--".to_string()]
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
/// measurement of the files layer checks.
pub(super) fn profile(exe: &Path) -> String {
    let exe = escape(&exe.to_string_lossy());
    // The dyld shared cache lives in the cryptex on Apple Silicon; on Intel
    // that path does not exist and the rule grants nothing.
    format!(
        "(version 1) (deny default) (allow process-exec (literal \"{exe}\")) \
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

/// Clamp the child's own resources. The rest of the confinement comes from the
/// runner.
#[cfg(target_os = "macos")]
pub(super) fn confine() -> (Layers, Vec<String>) {
    let mut layers = Layers::default();
    let mut detail = Vec::new();
    if super::clamp_resources() {
        layers.limits = true;
        detail.push(super::limits_detail());
    } else {
        detail.push("limits=failed".to_string());
    }
    (layers, detail)
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
        let profile = profile(Path::new("/opt/nanofile/nanofile"));
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
        let clean = profile(Path::new("/opt/nanofile/nanofile"));
        let injected = profile(Path::new("/tmp/\") (allow file-write*) \"/"));

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

    /// The runner is what confines files, the network and process creation, so
    /// the child must be told that those layers are not its own to establish.
    #[test]
    fn the_arguments_wrap_the_child_in_the_profile() {
        let args = args(Path::new("/opt/nanofile/nanofile"));
        assert_eq!(args[0], "-p");
        assert_eq!(args[2], "--");
        assert!(args[1].starts_with("(version 1)"));
    }
}
