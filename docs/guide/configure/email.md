# Email notifications

Mail is unavailable until SMTP is configured and switched on.

## Where it is configured

Every SMTP field is an ordinary setting, edited under **System Settings →
Email**; each row on that page also shows where its current value comes from
(environment variable, saved setting, configuration file, or built-in default) and
names the `NANOFILE_*` variable that overrides it.

The fields you usually touch:

| Key | Effect |
|---|---|
| `email.enabled` | the master switch. While it is off nothing is delivered, and the registration page that depends on mail answers 404. |
| `email.paused` | pause delivery: mail is still queued, just not sent. |
| `email.host`, `email.port` | the SMTP server address and port. |
| `email.tls` | `starttls`, `tls` or `none`. Choosing `none` earns a plaintext warning on the admin page. |
| `email.username`, `email.password` | the SMTP credentials. |
| `email.from_address`, `email.from_name` | the sender address and display name. |
| `email.timeout_secs` | the per-connection timeout. |
| `email.max_attempts` | how many delivery attempts one message gets. |

As environment variables, for instance `NANOFILE_EMAIL_ENABLED=1` and
`NANOFILE_EMAIL_HOST=smtp.example.com`.

## What is sent

- password-reset links,
- open-registration email-verification links (such an account cannot sign in
  until the link is opened),
- a notice when an API key is created.

Sign-in notices are three separate switches: `email.notify_new_login` (a new
sign-in), `email.notify_new_device` (a new device) and
`email.notify_api_key_created`.

## Keep secrets out of the process list

Secret values such as the password can be read from a file through a `*_FILE`
variable, for example `NANOFILE_EMAIL_PASSWORD_FILE`, which keeps them off the
command line and out of `ps` output. The settings page marks which keys support
it.

## The queue, and where to look when mail does not go out

**System Settings → Email** (`/sysadmin/email/`) is where delivery is watched:

- **Status** — whether the switch is on, whether delivery is ready (naming
  whatever is missing), and whether it is paused.
- **Test message** — send one to an address you control; the quickest way to
  confirm a configuration.
- **Outbox** — how many messages are pending, sent and failed, and a way to clear
  the finished ones.
- **Queue** — every message with its recipient, state and attempt count; a failed
  one can be retried or deleted.

A message that will not go out is retried up to `email.max_attempts` and then
stays failed until you deal with it in the queue. Failures also land on the tasks
page, see [Maintenance and troubleshooting](/guide/operate/maintenance).

## Related

- [Configuration](/guide/configure/configuration) — precedence and the settings pages.
- [Account security](/guide/admin/account-security) — what a sign-in notice covers.
- [Users and sharing](/guide/admin/users-and-sharing) — open registration needs working
  mail.
