/// build.rs — Nanofile build script
///
/// Three code-generation steps run here:
///   1. Tailwind CSS: `static/css/input.css` → `static/css/app.css`
///      (optional — falls back to a placeholder if Tailwind is unavailable)
///   2. esbuild JS: `frontend/entries/*.js` → `static/js/*.bundle.js`
///      (required — the frontend is written as ES modules and must be bundled
///      into plain IIFE scripts so inline template scripts keep working)
///   3. Tray icons (`tray` feature only): `static/img/favicon.svg` →
///      `$OUT_DIR/tray_icon.rgba` (+ `$OUT_DIR/nanofile.ico` and version
///      resources on Windows) — see the "Tray icons" section below
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NANOFILE_BUILD_TS={}", ts);

    // Only regenerate the CSS/JS when one of their inputs changes.
    // src/ is included because Tailwind v4 scans Rust string literals for
    // class names (e.g. file_icon_color) — without it a class change there
    // would leave app.css stale until an unrelated rebuild.
    println!("cargo:rerun-if-changed=static/css/input.css");
    println!("cargo:rerun-if-changed=templates/");
    println!("cargo:rerun-if-changed=frontend/");
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=static/img/favicon.svg");

    build_css();
    build_js();
    build_tray_icons();
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
    args.push("frontend/entries/public-upload.js");
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

// ─── Tray icons (`tray` feature only) ────────────────────────────────────────
//
// The repository ships no binary icon assets: everything the tray and the
// Windows exe need is rasterized here from `static/img/favicon.svg`:
//   - `$OUT_DIR/tray_icon.rgba` — straight-alpha 32×32 pixels, included by
//     `src/tray/icon.rs` via `include_bytes!`
//   - `$OUT_DIR/nanofile.ico` — multi-size Windows exe icon (DIB entries),
//     embedded together with version info via winresource
//
// Rendering the "N" glyph needs a font; system fonts are used and the icon
// degrades to the plain rounded square when none are installed.

#[cfg(feature = "tray")]
#[path = "src/tray/icon_gen.rs"]
mod icon_gen;

#[cfg(feature = "tray")]
fn build_tray_icons() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let svg = std::fs::read(manifest_dir.join("static/img/favicon.svg"))
        .expect("tray feature: failed to read static/img/favicon.svg");

    let rgba = rasterize_svg(&svg, icon_gen::TRAY_ICON_SIZE);
    std::fs::write(out_dir.join("tray_icon.rgba"), &rgba)
        .expect("tray: failed to write tray_icon.rgba");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut images = Vec::new();
        for &size in &icon_gen::EXE_ICON_SIZES {
            let img = rasterize_svg(&svg, size);
            images.push((size, icon_gen::dib_from_rgba(size, &img)));
        }
        let ico = icon_gen::build_ico(&images);
        std::fs::write(out_dir.join("nanofile.ico"), ico)
            .expect("tray: failed to write nanofile.ico");

        let mut res = winresource::WindowsResource::new();
        res.set_icon(out_dir.join("nanofile.ico").to_str().unwrap());
        res.set("FileDescription", "Nanofile server");
        res.set("ProductName", "Nanofile");
        res.set("OriginalFilename", "nanofile.exe");
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        res.set("FileVersion", &version);
        res.set("ProductVersion", &version);
        res.compile()
            .expect("tray: failed to embed Windows resources (needs rc.exe or llvm-rc)");
    }
}

#[cfg(not(feature = "tray"))]
fn build_tray_icons() {}

/// Rasterizes an SVG into a `size × size` straight-alpha RGBA buffer, fitting
/// the whole SVG centered into the square.
#[cfg(feature = "tray")]
fn rasterize_svg(svg: &[u8], size: u32) -> Vec<u8> {
    let mut options = resvg::usvg::Options::default();
    let mut fontdb = resvg::usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    // Generic-font fallback for the "N" glyph when the SVG's requested font
    // family (Arial) is not installed on the build machine.
    fontdb.set_sans_serif_family("DejaVu Sans");
    options.fontdb = std::sync::Arc::new(fontdb);

    let tree = resvg::usvg::Tree::from_data(svg, &options)
        .expect("tray: failed to parse static/img/favicon.svg");
    let svg_size = tree.size();
    let scale = (size as f32 / svg_size.width()).min(size as f32 / svg_size.height());
    let dx = (size as f32 - svg_size.width() * scale) / 2.0;
    let dy = (size as f32 - svg_size.height() * scale) / 2.0;

    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("tray: failed to allocate pixmap");
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy),
        &mut pixmap.as_mut(),
    );

    // tiny-skia stores premultiplied alpha; tray consumers want straight alpha.
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}
