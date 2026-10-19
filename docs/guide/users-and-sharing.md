# Users and sharing

## Users

`adduser` creates an account. It is an administrator by default; `--regular`
creates a non-admin account.

```bash
./target/release/nanofile adduser --email admin@example.com
./target/release/nanofile adduser --email someone@example.com --regular
```

The password is read from an interactive prompt by default. `--password-stdin`
reads it from the first line of standard input, and `--password-file <path>` reads
it from a file; both keep it out of shell history and `ps` output.

Invitation-code registration can be enabled in `[auth]` for self-registration.

### Registration modes

Two settings, both on **System Settings → Authentication**, decide whether
visitors can create an account:

| State | `auth.enable_registration` | `auth.require_invitation` |
|---|---|---|
| Registration closed | `false` | either |
| Invitation only (default) | `true` | `true` |
| Open registration | `true` | `false` |

Their environment variables are `NANOFILE_AUTH_ENABLE_REGISTRATION` and
`NANOFILE_AUTH_REQUIRE_INVITATION`. The old names — `auth.enable_invitations`
and `NANOFILE_AUTH_ENABLE_INVITATIONS` — are still accepted as deprecated
aliases, and a saved setting row is moved to the new key on upgrade.

Invitation registration is active immediately: the code is the proof.
**Open registration requires a working email setup.** The account is created
unverified, a verification link valid for 3 days is mailed to it, and the account
cannot sign in until the link is opened. A user who did not receive the message
can sign in with their password to send a new link (at most once every 5 minutes).
While outbound mail is off, paused or incomplete, the registration page answers
404 and the login page hides the **Create Account** link. Accounts that remain
unverified for 7 days are removed by the cleanup task.

Administrators can still create accounts directly; those accounts, accounts
created by `adduser`, the first-run administrator and invitation registrations
are all considered verified.

## Share links

Select a file or folder in the web file browser and choose **Share**. The
resulting link supports:

- a password,
- an expiry time,
- a view count.

Anonymous **upload links** accept uploads into a library without an account.

Setting `share_link_enabled` to `false` disables anonymous share and upload links.
Existing links stop resolving; they do not fall back to signed-in access.

## Encrypted libraries

An encrypted library requires its password for uploads as well as reads. Without
it:

- web preview and download return 440,
- anonymous upload links cannot be created for, or used against, the library.

See [Security](/guide/security) for the server-side settings and
[Account security](/guide/account-security) for credentials and API keys.
