---
description: Pick between the desktop tray, a headless server service, and Docker.
---
# Choose a deployment

**Who this is for:** a first-time installer choosing how to run Nanofile.

Nanofile is one binary, but there are three common ways to run it. Pick the shape
that matches the machine and the amount of unattended operation you need.

## The three shapes

| Shape | Best for | Start with |
|---|---|---|
| **Desktop tray** | A laptop or desktop where you want a tray menu and start-at-login | A `-tray` archive from [Installing and first run](/guide/start/getting-started) |
| **Headless server** | A Linux or macOS host that should start at boot and run without a desktop | [Running in the background](/guide/deploy/running-in-background) |
| **Docker** | A container host, a compose file, or an isolated deployment | [Docker](/guide/deploy/docker) |

Windows is a special case: every Windows build has the service commands, and the
`-tray` build adds the tray menu. See
[Windows service and desktop tray](/guide/deploy/desktop-and-service).

## Desktop tray

Use a `-tray` archive when Nanofile runs on the same machine you use and you want a
tray icon to open the web interface, open the configuration file, start at login, or
quit. On Windows, the tray can also install an auto-start service that runs at boot
without a login.

- **Good for:** a home server on a desktop, a NAS with a desktop session, evaluating
  Nanofile.
- **Not for:** a headless host with no desktop session, or a server that must start
  before anybody logs in. Use a service instead.
- **Keep in mind:** on Linux the tray is a StatusNotifierItem service on the session
  bus, so the desktop must provide a host for it (KDE natively, GNOME with the
  AppIndicator extension).

## Headless server

Run the plain archive under a service manager. The server writes logs to stdout by
default, so journald or launchd can collect them.

- **Linux:** a systemd unit, `Restart=always`, a dedicated user.
- **macOS:** a LaunchDaemon for boot, or a LaunchAgent for a user session.
- **Windows:** the Service Control Manager (`nanofile service …`).
- **Good for:** a server, a VM, a NAS without a desktop, or any unattended machine.
- **Not for:** a host where you cannot install a service or keep a process running.

## Docker

The published image is a `scratch` container with only the binary. It has no shell,
no `HOME` and no package manager; everything it needs must arrive as `NANOFILE_*`
variables or a mounted configuration file.

- **Good for:** a container host, compose, reproducible deployments and a clean
  upgrade path.
- **Not for:** a host where you cannot mount a writable data volume or cannot run a
  container.
- **Keep in mind:** the image runs as uid/gid `1000:1000`; the data volume must be
  writable by that user.

## What every shape needs

- A data directory the server user can read and write. The default is `data/` next
  to the binary; the first-run setup picks `~/.nanofile/data`.
- A master secret in `server.secret_key` or `NANOFILE_SERVER_SECRET_KEY`.
- A backup plan; see [Backup, restore and upgrade](/guide/operate/backup-and-upgrade).
- HTTPS through a reverse proxy if the server is reachable from the internet; see
  [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy).
- Optional: `ffmpeg` for media thumbnails, HEIC/HEIF/AVIF decoding before face
  detection and OCR.

## Decide in one minute

- **“I just want to try it on this laptop.”** Use a `-tray` archive and the first-run
  setup. You can move to a service later.
- **“It should start at boot on a Linux or macOS server.”** Use the plain archive and
  a systemd unit or LaunchDaemon.
- **“It runs in a container.”** Use the Docker image and a compose file.
- **“It is a Windows machine with no logged-in user.”** Install the Windows service;
  the tray build can do it from its menu.

## Next steps

- [Installing and first run](/guide/start/getting-started) — the end-to-end tutorial.
- [Docker](/guide/deploy/docker) — image, volumes and compose.
- [Running in the background](/guide/deploy/running-in-background) — systemd and launchd.
- [Windows service and desktop tray](/guide/deploy/desktop-and-service) — the Windows
  service and the tray menu.
