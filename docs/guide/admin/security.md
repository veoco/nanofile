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

### Reverse proxy examples

Two configurations you can use as they are, assuming Nanofile listens on
`127.0.0.1:8082` and the public name is `files.example.com`.

nginx:

```nginx
server {
    listen 443 ssl http2;
    server_name files.example.com;

    ssl_certificate     /etc/letsencrypt/live/files.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/files.example.com/privkey.pem;

    # Let the server apply its own upload limits; nginx's 1m default blocks
    # large files.
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

`$connection_upgrade` needs one definition in the `http` block:

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
```

Caddy brings its own TLS and forwarding headers, so the equivalent is one line:

```txt
files.example.com {
	reverse_proxy 127.0.0.1:8082
}
```

With a proxy in front, set `server.site_url` to `https://files.example.com` and
`server.trusted_proxies` to the proxy's address — `["loopback"]` when they share
the host:

```toml
[server]
site_url = "https://files.example.com"
trusted_proxies = ["loopback"]
```

## Proof-of-work challenges

`auth.captcha_mode` adds a self-hosted proof-of-work challenge to the anonymous
web forms. It is the practical replacement for an image captcha: current AI
models read distorted text and object grids, and a self-built visual challenge
needs a model of its own without being meaningfully harder to solve.

The browser receives a random seed and a difficulty, then searches for a nonce
such that `SHA-256(seed || nonce)` has enough leading zero bits. The server
re-computes the hash, accepts the answer once, and expires it after ten minutes.
The work is issued on the page and verified on the form POST, so there is no
extra endpoint to secure.

```toml
[auth]
# "off" (default), "adaptive" or "always".
captcha_mode = "adaptive"
# Leading zero bits: 12–22, default 17.
captcha_difficulty_bits = 17
# In adaptive mode, failures from one address before sign-in asks.
captcha_trigger_after = 2
```

- `off` leaves the forms exactly as they were.
- `adaptive` asks on sign-in only after `captcha_trigger_after` failures from one
  client address; registration and password reset always ask while it is on.
- `always` asks on every sign-in, registration and password-reset form.
- Difficulty grows by one bit per recent failure, up to 22 bits.

The challenge protects the **web forms only**. `POST /api2/auth-token/`, WebDAV,
sync and the desktop/mobile clients are never challenged, because they cannot run
the browser solver; those paths keep their existing lockout, spray detection and
per-IP limits. When a challenge is shown, that form requires JavaScript; a `<noscript>`
message says so.

**Limits.** A proof of work is not a human-versus-AI test. It raises the cost of
naive and volumetric automation; a funded attacker with native code, GPUs or a
browser farm can still pay it. Rate limits and email verification remain the
primary controls. If a public deployment is targeted by AI-driven bots, a
managed risk-based product (Cloudflare Turnstile or hCaptcha) is materially
stronger, at the cost of external network calls, an allow-listed script/frame in
the CSP, and site/secret key management; that is a separate integration. Prefer
`off` or `adaptive` where screen-reader users, no-JavaScript clients, or very
slow devices matter.

## Encrypted libraries

An encrypted library requires its password for uploads as well as reads. Without
it, web preview and download return 440, and anonymous upload links cannot be
created for or used against the library.

## Credential revocation

A password change, deactivation or device wipe revokes every credential the
account holds: other devices, sync clients and API keys, plus unused reset links.
See [Account security](/guide/admin/account-security).

## The master secret

Release builds refuse to start without `server.secret_key`.

A zero-configuration start never reaches that refusal: it runs the first-run setup,
which generates a strong key and persists it.

Debug builds with an explicit config that omits the key still generate an
ephemeral one, so sessions do not survive a restart.
`NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` does the same for a zero-config
start and covers local and CI use.

The key encrypts sessions, mail and at-rest blocks. Keep it out of the image and
out of the repository; see [Docker](/guide/deploy/docker) for generating and mounting it.

## Related

- [Configuration](/guide/configure/configuration) — precedence and the settings pages.
- [Sandbox](/guide/admin/sandbox) — confinement over untrusted parsing.
