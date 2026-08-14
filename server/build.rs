/// build.rs — Nanofile build script
///
/// Two code-generation steps run here:
///   1. Tailwind CSS: `static/css/input.css` → `static/css/app.css`
///      (optional — falls back to a placeholder if Tailwind is unavailable)
///   2. esbuild JS: `frontend/entries/*.js` → `static/js/*.bundle.js`
///      (required — the frontend is written as ES modules and must be bundled
///      into plain IIFE scripts so inline template scripts keep working)
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NANOFILE_BUILD_TS={}", ts);

    // Only regenerate the CSS/JS when one of their inputs changes.
    println!("cargo:rerun-if-changed=static/css/input.css");
    println!("cargo:rerun-if-changed=templates/");
    println!("cargo:rerun-if-changed=frontend/");

    build_css();
    build_js();
}

// ─── Tailwind CSS ────────────────────────────────────────────────────────────

fn build_css() {
    let project_root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let output_path = project_root.join("static/css/app.css");
    let input_path = project_root.join("static/css/input.css");

    let args: [&str; 0] = [];

    if let Some(bin) = which_binary("tailwindcss")
        && run_tailwind(&bin, &args, &input_path, &output_path)
    {
        return;
    }
    if let Some((npx_bin, npx_args)) = which_node_bin("tailwindcss")
        && run_tailwind(npx_bin, &npx_args, &input_path, &output_path)
    {
        return;
    }

    // No Tailwind available — fall back to a placeholder so the build still
    // succeeds with an unstyled UI (matches the documented "optional Tailwind"
    // setup in the README and the module doc comment above).
    eprintln!("warning: Tailwind CSS unavailable; UI will be unstyled.");
    let _ = std::fs::write(
        &output_path,
        "/* Tailwind CSS unavailable — UI rendered unstyled. */\n",
    );
}

fn run_tailwind(cmd: &str, extra_args: &[&str], input: &Path, output: &Path) -> bool {
    let input_s = input.to_string_lossy().to_string();
    let output_s = output.to_string_lossy().to_string();
    let mut args: Vec<&str> = extra_args.to_vec();
    args.push("-i");
    args.push(&input_s);
    args.push("-o");
    args.push(&output_s);
    args.push("--minify");

    match Command::new(cmd).args(&args).status() {
        Ok(s) if s.success() => {
            println!("cargo:info=✓ Tailwind CSS generated ({})", output.display());
            true
        }
        Ok(s) => {
            println!("cargo:warning=⚠ Tailwind CSS failed (exit: {}).", s);
            false
        }
        Err(e) => {
            println!("cargo:warning=⚠ Failed to execute Tailwind: {}.", e);
            false
        }
    }
}

// ─── esbuild JS ──────────────────────────────────────────────────────────────

fn build_js() {
    if let Some(bin) = which_binary("esbuild")
        && run_esbuild(&bin)
    {
        return;
    }
    if let Some((npx_bin, npx_args)) = which_node_bin("esbuild")
        && run_esbuild_with(npx_bin, &npx_args)
    {
        return;
    }

    panic!(
        "esbuild is required to bundle the frontend JS but was not found. \
         Install it with `npm install esbuild` or put an `esbuild` binary on PATH."
    );
}

fn run_esbuild(cmd: &str) -> bool {
    run_esbuild_with(cmd, &[])
}

fn run_esbuild_with(cmd: &str, extra_args: &[&str]) -> bool {
    let mut args: Vec<&str> = extra_args.to_vec();
    args.push("frontend/entries/common.js");
    args.push("frontend/entries/file-browser.js");
    args.push("--bundle");
    args.push("--minify");
    args.push("--format=iife");
    args.push("--outdir=static/js");
    args.push("--out-extension:.js=.bundle.js");

    match Command::new(cmd).args(&args).status() {
        Ok(s) if s.success() => {
            println!("cargo:info=✓ esbuild bundled frontend JS");
            true
        }
        Ok(s) => {
            println!("cargo:warning=⚠ esbuild failed (exit: {}).", s);
            false
        }
        Err(e) => {
            println!("cargo:warning=⚠ Failed to execute esbuild: {}.", e);
            false
        }
    }
}

// ─── Binary discovery ────────────────────────────────────────────────────────

/// Try a standalone binary named `name` (project root, then PATH).
fn which_binary(name: &str) -> Option<String> {
    let project_root = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let project_root_path = Path::new(&project_root);

    let local = project_root_path.join(name);
    if local.exists() {
        return Some(local.to_string_lossy().to_string());
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.exists() {
                Some(full.to_string_lossy().to_string())
            } else {
                let full_exe = dir.join(format!("{}.exe", name));
                if full_exe.exists() {
                    Some(full_exe.to_string_lossy().to_string())
                } else {
                    None
                }
            }
        })
    })
}

/// Try `node_modules/.bin/{name}` (installed via npm). Checks the crate root
/// first, then the workspace root (one level up).
fn which_node_bin(name: &str) -> Option<(&'static str, Vec<&'static str>)> {
    let crate_root = std::env::var("CARGO_MANIFEST_DIR").ok()?;

    for base in [
        Path::new(&crate_root).to_path_buf(),
        Path::new(&crate_root).parent()?.to_path_buf(),
    ] {
        let bin = if cfg!(windows) {
            base.join(format!("node_modules/.bin/{}.cmd", name))
        } else {
            base.join(format!("node_modules/.bin/{}", name))
        };
        if bin.exists() {
            return Some((bin.to_string_lossy().to_string().leak(), vec![]));
        }
    }
    None
}
