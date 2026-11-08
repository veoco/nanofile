---
description: Watch Nanofile's health, background tasks, load, disk space and backups.
---
# Monitoring

Nanofile exposes a small readiness endpoint and a task console; it does not ship a
Prometheus endpoint. This page covers what to watch and where to look.

**Who this is for:** anybody running Nanofile as a service or container. **Before you
start:** the server should be running with logs going somewhere you can read.

## Health endpoint

`GET /health` is the readiness check. It answers `200 OK` with an empty body and an
`x-nanofile-generation` header. Use it for:

- a container healthcheck;
- a load balancer or reverse-proxy upstream check;
- a script that waits for the server to come back after a restart.

The generation header changes on every in-place **Restart server**. That is what the
admin restart page watches: a plain 200 cannot tell the old process from the new one.
A healthcheck should require a 200, not a particular generation.

```bash
curl -fsS http://127.0.0.1:8082/health >/dev/null && echo ok
```

## Logs

`[logging]` decides where the log goes and how much is kept:

| Key | Effect |
|---|---|
| `logging.level` | the log level |
| `logging.file_enabled` | write to a file instead of stdout |
| `logging.file` | the log file path |
| `logging.max_file_size_mb` | the cap on a single log file |
| `logging.max_backups` | how many older files are kept |

- A desktop (tray) run logs to a file and records the resolved path in `[logging]
  file`.
- A headless run logs to stdout; read it with `journalctl -u nanofile -f` or the
  container log.
- A Windows service logs to `nanofile.log` next to the binary.

Log level and file caps are decided before the process starts. The in-place
**Restart server** button does not redo that step; only stopping and starting the
process applies them.

## Task Management

**System Settings → Task Management** is two pages:

- **Task list** puts the server's load measurements first (requests in flight, jobs
  running, queued tasks, database connections, busy workers, live tasks), then the
  runs themselves: what is running now and what the server actually did. The
  finished list keeps runs with something to report, so a cleanup pass that found
  nothing to do does not bury them. Filter by job and by success/failure.
- **Registered tasks** lists every job and long-lived service, grouped by how it
  runs. Each row says when it runs and what it does, and carries the last run read
  back from the history. Open a row for lifetime counters and the policies: priority,
  concurrency cap, timeout, retry, whether it waits for a quiet server, how finely it
  can be interrupted, and what a crash leaves behind.

After an upgrade, this is where you confirm the migration tasks finished. A failed
run carries its own error; the run id, summary and full error are one disclosure away.

## Load and disk

- The top of **Task Management → Task list** is the quickest read of server load:
  in-flight requests, running jobs, queued tasks, database connections, busy workers
  and live tasks.
- The left rail's storage meter shows the signed-in account's usage against its
  quota. If a size looks wrong after an upgrade, run the manual **Usage reconcile**
  job under **Task Management → Registered tasks**; it recomputes each library from
  its HEAD tree.
- The data directory grows with `blocks/`, the database, the index, thumbnails and
  avatars. Watch the filesystem, not just the database:

```bash
df -h /opt/nanofile/data
du -sh /opt/nanofile/data/*
```

- Deleting files does not free blocks immediately; garbage collection waits for
  `gc.min_block_age_secs` and for no commit to reference the block. See
  [Maintenance](/guide/operate/maintenance).
- The index is rebuildable; `index/` can be deleted and rebuilt if it grows or
  becomes inconsistent. `blocks/` and `nanofile.db` are not rebuildable.

## Backup verification

A backup is not a backup until it restores. Keep a schedule and test it:

1. Run the backup from [Backup, restore and upgrade](/guide/operate/backup-and-upgrade).
2. Restore it into a disposable data directory with the same master secret.
3. Start the restored instance on another port, sign in, download a file and run
   `nanofile verify-blocks` against the restored block store.
4. Record the date and the result outside the server, so a lost server does not lose
   the backup history too.

## What to alert on

- **`/health` failing** for more than a few checks.
- **Repeated task failures** on Task Management, especially migrations after an
  upgrade.
- **Disk usage** above 80% of the data filesystem.
- **Backup age** older than the schedule allows.
- **Certificate expiry** at the reverse proxy.
- **Mail queue** growing or stuck in failed state, if email is enabled.
- **Login lockouts or rate limiting** spiking, which can be a credential-stuffing
  attempt.

## Related

- [Logs and maintenance](/guide/operate/maintenance) — log settings, GC and
  verification commands.
- [Configuration](/guide/configure/configuration) — the settings page and how changes
  take effect.
- [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) — the backup
  procedure to verify.
- [Troubleshooting](/guide/operate/troubleshooting) — when an alert turns into an
  incident.
