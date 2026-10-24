# Data directory

State lives under the binary's directory in `data/` by default, since relative
paths resolve against that directory. The `[storage]`, `[index]` and `[database]`
sections of `config.toml` can name absolute paths instead and put each part
elsewhere.

## What the first-run setup decides

The first-run setup decides this once:

- it suggests `~/.nanofile/data` (next to the configuration file it is about to
  write),
- it offers `data/` next to the binary when that directory already exists, so an
  installation whose configuration file was deleted keeps its state,
- it records the result as **absolute** paths in `~/.nanofile/config.toml`.

Relative paths in a configuration file you write yourself keep resolving against
the binary's directory.

## Layout

```
data/
├── nanofile.db        # database (WAL mode, mode 0600)
├── nanofile.db-wal    # WAL journal
├── blocks/            # file blocks: repos/{sha1(repo_id)}/{2-hex prefix}/{40-hex SHA-1}
├── temp/              # upload staging
├── thumbnails/        # thumbnail cache
├── avatars/           # avatars
└── index/             # full-text search index
```

The parts are not alike, which matters when you back up or clean up:

| Item | Losing it | Rebuildable |
|---|---|---|
| `nanofile.db` | accounts, the library structure, share links and settings are gone | no |
| `blocks/` | all file content is gone | no |
| `index/` | search matches names only | yes, by re-indexing |
| `thumbnails/`, `avatars/` | thumbnails and avatars regenerate | yes |
| `temp/` | nothing (upload staging) | does not need backing up |

They are set by `block_dir`, `temp_dir`, `thumbnail_dir` and `avatar_dir` under
`[storage]`, `index_dir` under `[index]`, and `[database] url`.

## Moving one

**Stop the server first.** A directory location takes effect on the next start, so
moving it while the server runs only means it keeps writing to the old place.

1. Stop the service.
2. Move the directory (`mv /opt/nanofile/data /srv/nanofile-data`).
3. Point the corresponding key at the new **absolute** path — a relative one
   resolves against the binary's directory, not against the old location.
4. Start it, check the log for a directory that could not be used, then download
   one file to confirm.

The database path is `[database] url`, for example
`sqlite:/srv/nanofile-data/nanofile.db?mode=rwc`.

## Permissions

The user running the service has to read and write the whole data directory; in a
container that is uid/gid `1000:1000`:

```bash
chown -R 1000:1000 data
```

On Windows, installing the service grants the service account what it needs, see
[Desktop tray and Windows service](/guide/deploy/desktop-and-service).

## Related

- [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) — what must be backed
  up and how a restore goes.
- [Docker](/guide/deploy/docker) — mounting the data directory into the container.
- [Configuration](/guide/configure/configuration) — storage and database settings.
