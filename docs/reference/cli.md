# Command line

```
nanofile [--config <path>]           start the server (default)
nanofile [--config <path>] server    start the server explicitly
```

`--config <path>` overrides `NANOFILE_CONFIG` and the default lookup order. See
[Configuration](/guide/configure/configuration#where-the-config-file-is-looked-for).

## Server and account management

```
nanofile adduser [--email <address>] [--regular]
                 [--password <value> | --password-stdin | --password-file <path>]
```

Creates an account. It is an administrator by default; `--regular` creates a
non-admin user. Prefer the interactive prompt, `--password-stdin` or
`--password-file` over `--password`, which is visible in shell history and `ps`
output.

## Block maintenance

```
nanofile gc [--dry-run] [--mode audit|incremental]
```

Runs one garbage-collection pass by hand: prunes history beyond each library's
retention settings and removes blocks — and whole library prefixes — that nothing
references any more.

- `--dry-run` reports what it would remove and changes nothing.
- `--mode` overrides `[gc] mode` for this run. `audit` lists every library's blocks
  and also finds what a crashed upload left behind; `incremental` deletes only what
  pruning history just orphaned and needs no listing.
- Without `--mode`, the run follows the configured schedule: each library whose own
  `gc.audit_interval_hours` has passed is audited in full, and that library's audit
  time is recorded. With `--mode`, that scheduled upgrade is suppressed for this run.
- Blocks younger than `gc.min_block_age_secs` are never deleted. Blocks an upload
  announced and never committed are swept after `gc.pending_ttl_hours`.
- The output names the scan mode (`audit`, `incremental` or `mixed`), how much each
  category removed, and how many libraries were skipped because they were busy,
  unreadable, not fully stored, or changed during the pass.
- The command takes the grace window and the default mode from the configuration
  file and `NANOFILE_*` variables. Saved settings are not consulted: they configure
  the background job, which reads them at start.

```
nanofile migrate-blocks [--dry-run]
```

Moves the legacy flat block layout (`blocks/<2hex>/<id>`) to the per-library
layout. Normally this runs automatically at startup before the server serves its
first request. Use the command to pre-flight the copy volume (`--dry-run`) or to
run it explicitly with the server stopped.

```
nanofile convert-blocks [--status]
```

Converts pre-encryption blocks to the versioned at-rest format
(`NFE1 || key id || ciphertext || tag`) under their existing content-addressed ids,
records every finished library, and writes the completion marker
`{block_dir}/.encryption_converted`. It is the same pass the server runs in the
background in `lazy` mode, and the one it starts by itself when an `on` store has no
completion marker; run it with the server stopped to finish the conversion without
waiting for that pass.

- Stop the server first: two writers on one block directory are not supported.
- The pass is resumable: an interrupted run continues with the library it had not
  finished, and a block that is already converted is only inspected by its header.
- `--status` reports whether the conversion is complete and how many libraries are
  still pending. It changes nothing and exits non-zero while the conversion is
  unfinished, so a script can gate the switch to `on` on it.
- `storage.block_encryption_mode` must be `lazy` or `on` with the key available;
  `off` is refused because there is no cipher to convert with.
- A store that has always run in `on` mode does not need this command to start: the
  next start runs the same pass in the background and records the marker. Use the
  command to finish that pass with the server stopped, or to check the result.

```
nanofile verify-blocks [--dry-run] [--repair] [--repo <id>] [--json]
```

Hashes every stored block against its content-addressed id, and checks every file
object's blocks against the size it declares.

- Read-only unless `--repair` is given.
- `--repair` quarantines blocks that fail the id check under
  `{block_dir}/.quarantine/`. Their bytes are kept, so a client holding a good copy
  re-uploads them.
- `--repo` restricts the check to one library.
- `--json` writes a machine-readable report to stdout.
- Exits non-zero when it finds anything to act on.

```
nanofile verify-dir-order [--dry-run] [--repair] [--repo <id>] [--json]
```

Checks that every directory object reachable from a library's HEAD is stored in
Seafile's canonical (descending by name) order, which is what makes its `fs_id`
reproducible by an official client.

- Read-only unless `--repair` is given.
- `--repair` appends one normalizing commit per affected library. History is not
  rewritten and no object is deleted, so clients pick the fix up as an ordinary
  update.
- `--repo` and `--json` behave as for `verify-blocks`.
- Exits non-zero when it finds anything to act on.

## Windows service

Administrator rights are required to install or remove the service.

```
nanofile service install [--account virtual|system|network|<name>]
                         [--password <value> | --password-stdin]
nanofile service uninstall
nanofile service start
nanofile service stop
nanofile service status
nanofile service run
```

- `install` registers an auto-start service (starts at boot, no login). The default
  account is `NT SERVICE\Nanofile`. Installing from the tray also writes the "Start
  at login" entry, so the tray icon appears when you log in; a command-line install
  leaves that to the first run of the tray. See
  [Windows service and desktop tray](/guide/deploy/desktop-and-service#start-at-boot-admin).
- `uninstall` stops and removes the service, takes back the directory access it was
  granted, and removes this installation's "Start at login" entry.
- `start` / `stop` start or stop the registered service now; both need
  administrator rights. They are what the tray's "Start/Stop Windows service"
  items call.
- `status` reports the registration and its account; exit code 0 when the service
  is installed.
- `run` is what the Service Control Manager calls. Running it by hand exits with
  an error.

## Not an operator command

The binary also accepts a hidden `extract-worker` subcommand. The server spawns it
on itself for every sandbox-backed request; it is not meant to be run by hand. The
release and nightly workflows use `extract-worker --probe` to smoke-test the
sandbox of a built artifact and of the published image. See
[Sandbox internals](/dev/sandbox).
