# CI and releases

Three workflows build and publish the project, and one deploys this site.

## `ci.yml`

Runs on every push and pull request to `main`, `master` or `develop`, and can be
started manually with "Run workflow". A paths filter decides which jobs run:

- **`lint`** (Linux) — `cargo fmt --check`, clippy for the whole workspace, rustdoc
  and the dependency guards.
- **`comment-hygiene`** (Linux) — `node tools/check-comments.mjs` for misplaced
  module documents and mojibake.
- **`lint-platforms`** (macOS, Windows) — clippy and rustdoc for the `cfg`-gated code that the
  Linux runner never compiles.
- **`tray-check`** (Linux, macOS, Windows) — clippy and unit tests for the optional
  tray feature.
- **`tests`** (Linux x86_64 and arm64, macOS arm64, Windows amd64) — the Rust
  suite minus OCR.
- **`ocr-tests`** (Linux amd64, macOS arm64, Windows amd64) — the OCR cases, in
  parallel with the main rows.
- **`cross-check`** (Linux) — compiles and lints the four Linux targets with no
  runner (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
  `loongarch64-unknown-linux-gnu`, `loongarch64-unknown-linux-musl`).
- **`frontend-test`** — `node --test`.
- **`e2e`** — Playwright against the debug binary.
- **`audit`** — `cargo audit`.

A force-push or a manual dispatch runs every job regardless of the filter, because
the diff against the previous commit is not trustworthy after a rewrite.

### Pinned actions

Actions are pinned to full commit SHAs, with the tag in a trailing comment, so a
moved or hijacked tag cannot inject code into CI. Dependabot is deliberately not
enabled: refresh a pin by resolving the new tag and updating every occurrence.

### Dependency guards

The `lint` job fails when:

- a direct dependency is unused (`cargo machete`), or
- `aws-lc-rs`, `aws-lc-sys`, `rav1e`, `ravif`, `zstd-sys`, `jieba-rs`, `tract`
  (and its `tract-*` crates), `ort`/`ort-sys`, `onnxruntime`/`onnxruntime-sys`,
  `openblas-src` or `accelerate-src` re-enters the graph. AWS-LC arrives through
  `reqwest`'s default TLS, ravif/rav1e through `image`'s default formats (which is
  why the dev-dependencies in `server/Cargo.toml` disable them), and `zstd-sys`
  through `include-flate`'s default features. The others are the model runtimes,
  segmenter and BLAS libraries the in-tree inference runtime replaces.

`cargo audit` fails on vulnerabilities only; unmaintained-crate advisories are
reported as warnings. One advisory is explicitly ignored in `.cargo/audit.toml`:
`RUSTSEC-2026-0592`, a stack overflow in `url`'s `Url::origin()` that the server
never calls, with the patch held back because it changes how `sqlite://` URLs
with Windows paths are parsed.

### Build caches

Rust builds are shared through `Swatinem/rust-cache` under three `shared-key`
families: `nanofile-v2` holds check-only output (clippy and rustdoc, nothing
linked), `nanofile-build-v2` holds linked dev/test codegen, and `nanofile-cross`
holds the cross-target check units that `cross-check` writes. A cache entry is
immutable — the first job to save a key defines it for every later run — so a job
has to write the family that matches its artifacts. A check-only entry under a
build job's key leaves that job recompiling the whole dependency graph on every
run.

## `nightly.yml`

Runs daily at 10:00 UTC. A scheduled run first checks whether the latest commit is
from today; a manual run skips that check. It builds the multi-architecture image
and publishes it as `ghcr.io/veoco/nanofile:edge`, and smoke-tests the sandbox of
every native artifact and of the published image (`extract-worker --probe`).

## `release.yml`

Runs on a `v*.*.*` tag, or manually with a version. It builds every platform and
architecture, runs the same sandbox probe against the natively built artifacts,
builds the multi-architecture container image and publishes:

- versioned images (`v<version>`, `v<major>.<minor>`, `latest` on the default
  branch),
- a GitHub release with the archives and release notes.

The cross-built binaries (musl, arm64 and loong64) are not executed on the
runner, and the image probe runs on the runner's own architecture only. The
release archive includes the README.

## `docs.yml`

Builds this VitePress site on every change under `docs/` and deploys it to GitHub
Pages on `main`. Pull requests build but do not deploy, so a dead link fails the
PR. The `check` job runs the guide checker and the Rust API reference test on the
same change, so a docs-only PR cannot skip them. See
[Contributing](/dev/contributing) for the local commands.

## Related

- [Testing](/dev/testing) — what each job runs.
- [Contributing](/dev/contributing) — the local equivalents.
