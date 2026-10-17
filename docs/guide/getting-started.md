# Getting started

Nanofile is a single server binary named `nanofile` (not `server`). It speaks the
Seafile sync protocol, so the official Seafile clients can connect to it, and it
serves its own web interface.

## Requirements

- A stable Rust toolchain. The workspace uses edition 2024.
- Node.js for the frontend assets. CI uses Node 24.
- Optional: `ffmpeg` for media thumbnails. Without it, media thumbnails are
  unavailable and the media tests are skipped.

## Build from source

```bash
npm install                       # frontend bundler (esbuild is required)
cargo build --release -p server   # the binary is target/release/nanofile
./target/release/nanofile
```

## First run

A start with no configuration file and no `NANOFILE_*` environment variable is an
installation nobody has set up yet. It runs the **first-run setup** instead of
the server:

1. It serves a one-time page and prints its address to the console, for example
   `http://127.0.0.1:8082/setup/<token>`. A desktop build also opens it in a
   browser.
2. The page creates the first administrator and decides the listen address, the
   external URL, the data directory and whether stored blocks are encrypted.
3. Submitting the page writes `~/.nanofile/config.toml` (owner-only) and starts
   the server on the same port.

Every later start reads that file. Delete it to run the setup again.

## Manage the configuration yourself

A configuration file means the first-run setup never runs. This is the path for
a deployment kept in the source tree or provisioned non-interactively:

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # it holds the master secret
./target/release/nanofile --config config.toml
```

The server listens on `http://localhost:8082` by default.

An administrator account is required. The setup page creates one on a first run;
on an existing configuration, use the CLI:

```bash
# interactive password prompt
./target/release/nanofile adduser --email admin@example.com

# or non-interactive
printf '%s\n' 'secret123' | ./target/release/nanofile adduser --email admin@example.com --password-stdin
```

`--regular` creates a non-admin account. See the [CLI reference](/reference/cli)
for the remaining commands.

## Next steps

- [Configuration](/guide/configuration) — sources, precedence, restart rules.
- [Docker](/guide/docker) — the published image.
- [Security](/guide/security) — reverse proxy, `site_url`, secret key.
- [Data directory](/guide/data-directory) — where state lives.
