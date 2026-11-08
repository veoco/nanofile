# Testing

## Rust

```bash
cargo test --workspace
```

CI runs the same suite under `cargo-nextest`
(`cargo nextest run --workspace --profile ci`): every test gets its own process,
so one test's process-global sandbox state cannot reach another, and a timing
flake is retried alone instead of failing the row. nextest does not run
doctests, so CI runs those separately with `cargo test --workspace --doc`. The
OCR cases are the slowest in the suite and run in their own `ocr-tests` job on
linux-amd64 and macos-arm64, in parallel with the main rows. The command above
stays the local equivalent.

This runs the unit tests, the integration tests under `server/tests/` and the
documentation tests. The workspace suite is the main gate. `server/tests/docs_api_reference_test.rs`
keeps the API reference tied to the route table and the explicit anchors it links
to; see [API reference](/reference/api/). The docs workflow also runs
`npm run --prefix docs check`, which compares the English and Chinese page sets, the
guide heading levels, internal links and anchors, and the sidebar coverage; see
[Documentation](/dev/documentation).

The optional tray feature is compiled out by default, so its unit tests need the
feature explicitly:

```bash
cargo test -p server --features tray --bin nanofile
```

Automatic people tags and the embedded YuNet model are in the default test set;
run them on their own with:

```bash
cargo test -p server --test face_tags_model_test --test face_tags_test
```

## Frontend

```bash
node --test "server/frontend/**/*.test.js"
```

## Browser end-to-end

The e2e runner drives a real server in a browser. Build the debug binary first,
because the tests start it:

```bash
npm ci
cargo build --bin nanofile
npm ci --prefix e2e
npx --prefix e2e playwright install chromium
npm run --prefix e2e test
```

`e2e/global-setup.ts` starts the server; `e2e/global-teardown.ts` stops it. On a
fresh machine, the browser's system libraries may need
`npx --prefix e2e playwright install-deps chromium`.

## Media tests and ffmpeg

The media tests need a real `ffmpeg`. Point the suite at one with
`NANOFILE_TEST_FFMPEG_PATH`:

```bash
NANOFILE_TEST_FFMPEG_PATH="$(command -v ffmpeg)" cargo test --workspace --no-fail-fast
```

Without ffmpeg the media tests are skipped. CI installs ffmpeg on every runner, so
the media assertions are measured there rather than silently skipped.

## Sandbox probe

`server/tests/sandbox_probe_test.rs` asserts each sandbox profile's typed report,
the platform's mechanism tokens, the parsers' products, the media pipeline and the
detector's forward pass end to end. See [Sandbox internals](/dev/sandbox) for what
it checks.

## Performance probe

`server/tests/perf_probe.rs` measures the block-existence cache, CDC throughput,
PBKDF2 and an HTTP upload/list/history scenario. Every case is `#[ignore]`d so the
suite stays deterministic; run it in release mode and diff the `perf ...` lines:

```bash
cargo test -p server --release --test perf_probe -- --ignored --nocapture
```

See [Building](/dev/build#binary-size-and-the-performance-probe) for the release
profile notes and the `cargo bloat` command.

## What CI runs

| Job | Platforms | What it does |
|---|---|---|
| `lint` | Linux | `cargo fmt --check`, clippy, rustdoc, dependency guards, audit |
| `lint-platforms` | macOS, Windows | clippy and rustdoc for the platform-specific `cfg` code |
| `tray-check` | Linux, macOS, Windows | clippy, rustdoc and unit tests for the `tray` feature |
| `tests` | Linux x86_64/arm64, macOS arm64, Windows amd64 | the whole Rust suite |
| `cross-check` | Linux | compiles and lints four targets with no runner |
| `frontend-test` | Linux | `node --test` |
| `e2e` | Linux | Playwright |
| `audit` | Linux | `cargo audit` |

The full definitions are in
[`ci.yml`](https://github.com/veoco/nanofile/blob/main/.github/workflows/ci.yml).
See [CI and releases](/dev/ci-releases) for the workflow layout.
