---
description: What Nanofile is, how far Seafile compatibility goes, and what it does not do.
---
# What is Nanofile?

Nanofile is a self-hosted file sync and sharing server in a single Rust binary.
It implements the Seafile sync protocol and the HTTP APIs the official desktop
and mobile clients use, and it serves its own web interface. There is no separate
seaf-server + seahub stack to install.

**Who this is for:** people who want the official Seafile clients with a simpler
server, a small team, or a home lab. **Who it is not for:** a deployment that needs
every Seafile feature, a multi-node cluster, or a drop-in import of an existing
Seafile data directory.

## What it is

- **One binary.** The server, the web interface and the background workers are the
  same executable. There is no PHP, Python or Java runtime.
- **Seafile clients.** The official desktop and mobile clients connect directly and
  keep libraries in sync.
- **Web interface.** Browse, preview, upload, download, star, search, follow the
  activity feed and restore from the trash in the browser.
- **Sharing.** Password-protected share links with an expiry and a view count, plus
  anonymous upload links into a library.
- **Accounts and security.** Two-factor authentication, trusted devices, login
  lockout, sessions, sync tokens and API keys.
- **Admin console.** Users, quotas, registration, email, sharing policy, system
  settings and background tasks.
- **Confined parsing.** Document, image, EXIF, media, face detection and OCR parsing
  runs in bounded child processes, not inside the server process.
- **Optional media features.** Automatic people tags and OCR for images and scanned
  PDFs, both off by default.

## Seafile compatibility

Compatibility is a route-level and protocol-level promise, not a byte-for-byte
promise for every feature. The authoritative list is the
[Seafile API compatibility](/reference/api/compatibility) page; the protocol rules
are in [Seafile compatibility](/dev/protocol).

### What works

- The official Seafile desktop and mobile clients sign in and sync.
- The sync protocol, the legacy `/api2/` surface and the `/api/v2.1/` surface used by
  the clients.
- WebDAV, share links, upload links, API keys, quotas and two-factor authentication.
- The web interface and the admin console.

### What is not implemented

- A full seahub replacement: the Seafile admin UI, plugins and every Seafile Pro
  feature are not reproduced. Use the compatibility page for the exact gaps.
- Per-folder sharing. A library is the sharing unit.
- Wiki, SeaDoc and the Seafile OCR metadata endpoints.
- A Seafile data-directory import. The compatibility page compares HTTP APIs, not
  storage; plan on re-syncing or re-uploading into a fresh Nanofile instance.

## Core concepts

| Term | Meaning |
|---|---|
| **Library** | The unit of storage, sharing and permissions. The Seafile clients call it a repository. |
| **Sync** | A client keeps a local copy of a library in step with the server through the Seafile sync protocol. |
| **Block** | File content is stored as content-addressed blocks; the database holds the library and file structure. |
| **Commit** | A snapshot of a library's file tree, with history and a commit id compatible with Seafile. |
| **Share link / upload link** | A public link that lets somebody download from, or upload into, a library without an account. |
| **API key / WebDAV key** | A scoped credential for scripts, third-party tools or a WebDAV mount. It is not the account password. |
| **Encrypted library** | A client-side encrypted library. The server never receives the library password. |
| **Full-text index** | Extracted document text, stored next to the data directory and searchable from the web interface. |
| **Sandbox** | The confined child processes that parse untrusted content. |

## Limitations

- **One process, one SQLite database.** The current architecture has no built-in
  clustering or multi-node mode; the guide assumes one server per data directory.
- **Encrypted libraries are client-side.** The web interface cannot preview or
  download them, and anonymous upload links cannot target them.
- **WebDAV does not serve encrypted libraries**, because WebDAV cannot carry the
  library password.
- **OCR and people tags are optional and bounded.** Both need the sandbox, and both
  have accuracy and format limits; see their pages.
- **Seafile feature gaps are real.** Do not assume an endpoint or a UI feature exists
  because Seafile has it; check the compatibility page.
- **No Seafile data-directory import.** Moving from Seafile means re-syncing or
  re-uploading files into Nanofile.

## Requirements

- A Linux, macOS or Windows release archive, or the published Docker image. See
  [Choose a deployment](/guide/start/choose-deployment).
- A data directory the server user can read and write. Its default is `data/` next
  to the binary.
- A master secret. A release build refuses to start without one.
- For HTTPS, a reverse proxy in front of the server; see
  [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy).
- Optional: `ffmpeg` for media thumbnails and for decoding HEIC/HEIF/AVIF before
  face detection and OCR.

## Who it is for

Nanofile fits when you want the Seafile clients, a small self-hosted footprint and
a web interface without operating seaf-server and seahub. It does not fit when you
need Seafile Pro features, per-folder sharing, a multi-node cluster, or a supported
in-place migration from an existing Seafile installation.

## Next steps

- [Choose a deployment](/guide/start/choose-deployment) — desktop tray, headless
  server or Docker.
- [Installing and first run](/guide/start/getting-started) — the end-to-end tutorial.
- [Web interface](/guide/use/web-interface) — what you can do once you are signed in.
- [Seafile API compatibility](/reference/api/compatibility) — the endpoint-by-endpoint
  comparison.
