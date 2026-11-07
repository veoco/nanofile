---
description: The Nanofile guide, from first run to production.
---
# Nanofile guide

**Who this is for:** anyone installing, using or running Nanofile.

Nanofile is one binary that speaks the Seafile sync protocol and serves its own web
interface. This guide walks from the first run to running it for a long time:
installing it, connecting clients, configuring it, administering accounts and keeping
it healthy.

The guide follows the latest release. The `edge` image can contain unreleased
changes; check the release notes before relying on a new behaviour there.

## Start here

1. **[What is Nanofile?](/guide/start/what-is-nanofile)** — what it does, how far
   Seafile compatibility goes, and what it does not do.
2. **[Choose a deployment](/guide/start/choose-deployment)** — desktop tray, headless
   server or Docker.
3. **[Installing and first run](/guide/start/getting-started)** — the end-to-end
   tutorial: install, create the first administrator, upload a file, share it and
   connect a client.

## Common tasks

| I want to… | Go to |
|---|---|
| Try it on this machine | [Installing and first run](/guide/start/getting-started) |
| Connect the Seafile desktop or mobile client | [Connecting clients](/guide/use/clients) |
| Upload, download, organize or share files | [Files and sharing](/guide/use/files-and-sharing) |
| Enable two-factor authentication or manage sessions | [Account security](/guide/use/account-security) |
| Put it behind HTTPS | [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy) |
| Start it at boot | [Running in the background](/guide/deploy/running-in-background) or [Windows service and desktop tray](/guide/deploy/desktop-and-service) |
| Back up, restore or upgrade | [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) |
| Turn on people tags or OCR | [Automatic people tags](/guide/use/people-tags) or [Text in images and PDFs](/guide/use/ocr) |
| Diagnose a problem | [Troubleshooting](/guide/operate/troubleshooting) |

## How this guide is organized

- **Start here** — evaluate, choose a shape and install.
- **Using Nanofile** — the web interface, files and sharing, clients, account security
  and the optional media features.
- **Deployment** — Docker, system services, the Windows service and the HTTPS proxy.
- **Configuration** — the configuration file, environment variables, the settings
  pages, the data directory and email.
- **Administration and security** — users, quotas, sharing policy, the master secret
  and the sandbox.
- **Operations** — backup, upgrade, maintenance, monitoring and troubleshooting.

## Reference and development

- [API reference](/reference/api/) — every HTTP endpoint, and how it compares with
  Seafile's.
- [CLI reference](/reference/cli) — every operator command.
- [Development](/dev/architecture) — architecture, build, tests and CI.
