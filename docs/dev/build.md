# Building

This page is where compiling Nanofile from source lives. Running the server does
not require a build: see [Installing and first run](/guide/start/getting-started).

## Requirements

- **A stable Rust toolchain.** The workspace uses edition 2024.
- **Node.js and npm.** Only to provide `esbuild` and Tailwind, see below. CI uses
  Node 24.
- **Optional: `ffmpeg`.** It only affects media thumbnails; without it, media
  thumbnails are unavailable and the media tests are skipped.
- **Only for a `tray` build:** GTK development libraries on Linux, a resource
  compiler on Windows. See [Optional feature: `tray`](#optional-feature-tray).

## How the frontend reaches the binary

`server/build.rs` does three things before every build:

1. **Tailwind CSS** (optional) — compiles `static/css/input.css` into
   `static/css/app.css`. When no Tailwind is found it writes a placeholder
   stylesheet: the build still succeeds, but the UI is unstyled.
2. **esbuild** (required) — bundles the entry points under `frontend/entries/`
   into `static/js/*.bundle.js`. When no esbuild is found the build **fails**,
   because the scripts inlined in the templates depend on those artifacts.
3. **Tray icons** (only with the `tray` feature) — rasterized from
   `static/img/favicon.svg`, and on Windows embedded as exe resources.

Both tools are looked up in `node_modules/.bin/` of the crate root and the
workspace root first, then on `PATH`. That is all the `npm ci` below is for.

A frontend change needs a `cargo build`; there is no hot reload. The frontend's
own conventions are in [Frontend](/dev/frontend).

## Basic build

```bash
npm ci                            # esbuild and Tailwind
cargo build --release -p server   # the binary is target/release/nanofile
./target/release/nanofile
```

For day-to-day work the debug build is faster:

```bash
cargo build --bin nanofile        # the binary is target/debug/nanofile
```

The two profiles differ at startup: a release build **refuses to start** without
`server.secret_key`, while a debug build generates an ephemeral one, so sessions
do not survive a restart. See [Security](/guide/admin/security).

## Binary size and the performance probe

`[profile.release]` already uses fat LTO, one codegen unit and `strip`.
`panic = "abort"` is deliberately not used because the task executor and the
index extractor rely on catching panics. When a dependency change grows the
binary, check the per-crate split with:

```bash
cargo bloat --release --crates -n 60 -p server --bin nanofile
```

The release binary embeds the web assets and locale tables DEFLATE-compressed
(`include-flate`) and uses a dictionary-free CJK bigram analyzer for content, so
it carries no word list and links no C compression library. Keep the transitive
`include-flate` features at `deflate`: enabling `zstd` compiles `zstd-sys` into
every build, and CI rejects it.

The hot-path probe is `server/tests/perf_probe.rs`. It is `#[ignore]`d, so it
never runs in CI; run it before and after a performance change and diff the
`perf ...` lines (Linux reports the process's `VmRSS`):

```bash
cargo test -p server --release --test perf_probe -- --ignored --nocapture
PERF_FILES=500 cargo test -p server --release --test perf_probe -- --ignored --nocapture
```

## Optional feature: `tray`

`--features tray` builds a binary with a system tray menu:

```bash
cargo build --release -p server --features tray
```

- **Linux** needs `libgtk-3-dev` and `libayatana-appindicator3-dev`.
- **Windows** embeds the icon and version resources with `winresource`, which
  needs `rc.exe` or `llvm-rc`.
- **macOS** needs no extra system dependency; the tray icon is a template image
  the menu bar inverts itself.

GTK cannot be cross-compiled, so only the three native runner platforms ship a
tray variant (Linux amd64, macOS arm64, Windows amd64). What the tray itself does
is in [Desktop tray and Windows service](/guide/deploy/desktop-and-service).

## Cross-compilation

Linux targets are linked with `cargo-zigbuild`:

```bash
cargo zigbuild --release --target aarch64-unknown-linux-musl
```

| Target | Use |
|---|---|
| `x86_64-unknown-linux-gnu` / `-musl` | Linux amd64 |
| `aarch64-unknown-linux-gnu` / `-musl` | Linux arm64 |
| `loongarch64-unknown-linux-gnu` / `-musl` | Linux LoongArch64 |
| `x86_64-pc-windows-msvc` | Windows amd64 |
| `aarch64-apple-darwin` | macOS Apple Silicon |

The toolchain is pinned: `cargo-zigbuild@0.23.4` with zig `0.16.0`, and the
archive is checked against its SHA-256 before it is unpacked — a replaced CDN or
tarball cannot inject a toolchain into the build. Zig 0.15+ raises the baseline
of the binaries it emits to glibc 2.31 / Linux 5.10; keep older hosts working
with a suffix such as `--target aarch64-unknown-linux-gnu.2.28`.

The musl artifacts are statically linked, and they are what the container image
(`FROM scratch`) ships.

## Verification

The commands to run before a change is pushed, and what CI runs in their place,
are in [Contributing](/dev/contributing) and [Testing](/dev/testing).

## Related

- [Architecture](/dev/architecture) — the crate layout and where the build
  script sits in it.
- [Frontend](/dev/frontend) — templates, styles and JavaScript conventions.
- [Testing](/dev/testing) — the Rust, frontend and browser suites.
- [CI and releases](/dev/ci-releases) — per-platform builds, the sandbox smoke
  test and the published artifacts.
- [Installing and first run](/guide/start/getting-started) — running it without
  building.
