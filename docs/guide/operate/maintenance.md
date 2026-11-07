---
description: Logs, garbage collection, the search index and block verification.
---
# Maintenance

**Who this is for:** operators doing routine maintenance.

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
before the process starts, and the in-place "Restart server" button in the admin
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

Image and scanned-PDF text is a separate opt-in (`[ocr] enabled`): its text
joins the same documents, so a content search finds the words in a picture or a
scan page. Enabling OCR later re-examines images and PDFs that were previously
skipped, without rebuilding the document part of the index; a mixed PDF already
indexed through its text layer needs a re-index to add its scan pages. Disabling
it stops new OCR but leaves existing text until the repository is re-indexed.
See [Text in images and PDFs](/guide/use/ocr).

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

## Task Management

Background jobs and load measurements are on **System Settings → Task Management**;
see [Monitoring](/guide/operate/monitoring) for what each page shows and what to
watch.

## Common problems

Start with the symptom list in [Troubleshooting](/guide/operate/troubleshooting). It
covers startup, clients and sync, search, thumbnails and media, email, uploads and the
reverse proxy.

## Related

- [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) — offline backup and
  changing versions.
- [Configuration](/guide/configure/configuration) — when each setting takes effect.
- [Monitoring](/guide/operate/monitoring) — health, tasks and load.
- [Troubleshooting](/guide/operate/troubleshooting) — when something breaks.
- [Command line](/reference/cli) — every operator command.
- [Sandbox](/guide/admin/sandbox) — what the parsing child processes are confined to.
