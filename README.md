# Nanofile

[English](README.md) | [简体中文](README.zh-CN.md)

Nanofile is a self-hosted file sync and sharing server. It implements the Seafile
sync protocol and APIs, so the official Seafile desktop and mobile clients connect
to it directly, and it ships its own web interface without a separate seaf-server +
seahub stack.

**Documentation:** <https://veoco.github.io/nanofile/> · [中文](https://veoco.github.io/nanofile/zh/)

## Features

- **File sync** — the official Seafile clients keep libraries in sync automatically.
- **Web interface** — browse, preview, download, star, activity feed and trash.
- **Sharing** — share links with a password, an expiry and a view count; anonymous upload links.
- **Sign-in security** — TOTP two-factor authentication, trusted devices, login lockout and API keys.
- **Admin console** — users, quotas, email delivery, system settings and background tasks.
- **Confined parsing** — document, image, EXIF and media parsing runs in bounded child processes.

## Quick start

Download an archive for your platform from
[GitHub Releases](https://github.com/veoco/nanofile/releases) — `nanofile-v<version>-<suffix>.tar.gz`
(`.zip` on Windows), with `-tray` variants that add a desktop tray menu — and run
the binary inside it:

```bash
tar xzf nanofile-v1.2.3-linux-amd64.tar.gz
cd nanofile-v1.2.3-linux-amd64
chmod +x nanofile
./nanofile
```

A start with no configuration file and no `NANOFILE_*` variable runs the first-run
setup: it serves a one-time page at `http://127.0.0.1:8082/setup/<token>`, creates
the first administrator, writes `~/.nanofile/config.toml` and starts the server on
the same port. Delete that file to run the setup again.

To manage the configuration yourself instead, a config file keeps the setup from
running:

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # it holds the master secret
./nanofile --config config.toml
```

The server listens on <http://localhost:8082>. Create an administrator (the setup
page does this on a first run):

```bash
./nanofile adduser --email admin@example.com
```

`--regular` creates a non-admin account. More in the
[getting started guide](https://veoco.github.io/nanofile/guide/getting-started).

## Docker

The image is a `scratch` container holding only the binary. It runs as uid/gid
`1000:1000`, so the data volume must be writable by that user.

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret

docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

The master secret is generated once and reused: rotating it logs out every
session, breaks sync clients and makes encrypted blocks unreadable. See the
[Docker guide](https://veoco.github.io/nanofile/guide/docker) for the config-file
variant, the first-run caveat and upgrade permissions.

## Documentation

| | |
|---|---|
| [Installing and first run](https://veoco.github.io/nanofile/guide/getting-started) | Download an archive or the image, first run, administrator |
| [Web interface](https://veoco.github.io/nanofile/guide/web-interface) | Libraries, upload and download, search, trash |
| [Connecting clients](https://veoco.github.io/nanofile/guide/clients) | Seafile clients, WebDAV, API keys |
| [Configuration](https://veoco.github.io/nanofile/guide/configuration) | Config file, `NANOFILE_*`, settings pages |
| [Running in the background](https://veoco.github.io/nanofile/guide/running-in-background) | systemd, launchd, container restart policy |
| [Backup, restore and upgrade](https://veoco.github.io/nanofile/guide/backup-and-upgrade) | Backing up data, restoring, changing versions |
| [Maintenance and troubleshooting](https://veoco.github.io/nanofile/guide/maintenance) | Logs, garbage collection, common problems |
| [Security](https://veoco.github.io/nanofile/guide/security) | Reverse proxy, TLS, secret key |
| [API reference](https://veoco.github.io/nanofile/reference/api/) | Every HTTP endpoint, plus the Seafile comparison |
| [CLI reference](https://veoco.github.io/nanofile/reference/cli) | Every operator command |
| [Development](https://veoco.github.io/nanofile/dev/architecture) | Architecture, testing, CI |

## Development

The project is a Cargo workspace of four crates: `base` (domain types), `infra`
(database, storage, crypto, config), `server` (HTTP, sync protocol, WebDAV, web UI)
and `migration`. Dependencies flow `base → infra → server`.

Requirements: a stable Rust toolchain (edition 2024) and Node.js (CI uses 24) for
the frontend bundler; `ffmpeg` is optional and enables media thumbnails.

```bash
npm ci                            # esbuild and Tailwind
cargo build --release -p server   # the binary is target/release/nanofile
cargo test --workspace
node --test "server/frontend/**/*.test.js"
```

See [Building](https://veoco.github.io/nanofile/dev/build) for the `tray` feature
and cross-compilation, and [Testing](https://veoco.github.io/nanofile/dev/testing)
and [Contributing](https://veoco.github.io/nanofile/dev/contributing) for the
browser suite, the platform matrix and the dependency rules.

## License

MIT
