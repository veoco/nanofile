---
description: Put Nanofile behind HTTPS with nginx or Caddy, and make client IPs work.
---
# HTTPS and reverse proxy

Nanofile speaks plain HTTP. A reverse proxy terminates TLS, forwards the request and
adds the forwarding headers the server needs to see the real client address. This
page is the proxy setup; the credential and server-side rules are in
[Security](/guide/admin/security).

**Who this is for:** anybody exposing Nanofile outside a trusted LAN. **Before you
start:** a DNS name, a TLS certificate (or Caddy/Cloudflare to obtain one), and the
server listening on a loopback address.

## Before you start

- Point the DNS name at the proxy.
- Bind Nanofile to `127.0.0.1` when the proxy runs on the same host, so the plain
  HTTP port is not reachable directly. In Docker, put the container on a private
  network instead of publishing port 8082 to the world.
- Decide the external URL. It must be the address clients actually use, not the
  server's internal address.

## Set site_url

Set `server.site_url` to the HTTPS address:

```toml
[server]
site_url = "https://files.example.com"
```

`site_url` controls the `Secure` cookie attribute, HSTS, and the address used to
build share links. A wrong value signs clients in and then breaks sync, because the
client reuses that address to fetch blocks and upload.

On a first run, do not expose the one-time setup page; set `site_url` and the proxy
first, or reach the setup page through an SSH tunnel. See
[Installing and first run](/guide/start/getting-started).

## nginx

This configuration assumes Nanofile listens on `127.0.0.1:8082` and the public name
is `files.example.com`:

```nginx
server {
    listen 443 ssl http2;
    server_name files.example.com;

    ssl_certificate     /etc/letsencrypt/live/files.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/files.example.com/privkey.pem;

    # Let Nanofile apply its own upload limits; nginx's 1m default blocks large files.
    client_max_body_size 0;

    location / {
        proxy_pass http://127.0.0.1:8082;
        proxy_http_version 1.1;

        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # In-app notifications are a WebSocket; the handshake has to pass through.
        proxy_set_header Upgrade           $http_upgrade;
        proxy_set_header Connection        $connection_upgrade;

        # A large download or a long sync must not be cut off midway.
        proxy_read_timeout 3600s;
        proxy_buffering off;
    }
}
```

`$connection_upgrade` needs a definition in the `http` block:

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
```

Then set the proxy address in `server.trusted_proxies`:

```toml
[server]
site_url = "https://files.example.com"
trusted_proxies = ["loopback"]
```

## Caddy

Caddy obtains and renews the certificate by itself:

```txt
files.example.com {
	reverse_proxy 127.0.0.1:8082 {
		# Retire idle upstream connections before the server does.
		transport http {
			keepalive 10s
		}
	}
}
```

Caddy forwards `X-Forwarded-For` and `X-Forwarded-Proto`; set `server.site_url` and
`server.trusted_proxies = ["loopback"]` as above.

The `keepalive` line is not optional. Caddy reuses its upstream connections, and the
server retires one when `server.header_read_timeout_secs` (120 s by default) passes
while it waits for the next request head. A request written into a connection the
server has just closed reaches the client as a 502 — the sporadic kind that appears
only on the first request after an idle period, because everything else looks
healthy. Ten seconds is under any polling interval; raise it for throughput, but keep
it below the server's deadline. The nginx recipe above sends `Connection: close`
upstream, so it never reuses an idle connection and needs no equivalent.

## Cloudflare Tunnel

Behind a Cloudflare Tunnel (`cloudflared`), the origin sees a loopback or private
peer. Set:

```toml
[server]
site_url = "https://files.example.com"
trusted_proxies = ["private"]
```

Bind the origin to `127.0.0.1` or isolate it on the Docker network. Cloudflare then
supplies the visitor in `CF-Connecting-IP`, which is preferred over
`X-Forwarded-For`.

Only trust `private` when nothing else can reach the origin; otherwise a LAN peer
could forge the header.

## Trusted proxies

`trusted_proxies` decides which peer addresses may supply `CF-Connecting-IP` or
`X-Forwarded-For` for the client IP. Without it those headers are ignored, every
client collapses into the proxy address, and per-IP rate limits become one shared
bucket.

Entries may be an exact IP, a CIDR range, or a keyword:

- `loopback` — `127.0.0.1` and `::1`;
- `private` — RFC 1918 / private ranges;
- `link-local`, `unique-local` — the corresponding IPv6 ranges.

Keep the list as narrow as the proxy setup allows. A wide range trusts every host
that can reach the server from those addresses.

## Verify

- `curl -I https://files.example.com/` returns the expected redirect or page, and
  the response carries `strict-transport-security` when HSTS is on.
- Sign in over HTTPS; the session cookie has the `Secure` flag.
- Create a share link; it starts with `https://files.example.com`.
- Open the web interface and wait for the in-app notification connection; it should
  not reconnect in a loop.
- Upload a file larger than 1 MiB; the proxy must not answer 413.
- Check the server log for the client address; with `trusted_proxies` correct it is the
  visitor, not the proxy.

## Troubleshoot

- **502/504** — the proxy cannot reach `127.0.0.1:8082`; check `server.addr`,
  `server.port` and whether the server is running.
- **Sporadic 502 that clears by itself** — a proxied request landed on an idle
  upstream connection the server had just retired; see the Caddy `keepalive` note
  above, or raise `server.header_read_timeout_secs` past the client's poll interval.
- **Redirect loop** — the proxy sends `X-Forwarded-Proto: http`, or `site_url` says
  `https` while the proxy forwards plain HTTP; fix the header or the URL.
- **WebSocket disconnects** — the `Upgrade`/`Connection` headers are missing, or the
  proxy strips them.
- **Uploads fail at a fixed size** — the proxy's body limit is lower than the file;
  set `client_max_body_size 0` in nginx or the equivalent in the proxy.
- **Everybody has the same IP** — `trusted_proxies` is empty or does not include the
  proxy address; see above.
- **Clients reject the certificate** — use a trusted certificate; the official
  clients do not accept a self-signed one.

## Related

- [Security](/guide/admin/security) — the master secret, bind address, PoW and
  credential rules.
- [Configuration](/guide/configure/configuration) — where `site_url` and
  `trusted_proxies` sit in the settings.
- [Running in the background](/guide/deploy/running-in-background) — keep the server
  and the proxy running.
- [Troubleshooting](/guide/operate/troubleshooting) — when clients cannot connect.
