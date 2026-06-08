/// build.rs — Nanofile build script
///
/// Generates `static/css/app.css` from `static/css/input.css` via Tailwind.
/// Tries, in order:
///   1. Standalone `tailwindcss` binary (project root or PATH)
///   2. `npx @tailwindcss/cli` (Node.js npx, available on most systems)
///   3. Writes a minimal placeholder (app compiles but UI is unstyled)
use std::io::Write;
use std::path::Path;
use std::process::Command;

fn main() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NANOFILE_BUILD_TS={}", ts);

    let project_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let output_path = Path::new(&project_root).join("static/css/app.css");
    let input_path = Path::new(&project_root).join("static/css/input.css");

    let args: [&str; 0] = [];

    // Try standalone binary, then npx, then placeholder
    if let Some(bin) = which_tailwind()
        && run_tailwind(&bin, &args, &input_path, &output_path)
    {
        return;
    }

    if let Some((npx_bin, npx_args)) = which_npx()
        && run_tailwind(npx_bin, &npx_args, &input_path, &output_path)
    {
        return;
    }

    println!("cargo:warning=⚠ No Tailwind CLI found. Using placeholder CSS.");
    write_placeholder(&output_path);
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

fn write_placeholder(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            println!("cargo:warning=⚠ Failed to create placeholder CSS: {e}");
            return;
        }
    };
    let _ = f.write_all(b"/* placeholder - install Tailwind CLI for full styles */\n");
}

/// Try standalone `tailwindcss` binary (project root or PATH).
fn which_tailwind() -> Option<String> {
    let project_root = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let project_root_path = std::path::Path::new(&project_root);

    // Check project root first
    let local = project_root_path.join("tailwindcss");
    if local.exists() {
        return Some(local.to_string_lossy().to_string());
    }

    // Check PATH
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join("tailwindcss");
            if full.exists() {
                Some(full.to_string_lossy().to_string())
            } else {
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

/// Try `npx @tailwindcss/cli` (Node.js package).
fn which_npx() -> Option<(&'static str, Vec<&'static str>)> {
    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };

    // Check if npx is on PATH
    let has_npx = std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(npx).exists()));
    if !has_npx {
        return None;
    }

    Some(("npx", vec!["--yes", "@tailwindcss/cli"]))
}
