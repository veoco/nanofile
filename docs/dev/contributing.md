# Contributing

## Build and verify

```bash
npm ci
cargo build --bin nanofile
```

The repository's verification commands, in the order CI runs them:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p server --features tray --bin nanofile -- -D warnings
cargo test -p server --features tray --bin nanofile
cargo machete
NANOFILE_TEST_FFMPEG_PATH="$(command -v ffmpeg)" cargo test --workspace --no-fail-fast
node --test "server/frontend/**/*.test.js"
cargo audit --ignore RUSTSEC-2023-0071
```

The browser suite is described in [Testing](/dev/testing). CI is the authority;
it also runs the platform-specific lint and cross-target builds that a single
machine cannot.

## Comment style

Comments earn their place by saying something the code cannot. In particular:

- **Keep** invariants, security rationale, `unsafe` preconditions, concurrency and
  lifecycle reasoning, error/panic contracts, Seafile client quirks, migration and
  wire-format notes, `#[allow(...)]` reasons and doctests.
- **Delete** comments that restate a name or a control flow, historical notes
  ("used to", "before this existed"), step-by-step test narration and decorative
  banners.
- **Condense** a module document to its purpose, its key invariant and a link to
  the relevant page here. Multi-paragraph design notes belong in `docs/dev/`.
- Do not write local paths, author names or dates into comments.

`cargo doc` with `-D warnings` runs in CI, so a broken intra-doc link or a doc
comment that Markdown misparses fails the build. A change that removes a doctest
must keep an equivalent test.

## Dependency rules

The dependency graph is part of the build contract:

- Do not enable the default features of the dev `reqwest` (its TLS pulls in
  `aws-lc-rs`) or of the dev `image` (its formats pull in `rav1e`/`ravif`).
- TLS is used only by `lettre` (SMTP, rustls/ring). `sqlx`/`sea-orm` use
  `runtime-tokio` and only connect to SQLite.
- `ci.yml` fails if `cargo machete` finds an unused direct dependency, or if
  `aws-lc-rs`, `aws-lc-sys`, `rav1e` or `ravif` reappears in the graph.

## Documentation

The site lives in `docs/` and is built with VitePress. Every page has an English
original and a Simplified Chinese mirror under `docs/zh/`; change both in the same
commit.

```bash
npm ci --prefix docs
npm run --prefix docs dev       # http://localhost:5173/nanofile/
npm run --prefix docs build     # fails on a dead link
npm run --prefix docs preview   # http://localhost:4173/nanofile/
```

- `base` is `/nanofile/`; a fork has to change it in
  `docs/.vitepress/config.mts`.
- Site-internal links are `/guide/<chapter>/<page>` (English) or
  `/zh/guide/<chapter>/<page>` (Chinese). The guide is split into chapter
  directories — `start`, `deploy`, `configure`, `admin`, `operate` — so a new
  guide page goes in the matching chapter.
  Link to files outside `docs/` with their GitHub URL, not a relative path.
- Put angle-bracket placeholders such as `<owner>` in backticks; outside code
  fences they are parsed as HTML.
- `docs/public/favicon.svg` is a copy of `server/static/img/favicon.svg`.

## Commit messages

Write the subject in English and keep it imperative. A short subject plus a few
bullets describing what changed is enough. Do not record local test results,
counts or timings in a commit message; those belong in the conversation or the pull
request.
