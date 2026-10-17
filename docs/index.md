---
layout: home

hero:
  name: Nanofile
  text: A wire-compatible Seafile server
  tagline: Self-hosted file sync and sharing in a single Rust binary.
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
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
---

## Start here

- [Getting started](/guide/getting-started) — build the binary and run the first-run setup.
- [Configuration](/guide/configuration) — config file, `NANOFILE_*` variables and the settings pages.
- [Docker](/guide/docker) — run the published image.
- [Sandbox](/guide/sandbox) — what the server does and does not confine.
- [CLI reference](/reference/cli) — every operator command.
- [Development](/dev/architecture) — crate layout, testing and CI.
