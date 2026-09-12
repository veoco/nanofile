# Nanofile

[English](README.md) | [简体中文](README.zh-CN.md)

A wire-compatible [Seafile](https://www.seafile.com/) server written in Rust.

Nanofile speaks the Seafile sync protocol and REST APIs, so official Seafile desktop / mobile
clients and tools like `seaf-cli` can point at it directly. It also ships its own web UI
(file browser, sharing, admin panel) as a single static binary — no separate `seaf-server` +
`seahub` stack to install.

## Features

- **Seafile sync protocol** (`/seafhttp/`, protocol version 2): content-addressed commits and FS
  objects, block transfer with SHA-1 verification, packed FS objects, `check-fs` / `check-blocks`,
  quota & permission pre-checks, per-repo sync tokens, file locking.
  - **Security note**: `/seafhttp/repo/head-commits-multi/` is unauthenticated by protocol
    requirement — the official desktop client calls it with no credentials (seafile
    `daemon/http-tx-mgr.c`), and the official server validates no token (`server/http-server.c`);
    requiring auth would make desktop clients silently stop detecting remote updates. Exposure is
    bounded: library IDs are 128-bit random UUIDs (blind enumeration is infeasible), and knowing an
    ID only confirms existence and reveals the head-commit SHA, which is unusable without a repo
    sync token. The endpoint still rejects non-UUID ids and caps the array at 4096.
- **REST API**: legacy v1 (`/api2/*`) and v2.1 (`/api/v2.1/*`) surfaces covering libraries, files,
  directories, sharing, activities, search, trash, devices, avatars and more — compatible with the
  official mobile apps.
- **WebDAV** (`/dav/...`) with per-library keys, gated by `webdav_enabled`.
- **Web UI**: file browser with previews and thumbnails, starred files, activity feed, trash,
  settings (profile, devices, 2FA, invitations), and a **sysadmin panel** (users, shares,
  background tasks). Localized in English and Chinese.
- **Sharing**: share links (optional password / expiry / view counting), anonymous upload links,
  user shares with rw/r permissions, custom share permissions. A global `share_link_enabled` switch
  can disable anonymous share/upload links entirely (existing links become inaccessible and the
  `share-link-disabled` feature is advertised so clients hide sharing).
- **Security**: TOTP two-factor auth with backup codes and trusted devices, SSO / "view on website"
  login, invitation-code registration, login rate limiting with lockout, password reset (email-gated),
  hashed session cookies with CSRF protection, path-traversal-safe filename handling.
  - **Security note**: API, S2FA, SSO-login and client-login bearer tokens are stored as SHA-256
    hashes, so a leaked database does not yield usable credentials. Sync tokens stay recoverable
    (clients re-present them), so they are encrypted at rest with an AEAD key derived from
    `secret_key`; share-link tokens remain plaintext because the "my shares" list shows the
    copyable URL — matching official Seafile's plaintext URL model. In **release** builds
    `NANOFILE_SERVER_SECRET_KEY` / `[server] secret_key` is **required** (startup fails rather than
    deriving keys from an ephemeral secret); debug builds auto-generate one.
- **Encrypted libraries**: AES-256-CBC blocks with Seafile-compatible `magic` / `random_key`,
  in-memory password cache with TTL.
  - **Security note**: the stored `magic` is derived with PBKDF2-SHA256 at **1000 iterations**,
    which the Seafile wire protocol fixes and cannot be raised without breaking client interop.
    `magic` is a password-equivalent value, so a leaked database lets weak passwords be brute-forced
    offline. Use a long, random library password, and prefer **enc_version 4** (per-library random
    salt) over v2 (a fixed global salt). The server only accepts enc_version 2/4 and validates the
    magic/random_key format on creation. Known limitation: the `/api2/repos/` create API has no
    `salt` field, so a library created through it derives as if v2 — real per-library v4 salts are
    created through the sync protocol.
- **Storage & versioning**: per-user quotas, content-addressed block store **namespaced per
  library** (`data/blocks/repos/<sha1(repo_id)>/…`), full history with revision browse / restore,
  per-repo history limits and TTL, garbage collection (history pruning + unreachable FS-object
  cleanup), trash with revert, deleted-library restore. Optional transparent at-rest encryption for
  file blocks (`block_encryption_mode`: `off` / `on` / `lazy`), with the block id (SHA-1 of logical
  bytes) unchanged so Seafile clients and content-addressed dedup keep working.
  - **Library trash**: deleting a library keeps its content. Its commit graph and FS objects are
    copied into `deleted_repo_commits` / `deleted_repo_fs_objects` (the equivalent of the official
    server's `deleted_store/`) in the same transaction that removes the library, and its blocks stay
    on disk. `POST /api/v2.1/deleted-repos/` restores the library with its files, history and head
    commit; `DELETE /api/v2.1/deleted-repos/{repo_id}/` purges one library and
    `DELETE /api/v2.1/deleted-repos/` empties the trash, in both cases reclaiming the blocks.
    Libraries deleted *before* this build were never archived: their trash entries still restore, but
    the library comes back empty and the server logs why.
  - **Upgrading from an older build**: blocks used to live in one flat, server-wide tree
    (`data/blocks/<2hex>/<id>`). That layout keyed blocks only by content id, so any authenticated
    user could read any library's block by naming it through a library they *were* a member of.
    The server now copies each referenced block into the library that owns it, then removes the old
    tree — automatically at startup, before the first request is served. Use
    `nanofile migrate-blocks --dry-run` to pre-flight the copy volume (it prints repositories,
    blocks, and bytes) and `nanofile migrate-blocks` to run it explicitly with the server stopped.
    The migration copies (never hard-links), is resumable, and only deletes the old tree after every
    referenced block is confirmed in its new location; back up `data/blocks` first if you want a
    rollback path. Because deduplication is now per library, content duplicated across libraries is
    stored once per library — expect disk usage to grow accordingly.
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
cp config.toml.example config.toml   # edit to suit — see Configuration below

# 4. Run
./target/release/nanofile
```

Open `http://localhost:8082` and log in.

An admin account is needed. Either auto-create one on first startup via `[admin_init]` in
`config.toml` (or `NANOFILE_ADMIN_INIT_EMAIL` / `NANOFILE_ADMIN_INIT_PASSWORD_FILE`), or create one
with the CLI:

```bash
./target/release/nanofile adduser --email admin@example.com
```

It prompts for the password. To avoid the prompt, pipe it in or point at a file — a password passed
as `--password` is also visible in the shell history and in `ps` output on the same host:

```bash
printf '%s\n' 'secret123' | ./target/release/nanofile adduser --email admin@example.com --password-stdin
./target/release/nanofile adduser --email admin@example.com --password-file /run/secrets/admin
```

Pass `--regular` to create a non-admin account.

## Configuration

Settings are read from `config.toml` in the working directory. Override the path with
`--config <path>` (highest priority) or the `NANOFILE_CONFIG` environment variable. If the file is
missing, the server falls back to built-in defaults, so it can start with zero config — supply
whatever you need via `NANOFILE_*` environment variables. Every key can also be overridden with
a `NANOFILE_*` environment variable — the shipped `config.toml.example` lists the exact variable name
in a comment above each key (e.g. `NANOFILE_DATABASE_URL`, `NANOFILE_SERVER_PORT`). Environment
variables always win at runtime and are never written into the file.

The live `config.toml` is deliberately not tracked by git: it holds the master `secret_key` (and
optionally the admin-init, notification and storage-encryption keys). Copy the example, keep your
own copy out of version control.

On upgrade to a newer release, `config.toml` is automatically migrated in place (comments preserved)
when the config format changed, backed up as `config.toml.bak` first; on a read-only mount the
migration is applied in memory only.

| Section | Purpose |
|---------|---------|
| `[server]` | Bind address/port, `site_url` (external URL used for download/share links and cookies — set to your HTTPS domain behind a TLS proxy), max upload size, request timeout, CORS, WebDAV switch, feature switches (`sso_enabled`, `file_search_enabled`, `share_link_enabled`, `tray`), desktop-client branding (`desktop_custom_brand` / `desktop_custom_logo`), trusted reverse proxies (`trusted_proxies`). |
| `[database]` | SeaORM/SQLite connection URL (default `sqlite:data/nanofile.db?mode=rwc`) and pool size. |
| `[storage]` | Block store, temp, thumbnail and avatar directories, global storage quota cap (`max_storage_bytes`, `0` = unlimited), ffmpeg path for video thumbnails, resumable-upload temp limits (`max_temp_uploads`, `max_temp_upload_bytes`, `temp_upload_ttl_hours`), zip-archive caps (`max_zip_entries`, `max_zip_bytes`, `0` = unlimited), and transparent at-rest block encryption (`block_encryption_mode` / `encryption_key`). |
| `[auth]` | Password hashing cost, token TTLs, login lockout, invitation registration, password policy, and per-IP rate limits (password reset, registration, TOTP verification, share/upload-link passwords, anonymous share downloads). |
| `[ui]` | Default UI language (`en` / `zh`), tray menu language (`tray_language`: `auto` follows the OS locale, `en`/`zh` force one). |
| `[email]` | Master switch for the email backend. Password-reset links are only delivered to the owner's inbox and are never echoed back by the server, so the reset flow stays disabled until an SMTP backend exists. |
| `[admin_init]` | Optional first-start admin auto-creation. Prefer `NANOFILE_ADMIN_INIT_PASSWORD_FILE` for the password. |
| `[logging]` | Log level, optional rotating log file (`file_enabled`, `file`, `max_file_size_mb`, `max_backups`). |
| `[gc]` | Enable / schedule garbage collection. |
| `[index]` | Full-text search switch (`enabled`) and index directory. |
| `[notification]` | WebSocket notification settings and JWT private key, plus connection caps (`max_connections`, `max_connections_per_ip`) and the unauthenticated-connection subscribe timeout (`subscribe_timeout_secs`). |
| `[tasks]` | Max concurrent background copy/move tasks (`max_active_tasks`, `0` = unlimited; excess requests get HTTP 429). |

`secret_key` is the single master key: the notification key and CSRF signing key are derived from it.
Generate a unique one for production with `openssl rand -hex 32` and set it via
`NANOFILE_SERVER_SECRET_KEY` (an empty value auto-generates a random key on startup, which invalidates
sessions on restart).

## Security

The server ships secure defaults for a single-node deployment, but a few things depend on how you
run it:

- **Terminate TLS in front of nanofile and set `site_url` to the HTTPS URL.** That one setting
  drives `Secure` on session/link cookies and enables `Strict-Transport-Security`; left as plain
  HTTP, neither is sent (a LAN deployment must not be pinned to HTTPS it cannot serve). Sessions,
  share-link passwords and API tokens are bearer credentials.
- **File blocks are stored per library** (`data/blocks/repos/<sha1(repo_id)>/…`), and every block
  read/write names the library it belongs to. A block id therefore only grants access through a
  library the caller is a member of: a removed collaborator who still has another library on the
  server cannot read the blocks their client cached from the one they lost. This also means
  deduplication is per library rather than server-wide.
- **Deleted libraries keep their disk usage until their trash entry is purged.** Garbage collection
  never reclaims the blocks of a library that is still listed in the trash — that is what makes a
  restore serve its files again. Purge the library (or empty the trash) to free the space.
- **`addr = "0.0.0.0"` is the default** so the server is reachable on the host's interfaces. Bind
  `127.0.0.1` when a reverse proxy is the only intended entry point, and firewall the port otherwise.
- **Behind a reverse proxy, set `trusted_proxies`.** `X-Forwarded-For` is only honoured when the TCP
  peer is listed there, so client-IP rate limiting cannot be spoofed from outside.
- **`share_link_enabled = false`** turns off anonymous share/upload links entirely (existing links
  stop resolving). `allowed_hosts` pins the host names used to build absolute download URLs when
  `site_url` is unset.
- **Run the server with a minimal `PATH`.** Helper binaries (`ffmpeg` for video thumbnails,
  `xdg-open`/`launchctl` for tray actions) are looked up through `PATH`; point
  `storage.ffmpeg_path` at an absolute path and keep untrusted directories (a world-writable
  working directory, `node_modules/.bin`) out of the server's `PATH`.
- **Tighten the example's finite caps if you serve many users** (`max_zip_bytes`,
  `max_temp_upload_bytes`); `0` means unlimited.

Deliberate, documented trade-offs (no code path is unprotected — each is bounded by something else):

- **Encrypted-library passwords.** The key-derivation iteration count for encrypted libraries is
  fixed at 1000 by the sync protocol: the official clients derive the data key themselves, so
  changing it would make their libraries unreadable. `encrypted_library_pwd_hash_algo` /
  `encrypted_library_pwd_hash_params` can raise the cost of the *server-side verification* hash for
  newly created libraries (the desktop client reads those fields; mobile clients only support
  protocol version ≤ 2), but the defaults stay compatible. Online guessing is bounded by
  `repo_password_max_per_hour` instead.
- **Zip downloads (`/zip/{token}`) are capability URLs**, exactly like upstream's file-server
  tokens: single-use, expiring, unguessable and redacted from the logs. Unlike upstream, the
  requester's library permission is re-checked when the token is consumed, so a user whose access
  was revoked (or whose account was deactivated) inside the token's one-hour TTL cannot still pull
  the archive. Treat a zip URL like a password anyway.
- **`head-commits-multi` and `check_blocks`** answer anonymous/authenticated callers the same way
  upstream does (library metadata and block existence). They are required by the sync protocol;
  rate limiting bounds the request rate.
- **Configuration secrets are held in memory as ordinary strings** (server secret, notification
  key, at-rest encryption key, database URL, admin password) and are not zeroed on drop; the
  derived AEAD keys used by the token/TOTP/block ciphers are cleared. Scrubbing the live
  configuration would need a secret-typed config throughout and buys little against the threat
  model (an attacker reading process memory already has the running server).

## Logging

Headless runs (servers, Docker, CLI subcommands) log to stdout as before, controlled by
`[logging] level` (or `NANOFILE_LOG_LEVEL`).

Desktop (tray) runs log to a size-capped rotating file instead:

- The default location is `nanofile.log` **next to the nanofile binary**; the resolved absolute path
  is written back into `config.toml` (`[logging] file`) on first run, so login-started instances
  always use the same file regardless of their working directory, and you can change it there.
- `[logging] file` accepts an explicit path; a relative one resolves against the binary's directory
  (never the working directory, which is meaningless for auto-started instances).
- `max_file_size_mb` (default 10) caps each file; once exceeded it rotates to `nanofile.log.1`,
  `.2`, … with `max_backups` (default 3) older files kept. `max_backups = 0` truncates in place.
- `file_enabled = true/false` forces file/stdout output; unset means automatic (file in desktop
  mode, stdout otherwise). If the log file cannot be opened (e.g. a read-only binary directory),
  the server falls back to the working directory, then to stdout.

## System Tray (optional)

Release archives whose name ends in `-tray` (Windows / macOS / Linux-amd64) include an optional
system tray icon, compiled in with the `tray` feature. Plain builds contain no tray code at all,
so servers without a desktop are unaffected.

Right-clicking the tray icon opens a menu with (translated to the system language — Chinese systems
get Chinese menus; force a language with `tray_language = "en"/"zh"` in `[ui]`):

- **Open Web UI** — opens `site_url` in the default browser
- **Launch at Login** (checkable) — registers/unregisters auto-start for the current user:
  - Windows: `HKCU\...\CurrentVersion\Run` registry value (no admin rights needed). When the server
    runs elevated ("Run as administrator"), a dialog asks for confirmation before registering, since
    the entry then belongs to the elevated account; a login-started instance always runs without
    elevation.
  - macOS: a LaunchAgent at `~/Library/LaunchAgents/com.nanofile.nanofile.plist`
  - Linux: an XDG autostart entry at `~/.config/autostart/nanofile.desktop` (GNOME and KDE)
- **Open Config File** — reveals the config file actually in use (in Explorer / Finder / the file manager)
- **Quit** — triggers the same graceful shutdown as Ctrl+C

The auto-start entries always point at the running binary and pass `--config <absolute path>`, so
the auto-started instance uses the same config regardless of its working directory.

Notes:

- Toggle the tray off with `tray = false` in `[server]` (or `NANOFILE_SERVER_TRAY=false`), e.g. for
  an auto-started instance that should stay invisible.
- On Linux the tray needs a desktop session; without `DISPLAY`/`WAYLAND_DISPLAY` (or when the
  desktop session is broken) the server logs a warning and runs headless instead of failing.
- GNOME only shows tray icons with the "AppIndicator and KStatusNotifierItem Support" extension
  installed; KDE Plasma supports them out of the box.
- On Windows, `-tray` builds are GUI-subsystem binaries: no console window ever appears (double-click,
  auto-start, or terminal). Logs go to the rotating log file (see Logging). CLI-only output such as
  `--version` or `adduser` prompts is only visible when launched from a terminal (the binary
  reattaches to it, but `cmd` does not wait for the process) — for full console use the plain
  (non-`-tray`) build, which behaves exactly as before.

To build the tray variant yourself (Linux additionally needs `libgtk-3-dev` and
`libayatana-appindicator3-dev`):

```bash
cargo build --release -p server --features tray
```

The tray icon and the Windows executable icon are rasterized from `server/static/img/favicon.svg`
at compile time — no image assets are shipped in the repository.

## Docker

The release image is a `scratch` container holding only the `nanofile` binary — no config file or
data directory. It runs as uid/gid `1000:1000`, so the data volume must be writable by that user
(`chown -R 1000:1000 ./data` when upgrading an older deployment, or pass
`--user "$(id -u):$(id -g)"` to match your own account). Mount a config file and a persistent data
volume, and point the data paths at the volume:

**Create the master secret once and keep it.** It derives the session/CSRF keys, the notification
JWT keys, the sync-token encryption key and the at-rest storage key, so generating a new one on every
start logs everybody out, breaks every sync client and makes existing 2FA enrolments and at-rest
encrypted blocks undecryptable:

```bash
mkdir -p data
# One-time, persisted (mode 0600): rotating this value is a destructive operation.
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret

docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -v "$PWD/config.toml:/etc/nanofile/config.toml:ro" \
  -v "$PWD/nanofile-secret:/run/secrets/nanofile-secret:ro" \
  -e NANOFILE_CONFIG=/etc/nanofile/config.toml \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_INDEX_INDEX_DIR=/data/index \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
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
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/<owner>/nanofile:latest
```

(`nanofile-secret` is the persisted value created above — never regenerate it per start.)

## CLI

```
nanofile [--config <path>]           Start the server (default)
nanofile [--config <path>] adduser   Create a user (admin by default; --regular for a normal user)
                                     Password: interactive prompt by default, or
                                     --password-stdin / --password-file <path>
nanofile [--config <path>] migrate-blocks [--dry-run]
                                     Move blocks from the legacy flat layout to the per-library
                                     layout (normally done automatically at startup; --dry-run only
                                     reports what would be copied)
```

## Data Layout

All state lives under the working directory (defaults shown):

```
data/
├── nanofile.db        # SQLite database (WAL mode, file mode 0600)
├── nanofile.db-wal    # WAL journal
├── blocks/            # block store: repos/{sha1(repo_id)}/{2-hex prefix}/{40-hex SHA-1}
├── temp/              # resumable / chunked upload staging
├── thumbnails/        # generated image / video thumbnail cache
├── avatars/           # user avatar images
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
