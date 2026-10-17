# Command line

```
nanofile [--config <path>]           start the server (default)
nanofile [--config <path>] server    start the server explicitly
```

`--config <path>` overrides `NANOFILE_CONFIG` and the default lookup order. See
[Configuration](/guide/configuration#where-the-config-file-is-looked-for).

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
nanofile migrate-blocks [--dry-run]
```

Moves the legacy flat block layout (`blocks/<2hex>/<id>`) to the per-library
layout. Normally this runs automatically at startup before the server serves its
first request. Use the command to pre-flight the copy volume (`--dry-run`) or to
run it explicitly with the server stopped.

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
nanofile service status
nanofile service run
```

- `install` registers an auto-start service (starts at boot, no login) and removes
  this installation's "Start at login" entry. The default account is
  `NT SERVICE\Nanofile`; see
  [Desktop tray and Windows service](/guide/desktop-and-service#start-at-boot-admin).
- `uninstall` stops and removes the service and takes back the directory access it
  was granted.
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
