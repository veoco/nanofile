# CI and releases

Three workflows build and publish the project, and one deploys this site.

## `ci.yml`

Runs on every push and pull request to `main`, `master` or `develop`, and can be
started manually with "Run workflow". A paths filter decides which jobs run:

- **`lint`** (Linux) — `cargo fmt --check`, clippy for the whole workspace, the
  dependency guards, rustdoc and `cargo audit`.
- **`lint-platforms`** (macOS, Windows) — clippy and rustdoc for the `cfg`-gated code that the
  Linux runner never compiles.
- **`tray-check`** (Linux, macOS, Windows) — clippy, rustdoc and unit tests for the optional
  tray feature.
- **`tests`** (Linux x86_64 and arm64, macOS arm64, Windows amd64) — the whole
  Rust suite.
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
- `aws-lc-rs`, `aws-lc-sys`, `rav1e` or `ravif` re-enters the graph. Those arrive
  through the default features of `reqwest`'s TLS and `image`'s formats, which is
  why the dev-dependencies in `server/Cargo.toml` disable them.

`cargo audit` ignores `RUSTSEC-2023-0071` (the Marvin attack in `rsa`, pulled in by
`jsonwebtoken`'s rust_crypto feature). Nanofile only signs and verifies HS256
tokens, so the advisory is unreachable; the ignore is re-checked when
`jsonwebtoken` gains algorithm-level features.

## `nightly.yml`

Runs daily at 10:00 UTC. A scheduled run first checks whether the latest commit is
from today; a manual run skips that check. It builds the multi-architecture image
and publishes it as `ghcr.io/veoco/nanofile:edge`, and smoke-tests the sandbox of
every native artifact and of the published image (`extract-worker --probe`).

## `release.yml`

Runs on a `v*.*.*` tag, or manually with a version. It builds every platform and
architecture, runs the same sandbox probe against the released artifacts, builds
the multi-architecture container image and publishes:

- versioned images (`v<version>`, `v<major>.<minor>`, `latest` on the default
  branch),
- a GitHub release with the archives and release notes.

The release archive includes the README.

## `docs.yml`

Builds this VitePress site on every change under `docs/` and deploys it to GitHub
Pages on `main`. Pull requests build but do not deploy, so a dead link fails the
PR. See [Contributing](/dev/contributing) for the local commands.

## Related

- [Testing](/dev/testing) — what each job runs.
- [Contributing](/dev/contributing) — the local equivalents.
