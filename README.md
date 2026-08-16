# Nanofile

A wire-compatible [Seafile](https://www.seafile.com/) server written in Rust.

Nanofile speaks the Seafile sync protocol and REST APIs, so official Seafile desktop / mobile
clients and tools like `seaf-cli` can point at it directly. It also ships its own web UI
(file browser, sharing, admin panel) as a single static binary — no separate `seaf-server` +
`seahub` stack to install.

## Features

- **Seafile sync protocol** (`/seafhttp/`, protocol version 2): content-addressed commits and FS
  objects, block transfer with SHA-1 verification, packed FS objects, `check-fs` / `check-blocks`,
  quota & permission pre-checks, per-repo sync tokens, file locking.
- **REST API**: legacy v1 (`/api2/*`) and v2.1 (`/api/v2.1/*`) surfaces covering libraries, files,
  directories, sharing, activities, search, trash, devices, avatars and more — compatible with the
  official mobile apps.
- **WebDAV** (`/dav/...`) with per-library keys, gated by `webdav_enabled`.
- **Web UI**: file browser with previews and thumbnails, starred files, activity feed, trash,
  settings (profile, devices, 2FA, invitations), and a **sysadmin panel** (users, shares,
  background tasks). Localized in English and Chinese.
- **Sharing**: share links (optional password / expiry / view counting), anonymous upload links,
  user shares with rw/r permissions, custom share permissions.
- **Security**: TOTP two-factor auth with backup codes and trusted devices, SSO / "view on website"
  login, invitation-code registration, login rate limiting with lockout, password reset (email-gated),
  hashed session cookies with CSRF protection, path-traversal-safe filename handling.
- **Encrypted libraries**: AES-256-CBC blocks with Seafile-compatible `magic` / `random_key`,
  in-memory password cache with TTL.
- **Storage & versioning**: per-user quotas, content-addressed block store, full history with
  revision browse / restore, per-repo history limits and TTL, garbage collection (history pruning +
  unreachable FS-object cleanup), trash with revert, deleted-library restore.
- **Full-text search**: built-in Tantivy index with a jieba Chinese tokenizer; filename and content
  search across libraries.
- **Real-time notifications**: WebSocket push for repo updates, file locks, folder permissions and
  comment updates.
- **Ops**: resumable / chunked uploads (`Content-Range` assembly), zip batch downloads, background
  scheduler with metrics and manual triggers from the admin UI.

## Architecture

Nanofile is a Cargo workspace of four crates:

| Crate | Role |
|-------|------|
| `base` | Pure base types — `AppError`, path/filename sanitization, Seafile storage-format types and constants. No HTTP dependency unless the `with-axum` feature is enabled. |
| `infra` | Infrastructure — SeaORM entities, content-addressed block storage backend, crypto (AES / key derivation / magic), config + env-var overrides, rate limiting, DB setup. |
| `server` | The application — HTTP handlers, services, repositories, sync protocol, WebDAV, WebSocket notifications, full-text indexer, Askama web UI. |
| `migration` | SeaORM migrations (schema evolution from first launch). |

Dependency direction: `base → infra → server` (compile-time enforced); `migration` is used by `server`.

### Web frontend

The UI is server-rendered (Askama) with Tailwind CSS and a modular JavaScript frontend written as
ES modules:

```
server/frontend/
├── core/       # pure functions (i18n, formatting, file-meta, API helpers) — no DOM, unit-testable
├── browser/    # DOM layer (list, selection, right-panel, operations, upload, view …)
├── entries/    # esbuild entry points (common.js, file-browser.js)
```

`server/build.rs` bundles the `entries/` into `static/js/*.bundle.js` (esbuild) and compiles
`static/css/input.css` into `app.css` (Tailwind), then `rust-embed` embeds both into the binary.
esbuild is **required**; Tailwind is optional (see [Development](#development)).

## Quick Start

```bash
# 1. Install frontend build dependencies — esbuild is required; Tailwind is
#    optional but recommended (without it the UI renders unstyled)
npm install

# 2. Build the server (binary name is `nanofile`, not `server`)
cargo build --release -p server

# 3. Configure
cp config.toml .   # edit to suit — see Configuration below

# 4. Run
./target/release/nanofile
```

Open `http://localhost:8082` and log in.

An admin account is needed. Either auto-create one on first startup via `[admin_init]` in
`config.toml` (or `NANOFILE_ADMIN_INIT_EMAIL` / `NANOFILE_ADMIN_INIT_PASSWORD_FILE`), or create one
with the CLI:

```bash
./target/release/nanofile adduser --email admin@example.com --password 'secret123'
```

Pass `--regular` to create a non-admin account.

## Configuration

Settings are read from `config.toml` in the working directory. Override the path with
`--config <path>` (highest priority) or the `NANOFILE_CONFIG` environment variable. If the file is
missing, the server falls back to built-in defaults, so it can start with zero config — supply
whatever you need via `NANOFILE_*` environment variables. Every key can also be overridden with
a `NANOFILE_*` environment variable — the shipped `config.toml` lists the exact variable name in a
comment above each key (e.g. `NANOFILE_DATABASE_URL`, `NANOFILE_SERVER_PORT`). Environment variables
always win at runtime and are never written into the file.

On upgrade to a newer release, `config.toml` is automatically migrated in place (comments preserved)
when the config format changed, backed up as `config.toml.bak` first; on a read-only mount the
migration is applied in memory only.

| Section | Purpose |
|---------|---------|
| `[server]` | Bind address/port, `site_url` (external URL used for download/share links and cookies — set to your HTTPS domain behind a TLS proxy), max upload size, request timeout, CORS, WebDAV switch, feature switches (`sso_enabled`, `file_search_enabled`), desktop-client branding (`desktop_custom_brand` / `desktop_custom_logo`), trusted reverse proxies. |
| `[database]` | SeaORM/SQLite connection URL (default `sqlite:data/nanofile.db?mode=rwc`) and pool size. |
| `[storage]` | Block store and temp directories, global storage quota cap (`max_storage_bytes`, `0` = unlimited), ffmpeg path for video thumbnails. |
| `[auth]` | Password hashing cost, token TTLs, login lockout, invitation registration, password policy, rate limits. |
| `[ui]` | Default UI language (`en` / `zh`). |
| `[email]` | Master switch for the email backend. Password-reset links are only delivered to the owner's inbox and are never echoed back by the server, so the reset flow stays disabled until an SMTP backend exists. |
| `[admin_init]` | Optional first-start admin auto-creation. Prefer `NANOFILE_ADMIN_INIT_PASSWORD_FILE` for the password. |
| `[logging]` | Log level. |
| `[gc]` | Enable / schedule garbage collection. |
| `[index]` | Full-text search switch (`enabled`) and index directory. |
| `[notification]` | WebSocket notification settings and JWT private key. |

`secret_key` is the single master key: the notification key and CSRF signing key are derived from it.
Generate a unique one for production with `openssl rand -hex 32` and set it via
`NANOFILE_SERVER_SECRET_KEY` (an empty value auto-generates a random key on startup, which invalidates
sessions on restart).

## Docker

The release image is a `scratch` container holding only the `nanofile` binary — no config file or
data directory. Mount a config file and a persistent data volume, and point the data paths at the
volume:

```bash
mkdir -p data
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -v "$PWD/config.toml:/etc/nanofile/config.toml:ro" \
  -e NANOFILE_CONFIG=/etc/nanofile/config.toml \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_INDEX_INDEX_DIR=/data/index \
  -e NANOFILE_SERVER_SECRET_KEY="$(openssl rand -hex 32)" \
  ghcr.io/<owner>/nanofile:latest
```

Or with no config file at all — built-in defaults fill the rest, everything else comes from
environment variables:

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(openssl rand -hex 32)" \
  ghcr.io/<owner>/nanofile:latest
```

## CLI

```
nanofile [--config <path>]           Start the server (default)
nanofile [--config <path>] adduser   Create a user (admin by default; --regular for a normal user)
```

## Data Layout

All state lives under the working directory (defaults shown):

```
data/
├── nanofile.db        # SQLite database (WAL mode, file mode 0600)
├── nanofile.db-wal    # WAL journal
├── blocks/            # content-addressed block store: {2-hex prefix}/{40-hex SHA-1}
├── temp/              # resumable / chunked upload staging
└── index/             # Tantivy full-text search index
```

## Development

The frontend build runs as part of `cargo build` (see [Web frontend](#web-frontend)):

- **esbuild** bundles `frontend/entries/*.js` into `static/js/*.bundle.js`. It is required — the
  build panics if esbuild is not on `PATH` or in `node_modules/.bin`. Install with `npm install`.
- **Tailwind** compiles `static/css/input.css` into `app.css`. It is optional — if the Tailwind CLI
  is unavailable the build still succeeds and the UI renders unstyled.

`build.rs` tracks `frontend/`, `static/css/`, and `templates/` via `rerun-if-changed`, so editing
frontend source triggers a re-bundle on the next `cargo build`. There is no hot reload — the assets
are embedded in the binary, so a rebuild is required to pick up frontend changes.

## Testing

Tests are split across three layers:

| Layer | Command | CI job |
|-------|---------|--------|
| Rust unit + integration | `cargo test --workspace` | `test` |
| Frontend unit | `node --test "server/frontend/**/*.test.js"` (zero-dependency `node:test`) | `frontend-test` |
| Browser end-to-end | `cd e2e && npm install && npx playwright install --with-deps chromium && npx playwright test` | `e2e` |

The Playwright suite boots a real `nanofile` binary against an isolated temporary database and drives
the UI in Chromium, covering login, selection, view switching, sorting/filtering, upload, file
operations, sharing, history, preview, tags, and search. Failed runs capture the backend log at
`e2e/test-results/server.log`.

Formatting and lint checks are also enforced by CI:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## CI & Releases

- **`ci.yml`** (push / PR to `main`, `master`, `develop`): formatting, clippy (`-D warnings`),
  frontend unit tests, Playwright e2e, and the Rust test suite.
- **`nightly.yml`** (daily / manual): multi-arch release builds (Linux amd64/arm64/loong64 ×
  gnu/musl, macOS arm64, Windows amd64) and publishes OCI images to `ghcr.io` (`:edge`, `:sha-<sha>`).
- **`release.yml`** (tag `v*.*.*` / manual): the same multi-arch builds plus a GitHub release with an
  auto-generated changelog and versioned images (`:latest`, `:vX.Y.Z`, `:vX.Y`).

## License

MIT
