# Testing

## Rust

```bash
cargo test --workspace
```

This runs the unit tests, the integration tests under `server/tests/` and the
documentation tests. The workspace suite is the main gate.

The optional tray feature is compiled out by default, so its unit tests need the
feature explicitly:

```bash
cargo test -p server --features tray --bin nanofile
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
the platform's mechanism tokens, the parsers' products and the media pipeline end
to end. See [Sandbox internals](/dev/sandbox) for what it checks.

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
