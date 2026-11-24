---
description: Run the published image, mount the data directory and use compose.
---
# Docker

**Who this is for:** operators running the published image. **Before you start:** Docker, and a data volume writable by uid/gid 1000.

The image is a `scratch` container holding only the `nanofile` binary. It has no
config file, no data directory and no `HOME`. It runs as uid/gid `1000:1000`, so
the mounted data volume must be writable by that user.

## Keep the container out of the first-run setup

Both recipes below set `NANOFILE_*` variables, which counts as configuring the
installation. A container started with **no** environment variables at all enters
the first-run setup instead:

- The setup URL goes to the container's stdout, so `docker logs <name>` has the
  page address.
- The setup page listens on the configured listen address — by default
  `0.0.0.0:8082`, every interface — while the URL it prints is `127.0.0.1`, and
  the page has no TLS. On a remote host, restrict the port with a firewall or
  reach the page through an SSH tunnel rather than exposing it.
- The image has no `HOME` and its working directory is `/`, which uid 1000 cannot
  write, so the setup has nowhere to write `~/.nanofile/config.toml`. Give it a
  writable home and working directory, for example `-e HOME=/data -w /data`; the
  configuration is then written to `/data/.nanofile/config.toml`.
- To skip the setup instead, mount a configuration file that already exists and
  pass `--config` with its path — a named configuration file must exist, or the
  start is refused — then create the administrator with `adduser` as shown below.

When the variables skip the setup, create the first administrator in the container:

```bash
docker exec -it nanofile /nanofile adduser --email admin@example.com
```

The interactive password prompt needs a terminal; without `-it` the process exits
before the password is read. For scripts, feed the password on stdin:

```bash
printf '%s\n' 'secret123' | docker exec -i nanofile /nanofile adduser --email admin@example.com --password-stdin
```

Or mount a configuration file with `[admin_init]`. For production, replace `latest`
with a version tag; the `latest` tag moves to every new release.

## The master secret

Generate the master secret once and reuse it. It derives the CSRF signing secret,
the sync-token, TOTP and queued-mail keys, the notification JWT key (unless
configured) and the at-rest block key (unless `storage.encryption_key` is set).
Sessions are not derived from it — session tokens are random and stored as a
SHA-256 digest — so rotating it does not sign browsers out; it does break sync
clients and makes that data unreadable until the old secret is back.

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret
```

## Run with a config file

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -v "$PWD/config.toml:/etc/nanofile/config.toml:ro" \
  -v "$PWD/nanofile-secret:/run/secrets/nanofile-secret:ro" \
  -e NANOFILE_CONFIG=/etc/nanofile/config.toml \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_INDEX_INDEX_DIR=/data/index \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

`config.toml` is optional; built-in defaults fill the rest. The second recipe
sets only what has to live outside the image:

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

The `nanofile-secret` value is generated above and reused across starts.

## A compose file

```yaml
# docker-compose.yml
services:
  nanofile:
    image: ghcr.io/veoco/nanofile:latest
    restart: unless-stopped
    ports:
      - "8082:8082"
    volumes:
      - ./data:/data
      - ./config.toml:/etc/nanofile/config.toml:ro
      - ./nanofile-secret:/run/secrets/nanofile-secret:ro
    environment:
      NANOFILE_CONFIG: /etc/nanofile/config.toml
      NANOFILE_DATABASE_URL: sqlite:/data/nanofile.db?mode=rwc
      NANOFILE_STORAGE_BLOCK_DIR: /data/blocks
      NANOFILE_STORAGE_TEMP_DIR: /data/temp
      NANOFILE_SERVER_SECRET_KEY: ${NANOFILE_SECRET:?set NANOFILE_SECRET in .env first}
```

`${NANOFILE_SECRET:?…}` refuses to start when the variable is missing, rather
than coming up with an empty master secret.

## Verify

- `docker logs nanofile` shows the listener and no startup error.
- `curl -fsS http://127.0.0.1:8082/health` returns 200.
- Sign in, upload a file, restart the container with the same volume, and download it
  again.

## Upgrades and permissions

An upgrade can leave an existing data directory owned by a different uid. Make
it writable by `1000:1000` before restarting:

```bash
chown -R 1000:1000 data
```

`--user` overrides the image's numeric user, but the data directory must still be
writable by whatever uid the server runs as.

## Next steps

- [Data directory](/guide/configure/data-directory) — what each mounted directory holds.
- [Configuration](/guide/configure/configuration) — every `NANOFILE_*` variable.
- [Security](/guide/admin/security) — the master secret and reverse-proxy settings.
- [Backup, restore and upgrade](/guide/operate/backup-and-upgrade) — what the image tags
  (`latest`, `vX.Y.Z`, `edge`) are for.
