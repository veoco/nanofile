//! The interpreter a script helper's `#!` line names.
//!
//! A helper can be a script, and every platform that runs one has to allow the
//! program the kernel starts before the script's own code: `linux.rs` grants it
//! through Landlock, `seatbelt.rs` through the profile text. The `#!` line is
//! where that path is written down, so it is read rather than guessed at from a
//! list of names.
//!
//! This lives in its own file because the aarch64 syscall check in CI compiles
//! the real `linux.rs` against a hand-written parent module, and copies this file
//! beside it: one implementation, on every platform that runs a helper.

use std::path::{Path, PathBuf};

/// The interpreter the helper names, when it has one.
///
/// Only an absolute path counts, and a bare name is not how the kernel resolves a
/// shebang either — it does not search `PATH`. `#!/usr/bin/env ffmpeg` therefore
/// names `env` and not `ffmpeg`: the second program is one the grant does not
/// reach, which is the caller's to configure away rather than a reason to widen
/// the grant.
pub(crate) fn interpreter(helper: &Path) -> Option<PathBuf> {
    use std::io::Read as _;

    let mut head = [0u8; 256];
    let mut file = std::fs::File::open(helper).ok()?;
    let read = file.read(&mut head).ok()?;
    let line = std::str::from_utf8(&head[..read]).ok()?;
    let rest = line.strip_prefix("#!")?;
    let program = rest
        .split(['\n', ' ', '\t'])
        .find(|token| !token.is_empty())?;
    let path = Path::new(program);
    path.is_absolute().then(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script's own interpreter is read from its first line, and only an
    /// absolute path counts.
    #[test]
    fn a_script_names_its_own_interpreter() {
        let path = std::env::temp_dir().join(format!(
            "nanofile-shebang-{}-{:?}.sh",
            std::process::id(),
            std::thread::current().id()
        ));

        std::fs::write(&path, b"#!/bin/dash\nexit 0\n").expect("write");
        assert_eq!(interpreter(&path), Some(PathBuf::from("/bin/dash")));

        // Whitespace after the marker is the kernel's to skip as much as ours.
        std::fs::write(&path, b"#!  /usr/bin/env sh\n").expect("write");
        assert_eq!(interpreter(&path), Some(PathBuf::from("/usr/bin/env")));

        // A bare name is not a path, and the kernel does not search `PATH` for
        // one either.
        std::fs::write(&path, b"#!dash\nexit 0\n").expect("write");
        assert_eq!(interpreter(&path), None);

        // Nor is an ELF helper a script.
        std::fs::write(&path, b"\x7fELF\x02\x01\x01").expect("write");
        assert_eq!(interpreter(&path), None);

        let _ = std::fs::remove_file(&path);
        assert_eq!(interpreter(Path::new("/nonexistent-helper")), None);
    }
}
