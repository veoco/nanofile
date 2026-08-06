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
  user shares with rw/r permissions, custom share permissions, wikis, doc comments.
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

## Quick Start

```bash
# Build the server (binary name is `nanofile`, not `server`)
cargo build --release -p server

# Configure
cp config.toml .   # edit to suit — see Configuration below

# Run
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

Settings are read from `config.toml` in the working directory. Every key can also be overridden with
a `NANOFILE_*` environment variable — the shipped `config.toml` lists the exact variable name in a
comment above each key (e.g. `NANOFILE_DATABASE_URL`, `NANOFILE_SERVER_PORT`).

| Section | Purpose |
|---------|---------|
| `[server]` | Bind address/port, `site_url` (external URL used for download/share links and cookies — set to your HTTPS domain behind a TLS proxy), max upload size, request timeout, CORS, WebDAV switch, trusted reverse proxies. |
| `[database]` | SeaORM/SQLite connection URL (default `sqlite:data/nanofile.db?mode=rwc`). |
| `[storage]` | Block store and temp directories, global storage quota cap (`max_storage_bytes`, `0` = unlimited). |
| `[auth]` | Password hashing cost, token TTLs, login lockout, invitation registration, password policy, rate limits. |
| `[ui]` | Default UI language (`en` / `zh`). |
| `[email]` | Master switch for the email backend. Password-reset links are only delivered to the owner's inbox and are never echoed back by the server, so the reset flow stays disabled until an SMTP backend exists. |
| `[admin_init]` | Optional first-start admin auto-creation. Prefer `NANOFILE_ADMIN_INIT_PASSWORD_FILE` for the password. |
| `[logging]` | Log level. |
| `[gc]` | Enable / schedule garbage collection. |
| `[index]` | Full-text search index directory. |
| `[notification]` | WebSocket notification settings and JWT private key. |

`secret_key` is the single master key: the notification key and CSRF signing key are derived from it.
Generate a unique one for production with `openssl rand -hex 32` and set it via
`NANOFILE_SERVER_SECRET_KEY` (an empty value auto-generates a random key on startup, which invalidates
sessions on restart).

## CLI

```
nanofile          Start the server (default)
nanofile adduser  Create a user (admin by default; --regular for a normal user)
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

```bash
# Run the full test suite (unit + integration, ~40 test files)
cargo test --workspace

# Format & lint checks enforced by CI
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

The web UI is server-rendered (Askama) with Tailwind CSS and a small amount of vanilla JS, embedded
into the binary via `rust-embed`. `server/build.rs` runs the Tailwind CLI when available (standalone
binary or `npx @tailwindcss/cli`); if it's missing, the build still succeeds but the UI renders
unstyled. Install a Tailwind CLI for a styled interface:

```bash
curl -sL https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-linux-x64 \
  -o server/tailwindcss && chmod +x server/tailwindcss
```

CI runs formatting, clippy (`-D warnings`), the full test suite, and a release build. Nightly and
tag-triggered workflows build multi-arch binaries (linux amd64/arm64/loong64 × gnu/musl, macOS,
Windows) and publish OCI images to `ghcr.io` (`:edge`, `:sha-<sha>`, and `v<X.Y.Z>` tags for releases).

## License

MIT
