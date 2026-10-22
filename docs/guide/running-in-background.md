# Running in the background

Starting the tray build by hand is enough on a desktop; on a server or an
unattended machine the service has to come up with the system and come back after
a crash. Windows has its own service and tray, covered in
[Desktop tray and Windows service](/guide/desktop-and-service); this page covers
Linux, macOS and containers.

## What every method needs

Whichever you pick, these hold:

- **Relative paths resolve against the binary's directory** (not the working
  directory), so keeping the binary, the configuration and the data directory
  together is the least surprising setup; use absolute paths to split them, see
  [Data directory](/guide/data-directory).
- **A configuration file you named but that does not exist refuses to start**
  (`--config` or `NANOFILE_CONFIG`) instead of quietly falling back to built-in
  defaults — so a service never comes up attached to the wrong database.
- **A release build refuses to start without the master secret**, so the service
  process must be able to read `server.secret_key` (from the file, or from
  `NANOFILE_SERVER_SECRET_KEY`).
- **Do not run it as root.** Create a dedicated user that can read and write the
  configuration and the data directories.
- **Logs go to stdout by default** (headless runs), so journald or launchd can
  collect them; use `[logging]` to write a file instead.

## Linux: systemd

```ini
# /etc/systemd/system/nanofile.service
[Unit]
Description=Nanofile
After=network.target

[Service]
Type=simple
User=nanofile
Group=nanofile
WorkingDirectory=/opt/nanofile
ExecStart=/opt/nanofile/nanofile --config /opt/nanofile/config.toml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now nanofile
sudo systemctl status nanofile
journalctl -u nanofile -f        # follow the log
```

- Put the binary in `/opt/nanofile/` and make the configuration and data
  directories writable by that user:
  `sudo chown -R nanofile:nanofile /opt/nanofile`.
- `Restart=always` covers crashes and abnormal exits; a deliberate
  `systemctl stop` is not restarted.
- Upgrading is replacing the binary and `sudo systemctl restart nanofile`; the
  full sequence is in [Backup, restore and upgrade](/guide/backup-and-upgrade).

**Harden one step at a time.** systemd's isolation options
(`ProtectSystem=strict`, `PrivateTmp`, `ProtectHome`, …) change what filesystem
the process can see, and Nanofile's parsing child processes build their own
restrictions on top of that. Get the minimal unit above working first, then add
options one by one and restart to verify — do not paste a whole hardening block
in at once.

## macOS: launchd

On a desktop, run the `tray` variant directly; to have it start in the
background with the system, use a LaunchDaemon (a template — **not exercised by
this project's CI**; replace the paths and user with your installation):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!-- /Library/LaunchDaemons/io.github.veoco.nanofile.plist -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>io.github.veoco.nanofile</string>
    <key>ProgramArguments</key>
    <array>
        <string>/opt/nanofile/nanofile</string>
        <string>--config</string>
        <string>/opt/nanofile/config.toml</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/opt/nanofile</string>
    <key>UserName</key>
    <string>nanofile</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/nanofile.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/nanofile.log</string>
</dict>
</plist>
```

```bash
sudo launchctl load -w /Library/LaunchDaemons/io.github.veoco.nanofile.plist
```

To start it with a user session instead, put it in `~/Library/LaunchAgents/`,
load it with `launchctl load -w`, and drop the `UserName` key.

## Containers: a restart policy

Docker can watch the process by itself:

```bash
docker run -d --name nanofile --restart=unless-stopped \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

`unless-stopped` restarts the container after a crash and after Docker itself
restarts, but does not override a deliberate `docker stop`. In compose it is
`restart: unless-stopped`. See [Docker](/guide/docker).

The image has no `HOME` and no shell, so **everything a background run needs has
to arrive as `NANOFILE_*` variables or a mounted configuration file**; the
first-run setup has nowhere to write `~/.nanofile/config.toml` inside it.

## Related

- [Desktop tray and Windows service](/guide/desktop-and-service) — the Windows
  tray and SCM service.
- [Backup, restore and upgrade](/guide/backup-and-upgrade) — stopping, replacing
  the binary, changing the image tag.
- [Data directory](/guide/data-directory) — where all of this lives.
- [Configuration](/guide/configuration) — the config lookup order and
  `NANOFILE_*`.
