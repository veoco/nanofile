# Installing and first run

Nanofile is a single server binary named `nanofile` (not `server`). It speaks the
Seafile sync protocol, so the official Seafile clients can connect to it, and it
serves its own web interface.

## Install

### Download a release archive

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

**Which one:**

- Use a **non-tray** variant for a server or headless deployment; pick a `-tray`
  variant to control the service from a desktop tray menu (see
  [Desktop tray and Windows service](/guide/desktop-and-service)).
- When in doubt, or on a host with an old glibc, take the `musl` variant: it does
  not depend on the host C library.

```bash
tar xzf nanofile-v1.2.3-linux-amd64.tar.gz
cd nanofile-v1.2.3-linux-amd64
chmod +x nanofile
./nanofile
```

Two platforms stop a downloaded binary the first time: macOS quarantines it
because it is unsigned (allow it under System Settings → Privacy & Security, or
run `xattr -d com.apple.quarantine nanofile`), and Windows may raise a
SmartScreen prompt (choose "Run anyway").

### Docker

For the published image, see [Docker](/guide/docker):

```bash
docker run -d --name nanofile -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_SERVER_SECRET_KEY="$(openssl rand -hex 32)" \
  ghcr.io/veoco/nanofile:latest
```

### Build from source

To compile instead of downloading, see [Building](/dev/build). Everything after
that is the same as on this page.

## First run

A start with no configuration file and no `NANOFILE_*` environment variable is an
installation nobody has set up yet. It runs the **first-run setup** instead of
the server:

1. It serves a one-time page and prints its address to the console, for example
   `http://127.0.0.1:8082/setup/<token>`. A desktop build also opens it in a
   browser.
2. The page creates the first administrator and decides the listen address, the
   external URL, the data directory and whether stored blocks are encrypted.
3. Submitting the page writes `~/.nanofile/config.toml` (owner-only) and starts
   the server on the same port.

Every later start reads that file. Delete it to run the setup again.

The page is served on `127.0.0.1` without TLS. On a remote host, do not expose it
as it stands — set `site_url` and a reverse proxy first (see
[Security](/guide/security)), or reach it through an SSH tunnel.

## Manage the configuration yourself

A configuration file means the first-run setup never runs. This is the path for
a deployment kept in the source tree or provisioned non-interactively:

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # it holds the master secret
./nanofile --config config.toml
```

The server listens on `http://localhost:8082` by default.

An administrator account is required. The setup page creates one on a first run;
on an existing configuration, use the CLI:

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

- [Web interface](/guide/web-interface) — what you can do once you are signed in.
- [Connecting clients](/guide/clients) — the official Seafile clients and WebDAV.
- [Configuration](/guide/configuration) — sources, precedence, restart rules.
- [Security](/guide/security) — reverse proxy, `site_url`, secret key.
- [Data directory](/guide/data-directory) — where state lives.
