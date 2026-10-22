# Account security

An account's own settings live on two pages, **Settings → Security** and
**Settings → Sessions & Credentials**. The server-side credential rules are in
[Security](/guide/security).

## Two-factor authentication (TOTP)

Turn it on under **Settings → Security**:

1. Press **set up** under two-factor; the page shows a QR code.
2. Scan it with an authenticator app, or type the secret shown below it by hand.
3. Enter the 6-digit code to confirm.
4. **Save the backup codes** — they are shown once, and they are how you sign in
   when the authenticator is not at hand.

Turning two-factor off asks for your current password.

Signing in means password first, then the 6-digit code. Tick "trust this device"
and that device stops asking for **90 days** — sync clients use the same
mechanism, see [Connecting clients](/guide/clients). Codes have their own attempt
limit (`auth.totp_max_attempts`).

## Sessions and credentials

**Settings → Sessions & Credentials** groups everything that can reach the server
under your account, and every entry can be revoked on its own:

| Group | What it holds |
|---|---|
| Client devices | each device's platform, name and last sign-in; expanding one shows its session, sync token and trust state, and unlinks the whole device. |
| Browser sessions | every web sign-in, with the **current session** marked; revoke them one by one, or sign out of all others at once. |
| Sync tokens | the tokens clients hold, with an expiring-soon badge. |
| Trusted devices | the two-factor trust records, with their expiry. |
| API keys | the count and the way in; the page itself is **Settings → API Keys**. |

If you do not recognise a device, revoke it — revocation deletes no data, it only
makes that one credential stop working.

## Password

Changing your password asks for the current one. If you forgot it:

- with mail configured, use "forgot password" on the sign-in page for a reset
  link (`auth.enable_password_reset`, rate-limited per hour);
- an administrator can also create another administrator on the server with
  `nanofile adduser` and fix things from the web interface, see
  [Command line](/reference/cli).

Password policy (`auth.password_min_length`, `auth.require_strong_password`) and
login lockout (`auth.max_login_attempts`, `auth.lockout_duration_secs`) are set by
an administrator under **System Settings → Authentication / Rate limits**.

## What revokes what

| Action | What it affects |
|---|---|
| Changing your password | sessions on the account's **other devices**, every sync token, every API key |
| Disabling the account / remote wipe | every login the account holds, including unused password-reset links |
| Revoking one entry under Sessions & Credentials | that entry alone |
| Rotating the master secret `server.secret_key` | everybody: sessions are signed out, sync clients break, encrypted blocks cannot be decrypted (see [Security](/guide/security)) |

## API keys

API keys let third-party tools — WebDAV clients, sync clients, scripts — connect
without sharing the account password. A key carries only the capabilities granted
to it and can be narrowed to specific libraries; the key management surface
accepts a browser session only, so a key cannot issue another key.

Creating and using them is in [Connecting clients](/guide/clients).

## Sign-in notices

With mail configured, the server sends a notice for a sign-in from a new device
or browser, and when an API key is created. See
[Email notifications](/guide/email).

## Related

- [Users and sharing](/guide/users-and-sharing) — creating accounts and links.
- [Email notifications](/guide/email) — SMTP and the events that send mail.
- [Security](/guide/security) — the server-side credential rules.
- [Connecting clients](/guide/clients) — how two-factor behaves in a client.
