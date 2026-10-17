//! The interpreter a script helper's `#!` line names.
//!
//! A helper can be a script, so the program the kernel starts before its own code has
//! to be granted: `linux.rs` does it through Landlock, `seatbelt.rs` through the
//! profile text. The `#!` line is read rather than guessed at from a list of names.
//!
//! It lives in its own file because the aarch64 CI check compiles the real `linux.rs`
//! against a hand-written parent module and copies this file beside it.

use std::path::{Path, PathBuf};

/// The interpreter the helper names, when it has one.
///
/// Only an absolute path counts: the kernel does not search `PATH` for a shebang.
/// `#!/usr/bin/env ffmpeg` names `env`, not `ffmpeg`, and the second program is the
/// caller's to configure away rather than a reason to widen the grant.
pub(crate) fn interpreter(helper: &Path) -> Option<PathBuf> {
    use std::io::Read as _;

    let mut head = [0u8; 256];
    let mut file = std::fs::File::open(helper).ok()?;
    let read = file.read(&mut head).ok()?;
    let line = std::str::from_utf8(&head[..read]).ok()?;
    let rest = line.strip_prefix("#!")?;
    // The kernel's `isspace` set, so a CRLF after the path does not become part of it:
    // a grant for `/bin/sh\r` would name nothing and look like a refusal.
    let program = rest
        .split(['\n', '\r', '\t', '\x0b', '\x0c', ' '])
        .find(|token| !token.is_empty())?;
    let path = Path::new(program);
    path.is_absolute().then(|| path.to_path_buf())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A script's interpreter is read from its first line, and only an absolute path
    /// counts. Unix-only: `is_absolute` on Windows answers false for `/bin/dash`, and
    /// the reader is exercised there by the seatbelt tests.
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
