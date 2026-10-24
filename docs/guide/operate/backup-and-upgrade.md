# Backup, restore and upgrade

## What to back up

| Item | Required | Notes |
|---|---|---|
| The configuration file (or wherever your env vars come from) | **Yes** | It holds the master secret `server.secret_key` and, with at-rest encryption, `storage.encryption_key`. Lose it and sessions and sync tokens stop working, and encrypted blocks cannot be decrypted. |
| `nanofile.db` | **Yes** | Accounts, the library structure, share links, saved settings. |
| `blocks/` | **Yes** | The only copy of the file blocks; without it there is no file content at all. |
| `index/` | Optional | The full-text index; it can be rebuilt from `blocks/`. |
| `thumbnails/`, `avatars/` | Optional | Caches; both rebuild. |
| `temp/` | No | In-flight upload scratch space. |

Where the configuration and the data directory live is in
[Data directory](/guide/configure/data-directory).

## Offline backup (simplest)

```bash
sudo systemctl stop nanofile          # or: docker stop nanofile
cp -a /opt/nanofile/config.toml /backup/
cp -a /opt/nanofile/data /backup/
sudo systemctl start nanofile
```

With the service stopped, a straight copy gives you a database and a block store
from the same instant; WAL and ordering stop mattering.

## Online backup

Backing up while the server runs, two things matter:

- **Do not copy the database file directly.** It runs in WAL mode, and an
  un-merged `nanofile.db-wal` leaves the copy at an older state. Use SQLite's own
  backup command (it needs `sqlite3` on the host):

  ```bash
  sqlite3 /opt/nanofile/data/nanofile.db ".backup /backup/nanofile.db"
  ```

- **Order: copy `blocks/` first, then back up the database.** Writes continue
  during the backup, so blocks first and database second can only leave you with
  a few unreferenced blocks (garbage collection cleans those up); the other way
  round you can end up with a database that references blocks you never copied.

## Restore

1. **Stop the service.**
2. **Put the configuration file back**, with the same master secret as when the
   backup was taken. A rotated secret makes old sessions and encrypted blocks
   unusable.
3. **Put the database back** (the file you backed up; the server handles its
   journal on startup).
4. **Put `blocks/` back.**
5. **Check permissions**: the user running the service has to read and write
   these files; in a container that is uid/gid `1000:1000`
   (`chown -R 1000:1000 data`).
6. **Start it and read the log** — where the log is, is in
   [Maintenance and troubleshooting](/guide/operate/maintenance).
7. **Optionally verify**: `nanofile verify-blocks` checks every block against its
   SHA-1, see [Command line](/reference/cli).

`index/` does not have to match: if an index ends up next to a different block
store, emptying the index directory and letting it rebuild is the easy way out.

**With at-rest encryption:** `blocks/` depends on the key in the configuration
file and on `storage.block_encryption_mode`; the two have to be backed up and
restored together. See [Security](/guide/admin/security).

## Upgrade

1. **Back up first.**
2. **Stop the service.**
3. **Replace the binary**: unpack the new archive over the old file; for a
   container, change the image tag and recreate the container.
4. **Start it** and confirm the version (bottom of the left rail) and that the
   log is clean.
5. **Watch the tasks page**: **System Settings → Tasks** lists the migration
   jobs an upgrade runs by itself.

Worth knowing:

- **Do not count on downgrading.** Database migrations only move forward, and an
  older version may refuse to open a newer database — which is why step 1 is not
  optional.
- **Renamed settings migrate themselves.** The old key names are still read, and
  saved values move to the new keys on upgrade; you do not edit the configuration
  file by hand.
- **For a service**: `systemctl restart` after replacing the binary, see
  [Running in the background](/guide/deploy/running-in-background); a Windows service is
  stopped and replaced the same way, see
  [Desktop tray and Windows service](/guide/deploy/desktop-and-service).
- **For a container**: `docker compose pull && docker compose up -d`, leaving the
  volume alone; if permissions are wrong afterwards, run
  `chown -R 1000:1000 data` first, see [Docker](/guide/deploy/docker).

## Release channels

| Tag | Meaning |
|---|---|
| `ghcr.io/veoco/nanofile:latest` | the newest release on the default branch |
| `ghcr.io/veoco/nanofile:vX.Y.Z`, `vX.Y` | a pinned version — recommended for production |
| `ghcr.io/veoco/nanofile:edge` | nightly, built daily at 10:00 UTC from the latest commit, with changes that are not released yet |

Prebuilt archives exist for releases only (`vX.Y.Z`); there is no `edge`
equivalent.

## Related

- [Data directory](/guide/configure/data-directory) — what lives in each directory.
- [Maintenance and troubleshooting](/guide/operate/maintenance) — logs, garbage
  collection, verification commands.
- [Security](/guide/admin/security) — the master secret and at-rest encryption.
- [Command line](/reference/cli) — `verify-blocks`, `verify-dir-order`,
  `migrate-blocks`.
