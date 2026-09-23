# Nanofile

[English](README.md) | [简体中文](README.zh-CN.md)

Nanofile is a self-hosted file sync and sharing server. It implements the Seafile sync protocol and APIs, so the official Seafile desktop and mobile clients connect to it directly, and it includes its own web interface without a separate seaf-server + seahub stack.

## Features

- **File sync**: desktop and mobile clients keep libraries in sync automatically, as with an official Seafile server.
- **Web interface**: browse, preview, download, star, activity feed, trash.
- **Sharing**: share links with optional password, expiry and view count; anonymous upload links.
- **Sign-in security**: TOTP two-factor authentication, trusted devices, login lockout. API keys let third-party tools (WebDAV, sync clients) connect without sharing the account password.
- **Admin console**: users, quotas, mail delivery and system settings.
- **Other**: encrypted libraries, full-text search with Chinese tokenization, file history and trash, background garbage collection.

## Quick start

Build from source:

```bash
npm install                       # frontend bundler (esbuild is required)
cargo build --release -p server   # binary is `nanofile`, not `server`
cp config.toml.example config.toml
./target/release/nanofile
```

The server listens on http://localhost:8082. An admin account is required and can be created with the CLI:

```bash
# password prompt
./target/release/nanofile adduser --email admin@example.com
# or non-interactive
printf '%s\n' 'secret123' | ./target/release/nanofile adduser --email admin@example.com --password-stdin
```

`--regular` creates a non-admin account. A prebuilt container image is described under **Deploy with Docker**.

## Usage

### Users
`adduser` creates an account, admin by default. Invitation-code registration can be enabled in `[auth]` for self-registration.

### Sharing
Selecting a file or folder in the web file browser and choosing "Share" produces a share link, which supports a password, an expiry and a view count. Anonymous upload links accept uploads into a library without an account. Setting `share_link_enabled` to false disables anonymous share and upload links, and existing links stop resolving.

### Email notifications
Mail is off until SMTP is configured. Two places configure it: the web admin page "System Management → Email" (host, port, account, password, sender, then `enabled`), or environment variables such as `NANOFILE_EMAIL_ENABLED=1` and `NANOFILE_EMAIL_HOST=…`.

Once enabled, the server sends: password-reset links, new-device and new-browser sign-in notices, and a notice when an API key is created. Password values can be read from a file through a `*_FILE` variable (`NANOFILE_EMAIL_PASSWORD_FILE`), which keeps them out of the command line and process list.

### Account security
"Settings → Security" enables two-factor authentication and issues backup codes. "Settings → Sessions & Credentials" lists the devices signed in to the account, its repository sync tokens and its API keys, each individually revocable. Changing the password revokes other devices, sync tokens and all API keys.

## Deploy with Docker

The image is a `scratch` container holding only the `nanofile` binary, with no config file or data directory. It runs as uid/gid `1000:1000`, so the mounted data volume must be writable by that user.

The master secret is generated once and kept: it encrypts sessions, mail and at-rest blocks. Rotating it logs out every session, breaks sync clients and makes encrypted blocks unreadable.

```bash
mkdir -p data
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

A config file is optional; built-in defaults fill the rest:

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

`nanofile-secret` is the value generated above and is reused across starts.

## Configuration

Settings come from `config.toml` in the working directory (copy `config.toml.example`; each key's comment gives its environment-variable name) or from `NANOFILE_*` environment variables, which take precedence and are never written back to the file. Container deployments rely on the environment variables.

Frequently changed keys:

- `site_url`: the external URL (HTTPS domain). It controls the `Secure` attribute on session cookies, enables HSTS, and is used to build share links.
- Bind address and port (`[server]` `addr` / `port`): default `0.0.0.0:8082`.
- Storage directories (`[storage]`): default under `data/` in the binary's directory. Relative paths resolve against the binary's directory; absolute paths place state elsewhere.
- `secret_key`: the master key (see **Deploy with Docker**).

Most settings apply immediately; the listener, transfer limits and cache directories apply at the next start. Some runtime settings are also editable at "System Management → Settings" in the web interface, where a saved value overrides the same key in the config file.

The **Restart server** button at the bottom of those pages restarts the server in place: the process, the tray icon and the listening port stay the same, but the settings table, database connections, caches and background tasks are all rebuilt, so every "needs a restart" row takes effect immediately (and the administrator's session survives). The log level and file caps, and whether the desktop tray exists and in which language, are decided before the process starts: those are marked "needs a full process restart" and are only applied by stopping and starting the process.

On upgrade, a changed config format is migrated in place with comments preserved, and the previous file is kept as `config.toml.bak`.

Sections: `[server]` networking and feature switches; `[database]` connection; `[storage]` directories and quotas; `[auth]` login, password and rate limits; `[ui]` language; `[email]` mail; `[admin_init]` first-start admin; `[logging]` logs; `[gc]` garbage collection; `[index]` search; `[notification]` notifications; `[tasks]` background tasks. Exact keys are listed in `config.toml.example`.

## Security

- When the server runs behind an HTTPS reverse proxy, `site_url` is set to the HTTPS address. That setting controls the `Secure` cookie attribute and HSTS. Sessions, share passwords and API tokens are bearer credentials.
- The default bind address is `0.0.0.0`. Where only a reverse proxy should reach the server, `127.0.0.1` plus a closed firewall port restricts it.
- Behind a reverse proxy, `trusted_proxies` determines whether `X-Forwarded-For` is honoured. Without it, client IPs can be spoofed to bypass per-IP rate limits.
- Encrypted libraries require the library password for uploads as well as reads. Without it, web preview and download return 440, and anonymous upload links cannot be created for or used against an encrypted library.
- A password change, deactivation or device wipe revokes every credential the account holds: other devices, sync clients and API keys, plus unused reset links.
- Release builds refuse to start without `secret_key`. Debug builds generate one, but sessions do not survive a restart. `NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` covers local and CI use.

## Command line

```
nanofile [--config <path>]           start the server (default)
nanofile [--config <path>] adduser   create a user (admin by default; --regular for a normal user)
                                     password: interactive, or --password-stdin / --password-file <path>
nanofile [--config <path>] migrate-blocks [--dry-run]
                                     move from the shared block directory to the per-library layout
                                     (normally done automatically at startup; --dry-run previews only)
```

Windows service control (administrator rights required; equivalent to the tray's
"Start as a Windows Service" item, except for `run`):

```
nanofile [--config <path>] service install    register an auto-start service (starts at boot, no login)
nanofile [--config <path>] service uninstall  stop and remove it
nanofile [--config <path>] service status     report the registration (exit code 0 when installed)
nanofile [--config <path>] service run        what the SCM calls; running it by hand exits with an error
```

## Desktop tray and Windows service

Binaries built with `--features tray` carry a system tray menu: **Start automatically after login** (a per-user entry, so it starts once somebody logs in), **Start automatically at boot (administrator rights required)** on Windows — see below — plus open the web UI, open the config file and quit.

On Windows there is also **Start as a Windows Service**: registered with the Service Control Manager, it starts at boot with **no login at all**, which is what an unattended machine needs.

- Enabling it asks for administrator rights; dismissing the prompt changes nothing.
- The two automatic-start options are alternatives: registering the service removes the "Start automatically after login" entry (that is the one thing install changes about your startup entries, and the confirmation says so), and the tray disables that item while the service is registered. Removing the service enables the item again but does **not** re-create the entry: after that, Nanofile starts automatically only if you ask for it.
- Both registrations record absolute paths, so they outlive a move. A login entry whose recorded executable or config file is gone is repointed at the running copy the next time the tray starts (a healthy entry, or one this build cannot read, is left alone). A service registration that points at a folder which no longer exists is named as such in the confirmation before it is repointed. And naming a config file that does not exist (`--config`, or `NANOFILE_CONFIG`) is now a refused start rather than a silent fall back to built-in defaults — which would have quietly served a different database on a different port.
- It takes effect at the **next system start**; the tray copy running now keeps serving. Handing the port over immediately is not reliable on Windows, where a just-closed connection stays in `TIME_WAIT` for minutes and the new process would fail to bind.
- The service runs as `LocalSystem`, so the state directories have to be writable by that account. While it runs, starting the tray by hand gives you a "client mode" tray: it brings up no second server, and **Quit stops the service** — the same thing Quit means in an ordinary tray, where it stops the server. It restarts at the next boot, and the tray can also install/remove the service from that menu.
- A service has no desktop, so there is no tray icon; logs go to `nanofile.log` next to the executable.

## Data directory

State lives under the binary's directory in `data/` by default, since relative paths resolve against that directory. A section can name absolute paths instead.

```
data/
├── nanofile.db        # database (WAL mode, mode 0600)
├── nanofile.db-wal    # WAL journal
├── blocks/            # file blocks: repos/{sha1(repo_id)}/{2-hex prefix}/{40-hex SHA-1}
├── temp/              # upload staging
├── thumbnails/        # thumbnail cache
├── avatars/           # avatars
└── index/             # full-text search index
```

## Development

**Architecture**: a Cargo workspace of four crates — `base` (base types), `infra` (database, storage, crypto, config), `server` (HTTP, sync protocol, WebDAV, web interface), `migration` (database migrations). Dependency direction: `base → infra → server`.

**Frontend**: the web interface is server-rendered (Askama) with Tailwind CSS and modular JavaScript under `server/frontend/`. `server/build.rs` bundles `frontend/entries/*.js` into the binary with esbuild (required; Tailwind optional). Frontend changes require a `cargo build`; there is no hot reload.

**Testing**: `cargo test --workspace` for Rust, `node --test "server/frontend/**/*.test.js"` for frontend units, and `cd e2e && npx playwright test` for browser end-to-end. CI also runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.

**CI and releases**: `ci.yml` runs the test suites on push and pull requests; `nightly.yml` builds multi-architecture images daily (`:edge`); `release.yml` publishes versioned images and a GitHub release on a version tag.

## License

MIT
