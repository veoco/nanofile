---
description: Back up the database and blocks, restore them, and upgrade safely.
---
# Backup, restore and upgrade

**Who this is for:** operators responsible for the data and the version. **Before you start:** a backup destination, and permission to stop the service.

## What to back up

| Item | Required | Notes |
|---|---|---|
| The configuration file (or wherever your env vars come from) | **Yes** | It holds the master secret `server.secret_key`. Lose it and sync clients stop working, and TOTP seeds, queued mail and saved setting secrets cannot be decrypted; a changed secret also signs every session out at the next start. With at-rest encryption the block key is `storage.encryption_key`: it is never written to the file and comes only from `NANOFILE_STORAGE_ENCRYPTION_KEY`/`_FILE` (back that source up separately), or, when unset, is derived from `server.secret_key`. |
| `nanofile.db` | **Yes** | Accounts, the library structure, share links, saved settings. |
| `blocks/` | **Yes** | The only copy of the file blocks; without it there is no file content at all. |
| `index/` | Optional | The full-text index; it can be rebuilt from `blocks/`. |
| `thumbnails/` | Optional | Thumbnail cache; it rebuilds from `blocks/`. |
| `avatars/` | **Recommended** | Uploaded avatar originals — the only copy, held nowhere else (not in `blocks/` or the database). Derived sizes rebuild from them while they exist, but once this directory is lost the originals cannot be recovered; each user has to upload again. |
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

If at-rest encryption is on and you supplied the key through
`NANOFILE_STORAGE_ENCRYPTION_KEY` or `NANOFILE_STORAGE_ENCRYPTION_KEY_FILE`, back
up that source as well: the key is never written into `config.toml`, so a copy of
the configuration alone leaves the blocks unreadable.

## Online backup

Backing up while the server runs, two things matter:

- **Do not copy the database file directly.** It runs in WAL mode, and an
  un-merged `nanofile.db-wal` leaves the copy at an older state. Use SQLite's own
  backup command (it needs `sqlite3` on the host):

  ```bash
  sqlite3 /opt/nanofile/data/nanofile.db ".backup /backup/nanofile.db"
  ```

- **Order: back up the database first, then copy `blocks/`.** The SQLite snapshot
  is consistent as of the moment it starts, and an upload writes its blocks
  before the commit that references them. A snapshot taken first therefore only
  references blocks already on disk, and copying the block store afterwards
  includes all of them; blocks written in between are unreferenced garbage that
  garbage collection cleans up. The reverse order is the risky one: a commit made
  between the block copy and the database snapshot can reference a block the copy
  never saw, and the restored database then points at missing blocks.

## Restore

1. **Stop the service.**
2. **Put the configuration file back**, with the same master secret as when the
   backup was taken. A rotated secret signs every session out at the next start
   (the server records a fingerprint of the key), breaks sync clients and makes
   TOTP seeds, queued mail, saved setting secrets and — when the block key was
   derived from it — encrypted blocks unusable.
3. **Put the database back** (the file you backed up; the server handles its
   journal on startup).
4. **Put `blocks/` back.**
5. **Check permissions**: the user running the service has to read and write
   these files; in a container that is uid/gid `1000:1000`
   (`chown -R 1000:1000 data`).
6. **Start it and read the log** — where the log is, is in
   [Maintenance](/guide/operate/maintenance).
7. **Optionally verify**: `nanofile verify-blocks` checks every block against its
   SHA-1, see [Command line](/reference/cli).

`index/` does not have to match: if an index ends up next to a different block
store, emptying the index directory and letting it rebuild is the easy way out.

**With at-rest encryption:** `blocks/` depends on the block key and on
`storage.block_encryption_mode`; the two have to be backed up and restored
together. The key is never written into `config.toml`: it comes from
`NANOFILE_STORAGE_ENCRYPTION_KEY`/`_FILE`, and when neither is set it is derived
from `server.secret_key`, so restoring the master secret restores it. See
[Security](/guide/admin/security).

## Upgrade

1. **Back up first.**
2. **Stop the service.**
3. **Replace the binary**: unpack the new archive over the old file; for a
   container, change the image tag and recreate the container.
4. **Start it** and confirm the version (bottom of the left rail) and that the
   log is clean.
5. **Watch the log**: database migrations and any block-layout migration run
   synchronously at startup, before the server starts listening; if one fails,
   the start fails and the log says why. Task Management lists background jobs
   (such as the lazy at-rest encryption conversion), not migrations. See
   [Monitoring](/guide/operate/monitoring).

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
  [Windows service and desktop tray](/guide/deploy/desktop-and-service).
- **For a container**: `docker compose pull && docker compose up -d`, leaving the
  volume alone; if permissions are wrong afterwards, run
  `chown -R 1000:1000 data` first, see [Docker](/guide/deploy/docker).

## Verify

- `nanofile verify-blocks` passes on the restored block store.
- The restored instance serves the same version and a test download.
- The startup log shows the migrations finished without error.

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
- [Maintenance](/guide/operate/maintenance) — logs, garbage
  collection, verification commands.
- [Security](/guide/admin/security) — the master secret and at-rest encryption.
- [Command line](/reference/cli) — `verify-blocks`, `verify-dir-order`,
  `migrate-blocks`.
