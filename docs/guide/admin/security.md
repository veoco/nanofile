---
description: The master secret, bind address, proof-of-work challenges and credential rules.
---
# Security

**Who this is for:** administrators exposing Nanofile outside a trusted LAN.

## HTTPS and reverse proxies

A reverse proxy terminates TLS and forwards the request. Set `server.site_url` to the
external HTTPS URL, bind the server to `127.0.0.1` when the proxy runs on the same
host, and list the proxy in `server.trusted_proxies`.

The nginx and Caddy recipes, the Cloudflare Tunnel setup, the trusted-proxy keywords
and the verification steps are in
[HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy).

## Bind address

The default bind address is `0.0.0.0`. Where only a reverse proxy should reach the
server, bind `127.0.0.1` and close the port in the firewall.

## Trusted proxies

`trusted_proxies` decides which peer addresses may supply `CF-Connecting-IP` or
`X-Forwarded-For`. Without it those headers are ignored, every client collapses into
the proxy address, and per-IP rate limits become one shared bucket. The entry forms
and examples are in
[HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy#trusted-proxies).

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
- In adaptive mode the difficulty grows by at most three bits above the configured
  base (the default 17 rises to 20), and never above the 22-bit maximum.

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

A password change or deactivation revokes every credential the account holds:
other devices, sync clients and API keys, plus unused reset links. A device wipe
is narrower: it revokes only what the wiped device holds — its sessions, sync
tokens and two-factor device trust. See
[Account security](/guide/use/account-security).

## The master secret

Release builds refuse to start without `server.secret_key`.

A zero-configuration start never reaches that refusal: it runs the first-run setup,
which generates a strong key and persists it.

Debug builds with an explicit config that omits the key still generate an
ephemeral one: every start makes a new key, so the data encrypted under it (sync
tokens, TOTP seeds, queued mail and saved secrets) becomes unreadable, and the
start-up fingerprint check clears every stored session — sessions do not survive
the restart. With at-rest block encryption enabled the server refuses to start
instead of generating one, because the ephemeral key would make every stored
block permanently unreadable. `NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` does
the same for a zero-config start and covers local and CI use.

The key derives the CSRF signing secret, the sync-token encryption key, the TOTP
encryption key, the queued-mail key, the notification JWT key (unless configured)
and the at-rest block key (unless `storage.encryption_key` is set). Session and
account tokens are random and stored only as SHA-256 digests, so they are not
encrypted with this key; the server records a fingerprint of the key at start-up
and deletes every stored session when it changes. Rotating the secret therefore
signs every browser and API session out. API keys are hashed the same way but are
not tied to the fingerprint: revoke them explicitly if the key leaked. Rotation
also breaks sync clients and makes TOTP seeds, queued mail and saved secrets
undecryptable; encrypted blocks are affected only when the block key was derived
from it.

Keep it out of the image and out of the repository; see
[Docker](/guide/deploy/docker) for generating and mounting it.

## Related

- [HTTPS and reverse proxy](/guide/deploy/https-and-reverse-proxy) — TLS, nginx/Caddy
  and trusted proxies.
- [Configuration](/guide/configure/configuration) — precedence and the settings pages.
- [Sandbox](/guide/admin/sandbox) — confinement over untrusted parsing.
- [Account security](/guide/use/account-security) — two-factor, sessions and
  revocation from the user's side.
