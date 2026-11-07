---
layout: home

hero:
  name: Nanofile
  text: A wire-compatible Seafile server
  tagline: Self-hosted file sync and sharing in a single Rust binary.
  actions:
    - theme: brand
      text: Get started
      link: /guide/start/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/veoco/nanofile

features:
  - icon: 🔄
    title: Seafile-compatible sync
    details: The official Seafile desktop and mobile clients connect directly, with no separate seaf-server and seahub stack.
  - icon: 🖥️
    title: Web interface
    details: Browse, preview, download, star, follow the activity feed and restore from the trash in the browser.
  - icon: 🔗
    title: Sharing
    details: Share links with a password, an expiry and a view count, plus anonymous upload links into a library.
  - icon: 🔐
    title: Sign-in security
    details: TOTP two-factor authentication, trusted devices, login lockout and API keys for third-party tools.
  - icon: ⚙️
    title: Admin console
    details: Users, quotas, email delivery, system settings and background tasks, all from the web interface.
  - icon: 🧱
    title: Confined parsing
    details: Every parser that reads user content runs in a separate process with memory, CPU and time bounds.
  - icon: 👤
    title: Automatic people tags
    details: Optional local face detection tags photos as 人物, 单人 or 合影. Only a count is stored — no identity or biometric template.
---

## Start here

- [What is Nanofile?](/guide/start/what-is-nanofile) — features, Seafile compatibility, limitations.
- [Choose a deployment](/guide/start/choose-deployment) — desktop tray, headless server or Docker.
- [Installing and first run](/guide/start/getting-started) — install, create the first administrator, upload and share a file, connect a client.

Then read [Using Nanofile](/guide/use/web-interface) for the web interface and clients, [Deployment](/guide/deploy/docker) for production, or [Operations](/guide/operate/backup-and-upgrade) for backup and upgrades. The full map is the [guide](/guide/).

- [API reference](/reference/api/) — every HTTP endpoint, and how it compares with Seafile's.
- [CLI reference](/reference/cli) — every operator command.
- [Development](/dev/architecture) — crate layout, testing and CI.
