# Desktop tray and Windows service

Binaries built with `--features tray` carry a system tray menu:

- **Start at login** — a per-user entry, so it starts once somebody logs in.
- **Start at boot (admin)** on Windows — an auto-start service, described below.
- open the web UI,
- open the config file,
- quit.

## Start at boot (admin)

**Start at boot (admin)** registers an auto-start service with the Service Control
Manager, so it starts at boot with **no login at all** — what an unattended
machine needs. `nanofile service install` does the same from a command line,
including retiring the login entry.

Enabling it asks for administrator rights; dismissing the prompt changes nothing.
Before the prompt, the tray checks that the service could actually serve (config
file, every data directory, the log directory, `ffmpeg`, the port). A check that
cannot pass is named and refuses the install, and the advisories are confirmed
first.

### The service account

**The service runs as `NT SERVICE\Nanofile`**, a per-service virtual account: no
password, its own SID, no shared identity, and network access as the computer
account. That is the account Microsoft recommends over `LocalSystem`.

- `LocalSystem` stays available as `--account system` for a deployment whose
  directories a virtual account cannot be granted access to.
- `--account network` and a named account are also accepted. A named account needs
  `--password`, `--password-stdin` or the interactive prompt.

The account is named in the confirmation, reported by `service status`, and
logged at every start.

### Directory access

Installing **grants that account modify access** to the state directories, the log
directory and the config file — a service identity starts with access to nothing.
Removing the service takes the grant back. A directory it may not write is refused
before anything is registered, rather than failing at the next boot.

## The two automatic-start options are alternatives

Registering the service removes the "Start at login" entry (the confirmation says
so), and the tray disables that item while the service is registered.

Removing the service enables the item again but does **not** re-create the entry:
after that, Nanofile starts automatically only if you ask for it.

## Paths and repairs

Both registrations record absolute paths, so they outlive a move.

- A login entry whose recorded executable or config file is gone is repointed at
  the running copy the next time the tray starts. A healthy entry, or one this
  build cannot read, is left alone — but never while the service has replaced it.
- A service registration that points at a folder which no longer exists is named
  as such in the confirmation before it is repointed.

Naming a config file that does not exist (`--config`, or `NANOFILE_CONFIG`) is a
refused start, not a silent fall back to built-in defaults.

## When it takes effect

It takes effect at the **next system start**; the tray copy running now keeps
serving. Handing the port over immediately is not reliable on Windows, where a
just-closed connection stays in `TIME_WAIT` for minutes and the new process would
fail to bind.

## While the service runs

Starting the tray by hand gives you a "client mode" tray: it brings up no second
server, and **Quit stops the service** — the same thing Quit means in an ordinary
tray, where it stops the server.

It restarts at the next boot, and the tray can also install or remove the service
from that menu.

## Logs

A service has no desktop, so there is no tray icon; logs go to `nanofile.log` next
to the executable.

The service start logs its account, executable, config path, working directory and
the result of every directory check. It refuses to start (exit code 2) rather than
failing later with an unnamed error.

## Commands

See the [CLI reference](/reference/cli#windows-service) for `service install`,
`uninstall`, `status` and `run`.
