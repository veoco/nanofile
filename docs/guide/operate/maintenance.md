# Maintenance and troubleshooting

The part you reach for in day-to-day running: reading the log, making disk space
actually come back, rebuilding the index, verifying blocks, and where to look
first when something misbehaves.

## Logs

`[logging]` decides where the log goes and how much of it is kept:

| Key | Effect |
|---|---|
| `logging.level` | the log level. |
| `logging.file_enabled` | write to a file instead of stdout. |
| `logging.file` | the log file path. |
| `logging.max_file_size_mb` | the cap on a single log file. |
| `logging.max_backups` | how many older files are kept. |

**The default is chosen for you**: desktop (tray) runs log to a file, headless
runs log to stdout. A desktop run also records the resolved path back into
`[logging] file`, so every later start uses the same file. A Windows service has
no desktop, and logs next to the binary in `nanofile.log`.

**Level and file caps are "needs a full process restart"**: they are decided
before the process starts, and the in-place "Restart service" button in the admin
console does not redo that step. Only stopping and starting the process applies
them.

Running in the background sends the log to stdout, so read it with
`journalctl -u nanofile -f` (systemd) or the container log; see
[Running in the background](/guide/deploy/running-in-background).

## Garbage collection

Deleting a file does **not** hand the disk back immediately. A block is collected
on the next GC run only when no commit references it any more **and** it is older
than `gc.min_block_age_secs`:

| Key | Effect |
|---|---|
| `gc.enabled` | whether garbage collection runs. |
| `gc.interval_hours` | how often it runs. |
| `gc.min_block_age_secs` | how long a block must exist before it is eligible — this is the buffer for a block that was just uploaded and is not referenced yet. |

All three are **"needs a restart"** rows: the **Restart server** button in the
settings page header applies them at once, because it rebuilds the settings
table, the caches and the background tasks in place. The rows that genuinely need
a full process restart are a different set (log level, log caps, tray), see
[Configuration](/guide/configure/configuration).

## The search index

| Key | Effect |
|---|---|
| `index.enabled` | whether full-text indexing runs. Off means search matches names only. |
| `index.index_dir` | the index directory. |
| `index.backfill_enabled` | whether existing content is backfilled into the index. |

These three are **"needs a restart"** rows as well, so the restart button applies
them.

A single file or a selection can be re-indexed from the web interface: the
**re-index** button in the details drawer, or the bulk **re-index** when several
entries are selected. A whole-site rebuild is `POST /api2/reindex/`.

Indexing depends on document extraction inside the sandbox (PDF, Office, images,
…). While the sandbox is off, documents are not indexed — they are recorded as
retryable and picked up once it is back on. See [Sandbox](/guide/admin/sandbox).

## Block and directory verification

All three commands are **read-only by default**; they only write with `--repair`:

| Command | When to use it |
|---|---|
| `nanofile verify-blocks` | when you suspect block damage (disk errors, an interrupted copy, or after a restore). It checks each block against its SHA-1 and that a file's block sizes sum to its declared size. |
| `nanofile verify-dir-order` | only when the same server is used with the official clients and a client reports a directory structure problem: it checks that directory objects are stored in the order Seafile's spec requires. |
| `nanofile migrate-blocks` | to move from the old "one block directory for every library" layout to per-library directories. Startup does this by itself; the command estimates the copy volume up front or runs it while stopped. |

`verify-blocks --repair` does not delete a bad block, it quarantines it under
`{block_dir}/.quarantine/` — the bytes are kept, and clients holding a good copy
re-upload it. Full options are in [Command line](/reference/cli).

## The tasks page

**System Settings → Tasks** is where you watch background jobs: what is running,
what has run, why a run failed, and when each registered job runs next. After an
upgrade, this is where you confirm the migration jobs finished. See
[Configuration](/guide/configure/configuration).

## Common problems

**It opens the setup page instead of serving.**
There is no configuration file and no `NANOFILE_*` variable, which is what "not
set up yet" means — not a fault. Either walk through the one-time page or supply a
configuration file. See [Installing and first run](/guide/start/getting-started).

**A release build refuses to start.**
`server.secret_key` is missing. Generate one and put it in the configuration file
(or in `NANOFILE_SERVER_SECRET_KEY`): `openssl rand -hex 32`. See
[Security](/guide/admin/security).

**The port will not bind.**
If something else holds it, change `server.port`. On Windows, connections that
were just closed sit in `TIME_WAIT` for minutes and a new process binding the same
port fails — which is why the Windows service takes over on the next boot.

**You forgot the administrator password.**
Create another administrator from the command line and fix things from inside:
`./nanofile adduser --email <new address>` (`--password-stdin` for
non-interactive). See [Command line](/reference/cli).

**A client cannot connect, or signs in but does not sync.**
Work through the checklist at the end of [Connecting clients](/guide/start/clients);
the usual cause is a server address that differs from `server.site_url`.

**Search does not find something just uploaded.**
Check that `index.enabled` is on, then re-index that file once. Indexing is
asynchronous, so large files appear a little later.

**Thumbnails or previews do not work.**
With `[sandbox] enabled` off, those features do not run at all (and do not fall
back to parsing inside the server process); media thumbnails also need `ffmpeg`,
whose path is `storage.ffmpeg_path`. See [Sandbox](/guide/admin/sandbox).

**Mail is not going out.**
See [Email notifications](/guide/configure/email): check that `enabled` is on and the SMTP
fields are right, then read the failure reason on the tasks page.

**Uploads fail.**
In order: the account quota
([Users and sharing](/guide/admin/users-and-sharing)), a request-body limit in a proxy
or client, and the storage caps (in-flight resumable uploads and a single zip both
have default caps; set them to `0` to remove them, see
[Configuration](/guide/configure/configuration)).

**Everybody looks like one IP behind a proxy.**
Set `trusted_proxies`, or `X-Forwarded-For` / `CF-Connecting-IP` are ignored
outright and IP rate limits collapse into a single bucket. See
[Security](/guide/admin/security).

## Related

- [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) — offline backup and
  changing versions.
- [Configuration](/guide/configure/configuration) — when each setting takes effect, and the
  tasks page.
- [Command line](/reference/cli) — every operator command.
- [Sandbox](/guide/admin/sandbox) — what the parsing child processes are confined to.
