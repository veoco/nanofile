---
description: How the configuration file, environment variables and settings pages layer.
---
# Configuration

**Who this is for:** administrators changing settings by file or environment variable.

Every setting has a built-in default. There are two places to change one:

- **The web interface.** "System Settings → Settings" renders every setting, one
  page per area (Server, Security, Authentication, Rate limits, Storage,
  Encryption, Maintenance, Sandbox, Email, Notifications, Advanced). Pages are
  filterable by name, config key, description or `NANOFILE_*` variable.
- **`config.toml` and `NANOFILE_*` environment variables.** Copy
  [`config.toml.example`](https://github.com/veoco/nanofile/blob/main/config.toml.example).
  The template deliberately holds only what a settings page cannot store at all
  (the master secret, the database URL, the at-rest encryption mode, the
  first-start admin, the log file) plus the few values that have to be decided
  before the interface is reachable (external URL, listener, state directories,
  language).

The settings pages are the complete reference: every row says what the setting
does, which `NANOFILE_*` variable overrides it, where the current value came
from, and whether changing it needs a restart. This page explains the layering
and the keys you are most likely to set by hand.

## Precedence

Highest first:

1. An environment variable (`NANOFILE_*`).
2. The value saved at "System Settings", for a key that has one.
3. The config file — for keys with no saved value, plus every key listed in
   `config_override_keys`, or every key at all under `config_policy = "override"`.
4. The built-in default.

Environment variables always win and are never written back to the file, which is
what container deployments rely on. Under the default
`config_policy = "bootstrap"`, a saved value beats the file: once an administrator
saves a key in the interface, the file's value for that key stops mattering. See
the `[settings]` layer below.

## Where the config file is looked for

First match wins:

1. `--config <path>`
2. `NANOFILE_CONFIG`
3. `./config.toml` (the historical default, kept so an existing deployment keeps
   reading the file it always did)
4. `~/.nanofile/config.toml` — the per-user file the first-run setup writes

Any `NANOFILE_*` environment variable counts as configuring the installation, so
it suppresses the first-run setup even when no file exists. The exceptions are
the variables that decide nothing about the configuration: `NANOFILE_DOTENV`,
`NANOFILE_TEST_FFMPEG_PATH` and `NANOFILE_BUILD_TS`.

Naming a config file that does not exist (with `--config` or `NANOFILE_CONFIG`) is
a refused start, not a silent fall back to built-in defaults — which would have
quietly served a different database on a different port.

## Frequently changed keys

| Key | Meaning |
|---|---|
| `server.site_url` | The external URL (HTTPS domain). Controls the `Secure` cookie attribute, enables HSTS and builds share links. |
| `server.addr` / `server.port` | Listener. Default `0.0.0.0:8082`. |
| `server.secret_key` | The master key. File-only; see [Security](/guide/admin/security). |
| `storage.block_dir`, `storage.temp_dir`, `storage.thumbnail_dir`, `storage.avatar_dir`, `index.index_dir` | State directories. Relative paths resolve against the binary's directory; absolute paths place state elsewhere. Compatibility exception: if a relative path in this group already exists under the current working directory, the whole relative group is kept as configured, so an older deployment keeps its data where it is. |
| `storage.block_encryption_mode` | `off`, `on` or `lazy`. File-only. Turning it on converts the existing blocks once; `on` without the completion marker starts in that conversion window and finishes it in the background. See [At-rest block encryption](/guide/admin/security#at-rest-block-encryption). |
| `storage.block_flush` | `os` (the default) leaves flushing to the OS, as Seafile does. `always` fsyncs every block, so each upload waits for a device-level flush and concurrent clients can stall for minutes. File-only. |

Storage caps: open resumable uploads may declare 32 GiB in total and one archive
may build 8 GiB by default. Setting either limit to `0` on the Storage page (or in
`[storage]`) removes that cap.

`server.max_chunk_size_mb` caps one resumable upload chunk (default 32 MiB). The
official clients send 4 MiB chunks; the cap is what keeps one request from
buffering an arbitrary amount of memory. Raise it only for a client that really
sends larger chunks.

## When a change takes effect

Most settings apply immediately. The listener, most transfer and request limits
and cache directories apply at the next start; a few limits, such as
`server.max_chunk_size_mb`, are live. Every settings row says which case it is.

The **Restart server** button in the settings page's header restarts the server in
place: the process and the tray icon stay the same, and the listening socket is
kept only when the saved address and port did not change. A changed address is
rebound; if the new one cannot be bound, the settings page shows a banner and the
old socket keeps serving. The settings table, database connections, caches and
background tasks are rebuilt, so every "needs a restart" row takes effect
immediately. The administrator's session survives.

The log level and file caps, and whether the desktop tray exists and in which
language, are decided before the process starts. Those are marked "needs a full
process restart" and only apply by stopping and starting the process.

## The `[settings]` layer

`config.toml.example` has a `[settings]` section that decides what a saved
setting means. It is not editable from the interface.

- `config_policy = "bootstrap"` (the default) makes the file a first-start seed:
  a value in the file supplies a key with no saved value, and stops mattering for
  that key once an administrator saves one. Environment variables still win.
- `config_policy = "override"` makes the file win for every key it sets (below
  the environment).
- `config_override_keys` lists keys the file keeps winning for under
  `"bootstrap"`.
- `refresh_interval_secs` re-reads the saved settings so a change made elsewhere
  (or restored from a backup) is picked up without a restart — for the settings
  that apply immediately. A row that needs a restart still needs one. `0` disables
  the periodic read.

## Task Management

**Task Management** — in the user menu, alongside System Settings — shows the
server's load measurements, the background runs and the registered jobs. See
[Monitoring](/guide/operate/monitoring) for what each page shows and what to
watch.

## Sections

`[server]` networking and feature switches; `[database]` connection;
`[storage]` directories and quotas; `[auth]` login, password and rate limits;
`[ui]` language; `[sync]` sync-protocol hardening (`verify_fs_objects`,
`max_tree_depth`, `max_tree_visits`); `[email]` mail; `[admin_init]` first-start
admin; `[logging]` logs; `[gc]` garbage collection; `[index]` search;
`[face_tags]` automatic people tags; `[ocr]` text in images and scanned PDFs for
the search index; `[sandbox]` the confinement over untrusted parsing;
`[notification]` notifications; `[tasks]` background tasks; `[settings]` how the
file and the saved settings layer.

A key the file does not mention keeps its built-in default. The file is never
rewritten, so what you write is what stays on disk. The one exception is
`[logging] file`, which a desktop run resolves next to the binary and records
there so every later run uses the same file.

## Related

- [Web interface](/guide/use/web-interface) — where the settings pages sit.
- [Monitoring](/guide/operate/monitoring) — Task Management, load and disk.
- [Maintenance](/guide/operate/maintenance) — when the logging,
  garbage-collection and index settings matter.
- [Running in the background](/guide/deploy/running-in-background) — how a service gets
  its configuration.
- [Command line](/reference/cli) — the operator commands.
