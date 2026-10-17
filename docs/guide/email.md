# Email notifications

Mail is off until SMTP is configured. Two places configure it:

- **The web interface:** "System Settings → Email" — host, port, account,
  password, sender, then `enabled`.
- **Environment variables:** for example `NANOFILE_EMAIL_ENABLED=1` and
  `NANOFILE_EMAIL_HOST=...`. Every `NANOFILE_*` variable that overrides a setting
  is named on its settings page.

Once enabled, the server sends:

- password-reset links,
- new-device and new-browser sign-in notices,
- a notice when an API key is created.

## Keep secrets out of the process list

Password and other secret values can be read from a file through a `*_FILE`
variable, for example `NANOFILE_EMAIL_PASSWORD_FILE`. This keeps them out of the
command line and the process list.

## Related

- [Configuration](/guide/configuration) — precedence and the settings pages.
- [Account security](/guide/account-security) — what a sign-in notice covers.
