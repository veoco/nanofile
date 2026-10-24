# Connecting clients

Besides the web interface there are three ways in: the official Seafile clients
for syncing, WebDAV for mounting a library as a network drive, and API keys for
scripts and third-party tools.

## The official Seafile clients

The desktop and mobile clients connect to Nanofile directly; there is no separate
seaf-server + seahub stack to install.

**Server address** is the address the client can actually reach — the value of
`server.site_url` (see [Security](/guide/security)). It cannot just be "whatever
opens on the server": the client reuses that address to fetch blocks, upload and
build links, so a wrong one signs in and then does not sync.

### Signing in

Type your email address and password in the client and you are done. A client can
also sign in through the browser: it opens a browser → you sign in on the web →
the confirmation page shows the **platform and device name** that asked → once you
confirm, the server issues a token the client picks up. The device details are on
that page on purpose, so you can recognise a login you did not start.

### Two-factor authentication

With two-factor authentication on, a client login asks for one 6-digit code;
tick "trust this device" and that device does not ask again for **90 days**. Each
trusted device is listed separately under **Settings → Sessions & credentials**
and can be revoked at any time.

### Sync tokens

Every client login leaves a sync token behind. They are listed one by one under
**Settings → Sessions & credentials** and revoked individually. Changing your
password revokes all of them.

### Encrypted libraries

An encrypted library can only be created by a client, and its password is only
entered there — the server does not have it, which is why web preview and
download, and anonymous upload links, do not work for such a library.

## WebDAV

WebDAV mounts a library as an ordinary network drive, for when you do not want a
sync client and just want to browse and copy files.

- **Address:** `https://<site>/dav/<library id>/`. The library's **edit** dialog
  in the library list shows it, next to a copy button.
- **Authentication:** HTTP Basic — the username is your **email address** and the
  password is that library's **WebDAV key**.
- **The key:** created under **Settings → API keys** by picking a WebDAV preset
  (read-only or read-write); the library edit dialog links there too. A key is
  shown only once, when it is created.
- **Master switch:** `server.webdav_enabled`. When it is off, every WebDAV
  request is refused.
- **Permission:** whether you can read or write follows your permission on that
  library (read-only or read-write).

Anything that takes a WebDAV address plus Basic credentials works: Windows "Map
network drive", macOS "Connect to Server", davfs2/gvfs on Linux, and WebDAV apps
on mobile.

**Encrypted libraries do not support WebDAV** — WebDAV has no way to carry the
library password, so a key carrying WebDAV access cannot be bound to one.

## API keys

API keys exist so scripts and third-party tools do not need your password. Create
them under **Settings → API keys**:

- **Capabilities** — tick what the tool needs, or pick a preset (the presets
  include WebDAV read-only and read-write). Write capabilities are marked as
  such in the interface.
- **Scope** — every library, or per-library read-only (`r`) / read-write (`rw`).
- **Expiry** — from a few days to never.
- A key is **shown only once**. The list keeps its name, creation time, expiry
  and last-used time; you can rename it, change its expiry or revoke it.

To call in with one, put it in the `Authorization` header:
`Authorization: Bearer <key>` (`Token <key>` also works).

The key management surface accepts a browser session only, so **a key cannot
issue another key**; changing your password revokes every key on the account. See
[Account security](/guide/account-security).

## When it does not connect

Check in this order:

1. **Address** — is what the client has the same as `server.site_url`, and can
   the client really reach it?
2. **TLS** — over HTTPS, is the certificate trusted (clients reject
   self-signed ones)?
3. **Reverse proxy** — behind one, set `trusted_proxies`, or every client
   collapses into the proxy's address and IP rate limits put them all in one
   bucket. See [Security](/guide/security).
4. **Two-factor authentication** — is the client asking for a code, and is it
   asking for a code rather than rejecting the password?
5. **Logs** — the server log says why a login failed, see
   [Maintenance and troubleshooting](/guide/maintenance).

The proof-of-work challenge protects web forms only; clients never meet it,
because they are not a browser and cannot solve it.

## Related

- [API reference](/reference/api/) — the HTTP interface behind these clients.
- [Web interface](/guide/web-interface) — what the browser can do.
- [Account security](/guide/account-security) — two-factor, trusted devices and
  credential revocation.
- [Security](/guide/security) — `site_url`, TLS and reverse proxies.
