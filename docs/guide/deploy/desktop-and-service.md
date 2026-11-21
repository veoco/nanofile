---
description: The Windows service and the desktop tray menu.
---
# Windows service and desktop tray

**Who this is for:** Windows users who want a tray menu or an unattended service. **Before you start:** Windows, and administrator rights for the boot service.

This page is the Windows tray and its boot service; starting with the system on
Linux and macOS is in [Running in the background](/guide/deploy/running-in-background).

Binaries built with `--features tray` carry a system tray menu:

- **Start at login** — a per-user entry, so it starts once somebody logs in.
- **Start at boot (admin)** on Windows — an auto-start service, and the tray at
  login for the account that enables it; described below.
- open the web UI,
- open the config file,
- quit (in a client-mode tray the item is **Close tray**; the service keeps running).

## Start at boot (admin)

**Start at boot (admin)** registers an auto-start service with the Service Control
Manager, so it starts at boot with **no login at all** — what an unattended
machine needs. `nanofile service install` does the same from a command line. From
the tray it also writes the "Start at login" entry; a command-line install leaves
that to the first run of the tray.

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

## Start at boot includes the tray at login

Registering the service also writes the "Start at login" entry for the Windows
account that enables it, and the menu shows that item checked and disabled while
the service is registered. The tray only attaches to the server; it never binds
the address. Installing the service therefore does not leave you without a tray.

Removing the service removes that entry too, so Nanofile no longer starts
automatically. Check "Start at login" to run a server in your session instead.

The service runs for every user, but the tray entry is per Windows account:
another account gets its own tray the first time it runs Nanofile.

## Windows' own startup switch

Task Manager's **Startup** tab and **Settings ▸ Apps ▸ Startup** can turn a startup
entry off without removing it. That switch is respected: a switched-off entry is
shown unchecked, and clicking **Start at login** turns it back on. While the
service is registered the item is otherwise checked and disabled.

## Paths and repairs

Both registrations record absolute paths, so they outlive a move.

- A login entry whose recorded executable or config file is gone is repointed at
  the running copy the next time the tray starts; with the service registered, a
  missing entry is written for the same reason. A healthy entry, or one this build
  cannot read, is left alone.
- An entry that belongs to another installation is left alone and reported in the
  menu.
- A service registration that points at a folder which no longer exists is named
  as such in the confirmation before it is repointed.

Naming a config file that does not exist (`--config`, or `NANOFILE_CONFIG`) is a
refused start, not a silent fall back to built-in defaults.

## When it takes effect

It takes effect at the **next system start**; the tray copy running now keeps
serving, and the menu reports that the service is registered but not running.
Handing the port over immediately is not reliable on Windows, where a just-closed
connection stays in `TIME_WAIT` for minutes and the new process would fail to bind.

Once the service is registered it can be started or stopped at any time from the
menu ("Start/Stop Windows service", administrator) or with
`nanofile service start|stop`.

## Upgrading

The service and the tray run the same executable, and both keep it open, so close
both before replacing the file:

1. In the tray, use **Stop Windows service (admin)** and wait until the status row
   says the service is stopped. Without a tray, run `nanofile service stop` from an
   elevated prompt (add `--config` if you installed with a custom path).
2. **Close the tray.** The service is already stopped; in a client-mode tray the
   menu item is **Close tray**, and it never stops the service.
3. Replace the binary at the same path. Leave the config file and data directories
   alone. Make sure no `nanofile` process is left (the service, the tray and their
   extract workers all run this file); otherwise the copy fails with "file in use".
4. Start the binary again (or wait for the next login): because the service is
   registered, it starts as a **client-mode tray** and shows "stopped (service
   registered)". It does not start a server of its own.
5. Use **Start Windows service (admin)** in the tray, run `nanofile service start`
   from an elevated prompt, or reboot. The service starts the new binary from the
   same registration; the tray then shows "running (Windows service)".

Replacing the file at the same path needs no re-registration. Moving the
installation to a new folder does: the next tray start names the old registration
and offers to repoint it, and the login tray entry is repointed automatically.

If the service does not report running after a stop/start (the port can still be in
`TIME_WAIT`), wait a minute and retry, or reboot. Back up the database and the
config before upgrading: the service runs migrations when it starts. The server is
down from the stop until the service is started again.

## While the service runs

Starting the tray by hand gives you a "client mode" tray: it brings up no second
server, shows who serves the address, and **Close tray** only closes the tray — the
service keeps running. Use "Stop Windows service" (administrator) to stop the
server; it starts again at the next boot, or from "Start Windows service".

The menu also installs or removes the service, and opens the web UI and the config
file.

## Notifications and dialogs

The tray posts one desktop notification each time it starts (so once per login
when Start at login is enabled; Windows toast, Linux notification service; macOS
builds do not post notifications). It is informational only and has no switch.

Dialogs are reserved for decisions and failures; an action never stacks two
confirmations, and a failure never stacks two error dialogs:

- **Start at boot**: one confirmation lists the consequences, any preflight
  advisories, and — when the tray is elevated — the Windows account whose login
  entry will be registered; then the UAC prompt, then one success dialog. On a
  failure, one dialog carries the technical reason.
- **Start/stop Windows service**: no confirmation beyond UAC. Success is shown by
  the status row; only a failure opens a dialog.
- **Start at login**: normally silent. A failed write is reported by a
  notification (Windows/Linux; only the log on macOS).
- **Fatal startup errors**: a configuration error, a first-run setup timeout, a
  start refused because the service owns the address, a server failure after
  startup, or a tray that cannot be created opens a dialog where a desktop
  exists: a message box on Windows, an alert via `osascript` on macOS, and
  `zenity`/`kdialog` (or `notify-send`) on Linux.
- The status row and tooltip are re-probed every five seconds on Windows and
  Linux. macOS refreshes when the tray icon is clicked, so a panel may show the
  previous state for that one open.

The full reason is always in `nanofile.log` next to the executable; dialogs show a
shortened form.

## Logs

A service has no desktop, so there is no tray icon; logs go to `nanofile.log` next
to the executable.

The service start logs its account, executable, config path, working directory and
the result of every directory check. It refuses to start (exit code 2) rather than
failing later with an unnamed error.

## Verify

- The tray icon is present and its menu opens the web interface and the config file.
- The menu shows the server state; with the service registered, "Start at login" is
  checked and disabled.
- `nanofile service status` reports the registration and its account.
- After the next boot, the service runs with no login; the log names the account and
  config path.

## Commands

See the [CLI reference](/reference/cli#windows-service) for `service install`,
`uninstall`, `start`, `stop`, `status` and `run`.
