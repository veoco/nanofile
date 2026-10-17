# Account security

"Settings → Security" enables two-factor authentication and issues backup codes.

"Settings → Sessions & Credentials" lists the devices signed in to the account,
its repository sync tokens and its API keys. Each entry can be revoked
individually.

## What revokes credentials

Changing the password revokes the account's other devices, sync tokens and all API
keys. Deactivating an account or wiping a device revokes every credential the
account holds, including unused password-reset links.

## API keys

API keys let third-party tools — WebDAV clients, sync clients, scripts — connect
without sharing the account password. A key is bound to the capabilities it was
granted, and can be narrowed to specific libraries. The key-management surface
itself only accepts a browser session, so a key cannot mint another key.

## Sign-in notices

Once email is configured, the server sends a notice for a new-device or
new-browser sign-in and when an API key is created. See
[Email notifications](/guide/email).

## Related

- [Users and sharing](/guide/users-and-sharing) — creating accounts and links.
- [Email notifications](/guide/email) — SMTP and the events that send mail.
- [Security](/guide/security) — server-side credential rules.
