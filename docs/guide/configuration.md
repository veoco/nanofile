# Configuration

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
2. The config file.
3. The value saved at "System Settings".
4. The built-in default.

Environment variables always win and are never written back to the file, which is
what container deployments rely on.

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
| `server.secret_key` | The master key. File-only; see [Security](/guide/security). |
| `storage.block_dir`, `storage.temp_dir`, `storage.thumbnail_dir`, `storage.avatar_dir`, `index.index_dir` | State directories. Relative paths resolve against the binary's directory; absolute paths place state elsewhere. |
| `storage.block_encryption_mode` | `off`, `on` or `lazy`. File-only: it decides how existing ciphertext is read. |

Storage caps: open resumable uploads may declare 32 GiB in total and one archive
may build 8 GiB by default. Setting either limit to `0` on the Storage page (or in
`[storage]`) removes that cap.

## When a change takes effect

Most settings apply immediately. The listener, transfer limits and cache
directories apply at the next start. Every settings row says which case it is.

The **Restart server** button in the settings page's header restarts the server in
place: the process, the tray icon and the listening port stay the same, but the
settings table, database connections, caches and background tasks are rebuilt, so
every "needs a restart" row takes effect immediately. The administrator's session
survives.

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
  (or restored from a backup) is picked up without a restart. `0` disables it.

## The task pages

"System Settings → Task Management" is two pages.

- **Task list** puts the server's load measurements first (requests in flight,
  jobs running, queued tasks, database connections, workers busy, live tasks),
  then the runs themselves: what is running now and what the server actually did.
  The finished list holds runs with something to report — a failure, or work that
  was done — so a cleanup pass that found nothing to do does not bury them. It can
  be narrowed to one job and to what succeeded or failed. Each row names the job
  and its verdict, the account that submitted it, when it finished and how long it
  took. A failed run carries the job's own error; the run id, summary and full
  error are one disclosure away.
- **Registered tasks** lists every job and long-lived service this server has
  registered, grouped by how it runs. Each row says when it runs and what it does,
  and carries the same right-aligned columns for the job's last run, read back
  from the history. Opening a row shows lifetime counters and the policies the job
  runs under: priority, concurrency cap, timeout, retry, whether it waits for a
  quiet server, how finely it can be interrupted, and what a crash leaves behind.
  A job or service whose subsystem is switched off is named with the switch that
  would bring it back, and a run can be triggered by hand where that means
  something.

## Sections

`[server]` networking and feature switches; `[database]` connection;
`[storage]` directories and quotas; `[auth]` login, password and rate limits;
`[ui]` language; `[email]` mail; `[admin_init]` first-start admin;
`[logging]` logs; `[gc]` garbage collection; `[index]` search;
`[sandbox]` the confinement over untrusted parsing; `[notification]`
notifications; `[tasks]` background tasks; `[settings]` how the file and the
saved settings layer.

A key the file does not mention keeps its built-in default. The file is never
rewritten, so what you write is what stays on disk. The one exception is
`[logging] file`, which a desktop run resolves next to the binary and records
there so every later run uses the same file.

## Related

- [Web interface](/guide/web-interface) — where the settings and task pages sit.
- [Maintenance and troubleshooting](/guide/maintenance) — when the logging,
  garbage-collection and index settings matter.
- [Running in the background](/guide/running-in-background) — how a service gets
  its configuration.
- [Command line](/reference/cli) — the operator commands.
