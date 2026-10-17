# Security

## HTTPS and reverse proxies

When the server runs behind an HTTPS reverse proxy, set `server.site_url` to the
HTTPS address clients actually use. That setting controls:

- the `Secure` cookie attribute,
- HSTS,
- the URL used to build share links.

Sessions, share passwords and API tokens are bearer credentials. Serving them over
plain HTTP exposes them.

## Bind address

The default bind address is `0.0.0.0`. Where only a reverse proxy should reach the
server, bind `127.0.0.1` and close the port in the firewall.

## Trusted proxies

Behind a reverse proxy, `trusted_proxies` decides which peer addresses may supply
`CF-Connecting-IP` / `X-Forwarded-For` for the client IP. Without it those headers
are ignored, so every client collapses to the proxy address and per-IP rate limits
become one shared bucket.

Entries may be an exact IP, a CIDR range, or a keyword: `loopback`, `private`,
`link-local`, `unique-local`.

### Cloudflare Tunnel

Behind a Cloudflare Tunnel (`cloudflared`), the origin sees a loopback or private
peer. Set `trusted_proxies = ["private"]` and bind the origin to `127.0.0.1` (or
isolate it on the Docker network). Cloudflare then supplies the visitor in
`CF-Connecting-IP`, which is preferred over `X-Forwarded-For`.

Only trust `private` when nothing else can reach the origin, or a LAN peer could
forge the header.

## Encrypted libraries

An encrypted library requires its password for uploads as well as reads. Without
it, web preview and download return 440, and anonymous upload links cannot be
created for or used against the library.

## Credential revocation

A password change, deactivation or device wipe revokes every credential the
account holds: other devices, sync clients and API keys, plus unused reset links.
See [Account security](/guide/account-security).

## The master secret

Release builds refuse to start without `server.secret_key`.

A zero-configuration start never reaches that refusal: it runs the first-run setup,
which generates a strong key and persists it.

Debug builds with an explicit config that omits the key still generate an
ephemeral one, so sessions do not survive a restart.
`NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` does the same for a zero-config
start and covers local and CI use.

The key encrypts sessions, mail and at-rest blocks. Keep it out of the image and
out of the repository; see [Docker](/guide/docker) for generating and mounting it.

## Related

- [Configuration](/guide/configuration) — precedence and the settings pages.
- [Sandbox](/guide/sandbox) — confinement over untrusted parsing.
