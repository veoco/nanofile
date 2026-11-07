---
description: Install Nanofile, create the first administrator, upload and share a file, and connect a client.
---
# Installing and first run

This tutorial takes a fresh machine to a working Nanofile server: an administrator,
a first library, an uploaded file, a share link and a connected client. It uses
either a release archive or the Docker image; pick one path and follow it through.

**Time:** about 15 minutes. **Who this is for:** a first-time self-hoster.

## What you will end up with

- Nanofile listening on `http://localhost:8082`.
- An administrator account and a configuration file.
- A library with one uploaded file.
- A share link that works in a private browser window.
- The official Seafile client signed in and syncing that library.

## Before you start

- A Linux, macOS or Windows host, a VM or a container host. For production, prefer
  Linux; the release archives cover the three operating systems.
- A directory for the database, blocks, thumbnails and index. The default is `data/`
  next to the binary.
- Optional: `ffmpeg` for media thumbnails and for HEIC/HEIF/AVIF decoding.
- A DNS name and a TLS certificate if the server will be reachable from the
  internet. You can add HTTPS after the local test; see
  [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy).

## Install

### Release archive

Every version publishes per-platform archives on GitHub Releases, named
`nanofile-v<version>-<suffix>.tar.gz` (`.zip` on Windows). An archive holds
`nanofile` (`nanofile.exe` on Windows), `README.md`, `LICENSE` and
`config.toml.example`.

| Suffix | Use |
|---|---|
| `linux-amd64` | Linux x86_64, linked against the host glibc |
| `linux-amd64-musl` | Linux x86_64, statically linked, no host glibc needed |
| `linux-arm64`, `linux-arm64-musl` | Linux ARM64 (both linkage styles) |
| `linux-loong64`, `linux-loong64-musl` | Linux LoongArch64 |
| `macos-arm64` | macOS Apple Silicon |
| `windows-amd64` | Windows x86_64 (`.zip`) |
| `linux-amd64-tray`, `macos-arm64-tray`, `windows-amd64-tray` | Same platforms, with a system tray menu |

Use a **non-tray** variant for a server; pick a `-tray` variant to control a desktop
installation from a tray menu (see
[Windows service and desktop tray](/guide/deploy/desktop-and-service)). When in
doubt, or on a host with an old glibc, take the `musl` variant.

```bash
tar xzf nanofile-v1.2.3-linux-amd64.tar.gz
cd nanofile-v1.2.3-linux-amd64
chmod +x nanofile
./nanofile
```

Two platforms stop a downloaded binary the first time: macOS quarantines it because
it is unsigned (allow it under System Settings → Privacy & Security, or run
`xattr -d com.apple.quarantine nanofile`), and Windows may raise a SmartScreen prompt
(choose "Run anyway").

### Docker

The published image is a `scratch` container with the binary at `/nanofile`. It runs
as uid/gid `1000:1000`, so the data volume must be writable by that user:

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret

docker run -d --name nanofile -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

The variables count as configuring the installation, so the container skips the
one-time setup page. Create the first administrator inside the container:

```bash
docker exec nanofile /nanofile adduser --email admin@example.com
```

For a production deployment, pin a version tag instead of `latest`, and see
[Docker](/guide/deploy/docker) for the config-file variant, the first-run caveat and
upgrade permissions.

### Build from source

To compile instead of downloading, see [Building](/dev/build). Everything after that
is the same as on this page.

## First run

A start with no configuration file and no `NANOFILE_*` environment variable is an
installation nobody has set up yet. It runs the **first-run setup** instead of the
server:

1. It serves a one-time page and prints its address to the console, for example
   `http://127.0.0.1:8082/setup/<token>`. A desktop build also opens it in a
   browser.
2. The page creates the first administrator and decides the listen address, the
   external URL, the data directory and whether stored blocks are encrypted.
3. Submitting the page writes `~/.nanofile/config.toml` (owner-only) and starts the
   server on the same port.

Every later start reads that file. Delete it to run the setup again.

The page is served on `127.0.0.1` without TLS. On a remote host, do not expose it as
it stands — set `site_url` and a reverse proxy first (see
[HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy)), or reach it
through an SSH tunnel.

**Docker:** the command above already set `NANOFILE_*`, so there is no setup page.
Use the `docker exec … adduser` command from the Docker section, then open
`http://localhost:8082`.

## Create a library and upload a file

1. Open `http://localhost:8082` and sign in with the administrator account.
2. In **Libraries**, choose **new library**, give it a name and create it.
3. Open the library, choose **upload file**, pick a small file and wait for the
   progress panel to finish.
4. The file appears in the list; double-click it to check the preview or download.

If the upload button is missing, the account has read-only permission on that
library. The creator of a library has read/write permission; other users are added
through sharing, which this tutorial does not cover.

## Share the file

1. Open the file's **⋯** menu and choose **share link**.
2. Set a password and an expiry time if you want one. A share link also supports a
   view limit.
3. Copy the link, open it in a private browser window, enter the password and
   download the file.

That is the public sharing path. **Upload links** work the same way but let the
recipient upload into the library. The full set of options is in
[Files and sharing](/guide/use/files-and-sharing).

## Connect a client

The official Seafile desktop and mobile clients connect to Nanofile directly.

1. Install the official Seafile client for your platform.
2. Use the address in `server.site_url` as the server address. For the local test
   that is the address you opened, for example `http://localhost:8082`.
3. Sign in with the administrator email and password. If two-factor authentication
   is on, enter the 6-digit code and optionally trust the device.
4. Add the library you created and let the client sync it.
5. Put a file into the synced folder; it should appear in the web interface.

The client reuses the server address to fetch blocks and upload, so a wrong address
can sign in and then fail to sync. WebDAV and API keys are separate ways in; see
[Connecting clients](/guide/use/clients).

## Verify

- `curl -fsS http://127.0.0.1:8082/health` returns a 200 with no body.
- The file downloads from the web interface.
- The share link opens in a private window and the password works.
- The client shows the library as synced, and a file added on either side appears on
  the other.
- The server log has no repeated errors.

## Manage the configuration yourself

A configuration file means the first-run setup never runs. This is the path for a
deployment kept in the source tree or provisioned non-interactively:

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # it holds the master secret
./nanofile --config config.toml
```

The server listens on `http://localhost:8082` by default.

An administrator account is required. The setup page creates one on a first run; on
an existing configuration, use the CLI:

```bash
# interactive password prompt
./nanofile adduser --email admin@example.com

# or non-interactive
printf '%s\n' 'secret123' | ./nanofile adduser --email admin@example.com --password-stdin
```

`--regular` creates a non-admin account. `[admin_init]` in the configuration file
creates the first administrator automatically on a first start instead. See the
[CLI reference](/reference/cli) for the remaining commands.

## Next steps

- [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy) — put it behind
  TLS before exposing it.
- [Running in the background](/guide/deploy/running-in-background) — systemd or
  launchd.
- [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) — back up before
  you need it.
- [Users and sharing](/guide/admin/users-and-sharing) — accounts, quotas and sharing
  policy.
- [Automatic people tags](/guide/use/people-tags) and
  [Text in images and PDFs](/guide/use/ocr) — the optional media features.
