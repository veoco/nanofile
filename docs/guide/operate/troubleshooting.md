---
description: Diagnose startup, sync, search, media, mail, upload and proxy problems.
---
# Troubleshooting

Start with the symptom, not the setting. The first five minutes below narrow the
problem; the sections after it go topic by topic.

**Who this is for:** whoever is on call for the server. **Before you start:** access
to the service manager, the log and the data directory.

## First five minutes

1. **Is the process up?** `systemctl status nanofile`, `docker ps`, or the Windows
   service status. If it restarted, read the last lines of the log.
2. **Is it serving?** `curl -fsS http://127.0.0.1:8082/health`. A 200 means the
   listener is up; it does not prove the database or every feature is healthy.
3. **Read the log.** `journalctl -u nanofile -n 100`, `docker logs --tail 100
   nanofile`, or `nanofile.log` next to a Windows service.
4. **Check disk and permissions.** `df -h` on the data filesystem, and make sure the
   service user can write the data directory.
5. **Check what changed.** A new proxy, a new certificate, a settings change, an
   upgrade, or a full disk explains most sudden failures.

## Setup and startup

**It opens the setup page instead of serving.** There is no configuration file and
no `NANOFILE_*` variable, which is what “not set up yet” means. Walk through the
one-time page or supply a configuration file. See
[Installing and first run](/guide/start/getting-started).

**A release build refuses to start.** `server.secret_key` is missing. Generate one
and put it in the configuration file or `NANOFILE_SERVER_SECRET_KEY`:
`openssl rand -hex 32`. See [Security](/guide/admin/security).

**A named configuration file does not exist.** `--config` or `NANOFILE_CONFIG`
pointing at a missing file is a refused start, not a silent fallback. Check the path
and the service unit's working directory.

**The port will not bind.** Another process holds `server.port`; change the port or
stop the other process. On Windows, connections that were just closed sit in
`TIME_WAIT` for minutes, which is why the Windows service takes over on the next
boot.

**The data directory is not writable.** The service user must read and write the
whole data directory; in a container that is uid/gid `1000:1000`. See
[Data directory](/guide/configure/data-directory).

## Clients and sync

**A client cannot connect, or signs in but does not sync.** Work through the
checklist at the end of [Connecting clients](/guide/use/clients). The usual cause is
a server address that differs from `server.site_url`.

**HTTPS fails or the client rejects the certificate.** The official clients do not
accept a self-signed certificate. Check the proxy chain, the certificate name and
the system clock. See [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy).

**Two-factor authentication loops.** Check the server clock and the authenticator's
clock; TOTP codes are time-based. A trusted device lasts 90 days and can be revoked
under **Settings → Sessions & Credentials**. See
[Account security](/guide/use/account-security).

**Only one client is affected.** Revoke that device's session or sync token, then
sign in again. A password change revokes every sync token on the account.

## Search and indexing

**Search does not find something just uploaded.** Check that `index.enabled` is on,
then re-index that file once. Indexing is asynchronous, so large files appear a
little later.

**A PDF or Office document is not searchable.** Document extraction runs in the
sandbox. If `[sandbox] enabled` is off, documents are recorded as retryable and are
picked up when it is back on. See [Sandbox](/guide/admin/sandbox).

**Images or scanned PDFs are not searchable.** OCR is a separate opt-in. Check both
`index.enabled` and `ocr.enabled`, and that the sandbox is on. See
[Text in images and PDFs](/guide/use/ocr).

**A mixed PDF still lacks its scan pages.** It was indexed through its text layer
before OCR was enabled; re-index that library. Enabling OCR later only re-examines
files the index had skipped.

**Everything is slow.** Lower the OCR `max_side`, raise `min_score`, or turn OCR off;
check Task Management for queued tasks. See [Monitoring](/guide/operate/monitoring).

## Thumbnails and media

**Thumbnails or previews do not work.** With `[sandbox] enabled` off, those features
do not run at all, and they do not fall back to parsing inside the server process.
See [Sandbox](/guide/admin/sandbox).

**Media thumbnails fail.** They need `ffmpeg`; its path is `storage.ffmpeg_path`.
Saving the path re-resolves the grant immediately.

**HEIC/HEIF/AVIF images are skipped.** They are decoded through the media helper
before face detection or OCR. Configure `ffmpeg`; the backfill retries those files
once the helper is available.

## Email

**Mail is not going out.** Check that `email.enabled` is on and the SMTP fields are
right, then send a test message from **System Settings → Email Management**. See
[Email notifications](/guide/configure/email).

**Messages stay failed.** Read the failure in the email queue or on Task Management;
a message is retried up to `email.max_attempts` and then stays failed until you retry
or delete it.

**Open registration returns 404.** Open registration requires working mail. While
outbound mail is off, paused or incomplete, the registration page answers 404 and the
login page hides the create-account link. See
[Users and sharing](/guide/admin/users-and-sharing).

## Uploads and quotas

**Uploads fail.** In order: the account quota
([Users and sharing](/guide/admin/users-and-sharing)), a request-body limit in the
proxy or client, and the storage caps (in-flight resumable uploads and a single zip
both have default caps; set them to `0` to remove them, see
[Configuration](/guide/configure/configuration)).

**One large upload buffers too much memory.** `server.max_chunk_size_mb` caps a
single resumable chunk (default 32 MiB). Raise it only for a client that really sends
larger chunks.

**A restore or copy lost blocks.** Run `nanofile verify-blocks`; `--repair`
quarantines bad blocks under `{block_dir}/.quarantine/`, and clients holding a good
copy re-upload them. See [Command line](/reference/cli).

## Reverse proxy and client IPs

**Everybody looks like one IP behind a proxy.** Set `trusted_proxies`, or
`X-Forwarded-For` / `CF-Connecting-IP` are ignored and IP rate limits collapse into
one bucket. See [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy).

**Large uploads fail at a fixed size.** The proxy's body limit is lower than the file;
set `client_max_body_size 0` in nginx or the equivalent in the proxy.

**WebSocket notifications reconnect in a loop.** The proxy is stripping the
`Upgrade`/`Connection` headers.

## Related

- [Monitoring](/guide/operate/monitoring) — health, tasks, load and alerts.
- [Maintenance](/guide/operate/maintenance) — logs, GC and verification commands.
- [Configuration](/guide/configure/configuration) — where each setting lives and when
  it takes effect.
- [Command line](/reference/cli) — `verify-blocks`, `verify-dir-order`,
  `migrate-blocks`.
