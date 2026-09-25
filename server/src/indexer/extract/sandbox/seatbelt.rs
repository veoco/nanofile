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
/// * `process-fork` — the parsers run their work on a thread, which Seatbelt
///   charges to `process-fork`. `process-exec` is deliberately *not* allowed,
///   so no other program can be started.
/// * `signal (target self)` — the runtime's own bookkeeping.
/// * `sysctl-read` — the allocator and the runtime read a few sysctl values
///   while starting up.
/// * `file-read*` under the system library paths and for this binary: macOS
///   binds symbols lazily, so the first call into a symbol that is not resolved
///   yet sends dyld back to the library it lives in. Without this the child
///   dies on its first allocation, which reads as "the sandbox broke parsing"
///   rather than "the profile is too tight".
/// * `/dev/null` and `/dev/urandom`: the runtime keeps the first for the
///   standard streams the parent set up, and `getentropy` may fall back to the
///   second.
///
/// Everything else — every write, every socket, every other path — is denied.
pub(super) fn profile(exe: &Path) -> String {
    format!(
        "(version 1) (deny default) (allow process-fork) (allow signal (target self)) \
         (allow sysctl-read) (allow file-read* (subpath \"/usr/lib\") \
         (subpath \"/System/Library\") (literal \"/dev/null\") (literal \"/dev/urandom\") \
         (literal \"{}\"))",
        escape(&exe.to_string_lossy())
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
        assert!(profile.contains("(allow file-read* (subpath \"/usr/lib\")"));
        assert!(profile.contains("(literal \"/opt/nanofile/nanofile\")"));
        // Nothing may grant a write, a socket, or another program.
        for forbidden in [
            "file-write", "network", "process-exec", "mach-lookup", "iokit",
        ] {
            assert!(
                !profile.contains(forbidden),
                "the profile must not mention {forbidden}: {profile}"
            );
        }
    }

    /// A path is data, never policy text: every quote it brings is escaped, so
    /// it stays inside the literal it was written into.
    #[test]
    fn a_path_cannot_rewrite_the_profile() {
        let profile = profile(Path::new("/tmp/\") (allow file-write*) \"/"));
        let escaped = profile.matches("\\\"").count();
        assert_eq!(escaped, 2, "the path's quotes are escaped: {profile}");
        assert_eq!(
            profile.matches('"').count() - escaped,
            10,
            "only the profile's own delimiters are unescaped: {profile}"
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
