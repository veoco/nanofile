/// build.rs — Nanofile build script
///
/// Integrates Tailwind CSS generation into the `cargo build` workflow:
/// - If `tailwindcss` CLI is found in PATH or project root, generates CSS
/// - If not found, emits a warning and continues (graceful degradation)
/// - **Always runs** on every build (no `rerun-if-changed` directives) so
///   that newly added template classes are never missed by Tailwind's JIT.
use std::process::Command;

fn main() {
    // NOTE: Intentionally omitting rerun-if-changed directives.
    // Without them Cargo always re-runs the build script, which guarantees
    // Tailwind JIT picks up any classes added in new or modified templates.
    // The cost is ~60ms per build — well worth the correctness.

    // Emit a build timestamp so the app can use it for cache busting.
    // Use CARGO_PKG_VERSION as a fallback; the timestamp changes every build.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NANOFILE_BUILD_TS={}", ts);

    // Try to locate the Tailwind standalone CLI binary
    let tailwind = which_tailwind();

    match tailwind {
        Some(bin) => {
            let input = "static/css/input.css";
            let output = "static/css/app.css";

            let status = Command::new(&bin)
                .args(["-i", input, "-o", output, "--minify"])
                .status();

            match status {
                Ok(s) if s.success() => {
                    println!("cargo:info=✓ Tailwind CSS generated ({})", output);
                }
                Ok(s) => {
                    println!(
                        "cargo:warning=⚠ Tailwind CSS generation failed (exit: {}). CSS may be stale.",
                        s
                    );
                }
                Err(e) => {
                    println!(
                        "cargo:warning=⚠ Failed to execute tailwind CLI: {}. CSS may be stale.",
                        e
                    );
                }
            }
        }
        None => {
            println!(
                "cargo:warning=⚠ Tailwind CLI not found. Install it or run manually:\n  \
                 curl -sL https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-macos-arm64 \
                 -o tailwindcss && chmod +x tailwindcss\n  \
                 ./tailwindcss -i static/css/input.css -o static/css/app.css"
            );
        }
    }
}

/// Look for `tailwindcss` in PATH, then in the project root directory.
///
/// NOTE: During `cargo build`, `std::env::current_dir()` returns the build
/// script's temporary directory (e.g. `target/debug/build/nanofile-xxx/`),
/// NOT the project root.  Use `CARGO_MANIFEST_DIR` instead, which Cargo
/// always sets to the directory containing `Cargo.toml`.
fn which_tailwind() -> Option<String> {
    let project_root = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let project_root_path = std::path::Path::new(&project_root);

    // Check project root first (allows project-local installation)
    let local = project_root_path
        .join("tailwindcss")
        .exists()
        .then(|| project_root_path.join("tailwindcss").to_string_lossy().to_string());

    if local.is_some() {
        return local;
    }

    // Check PATH via `which tailwindcss`
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join("tailwindcss");
            if full.exists() {
                Some(full.to_string_lossy().to_string())
            } else {
                // On Windows, also check for tailwindcss.exe
                let full_exe = dir.join("tailwindcss.exe");
                if full_exe.exists() {
                    Some(full_exe.to_string_lossy().to_string())
                } else {
                    None
                }
            }
        })
    })
}
