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

Garbage collection is **off by default** (`gc.enabled = false`): until it is
enabled, nothing is reclaimed and total disk usage only grows. With it on,
deleting a file does **not** hand the disk back immediately: a block is collected
on the next GC run only when no commit references it any more **and** it is older
than `gc.min_block_age_secs`:

| Key | Effect |
|---|---|
| `gc.enabled` | whether garbage collection runs (default `false`). |
| `gc.interval_hours` | how often it runs. |
| `gc.mode` | how a pass finds orphaned blocks: `audit` lists every library and diffs it against what its retained commits reference; `incremental` deletes only what history pruning just orphaned and needs no listing. |
| `gc.pending_ttl_hours` | how long a block an upload wrote but never committed is kept. It must be at least the resumable-upload lifetime; `0` disables the block sweep, leaves those blocks to an audit, and still drops the ledger rows after 30 days. |
| `gc.audit_interval_hours` | in `incremental` mode, how often each library's own full audit is due. A pass audits the libraries whose turn has come and prunes the rest incrementally. `0` disables the automatic audit. |
| `gc.min_block_age_secs` | how long a block must exist before it is eligible — this is the buffer for a block that was just uploaded and is not referenced yet. |

`audit` is the default and the only mode that finds blocks a crashed upload left
behind; it costs one listing per library per run. `incremental` exists for storage
backends where a listing is charged per request: it still prunes history and still
removes the prefix of a library that no longer exists, and each library is fully
audited every `gc.audit_interval_hours` — a pass audits the libraries whose turn has
come — so a crashed upload's blocks are still collected eventually. An upload's
blocks are recorded when they are written, so a restart in the middle of an upload
no longer hides them from the next pass; they are swept once
`gc.pending_ttl_hours` has passed without a commit.
A library with write activity in the last hour is skipped for that pass — the
scheduled job and a manual `nanofile gc` both read the upload ledger for that
signal, so a separate process applies the same protection. One library whose
stored objects cannot be read, or whose retained history references objects that
are not stored yet, is skipped and reported while the rest of the pass continues.
Run `nanofile gc --dry-run` to see what a pass would remove without touching
anything.

All six are **"needs a restart"** rows: the **Restart server** button in the
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
entries are selected. `POST /api/reindex/` rebuilds a single library: its body is
`{"repo_id": ...}` and only the library owner or an administrator may call it.
There is no whole-site endpoint; rebuild each library in turn, or set
`index.backfill_enabled` and restart the server to backfill every library.

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

`verify-blocks` and `verify-dir-order` are **read-only by default** and only write
with `--repair`. `migrate-blocks` is the opposite: it migrates straight away, and
only `--dry-run` makes it report without writing:

| Command | When to use it |
|---|---|
| `nanofile verify-blocks` | when you suspect block damage (disk errors, an interrupted copy, or after a restore). It checks each block against its SHA-1 and that a file's block sizes sum to its declared size. |
| `nanofile verify-dir-order` | only when the same server is used with the official clients and a client reports a directory structure problem: it checks that directory objects are stored in the order Seafile's spec requires. |
| `nanofile migrate-blocks` | to move from the old "one block directory for every library" layout to per-library directories. Startup does this by itself; the command estimates the copy volume up front or runs it while stopped. |
| `nanofile convert-blocks` | to finish or check the at-rest block encryption conversion. The `lazy` mode job converts in the background; this command completes it with the service stopped, or only reports with `--status`. `on` mode is refused until the completion marker exists. |

`verify-blocks --repair` does not delete a bad block, it quarantines it under
`{block_dir}/.quarantine/` — the bytes are kept, and clients holding a good copy
re-upload it. Full options are in [Command line](/reference/cli).

## Task Management

Background jobs and load measurements are on **Task Management** — in the user
menu, next to System Settings; see [Monitoring](/guide/operate/monitoring) for
what each page shows and what to watch.

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
