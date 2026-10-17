# Data directory

State lives under the binary's directory in `data/` by default, since relative
paths resolve against that directory. A section in `config.toml` can name absolute
paths instead.

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

The `[storage]` and `[index]` settings can move each directory. `[database] url`
sets the database path.

## Related

- [Docker](/guide/docker) — mounting the data directory into the container.
- [Configuration](/guide/configuration) — storage and database settings.
