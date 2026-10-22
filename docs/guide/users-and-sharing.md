# Users and sharing

## Users

`adduser` creates an account. It is an administrator by default; `--regular`
creates a non-admin account.

```bash
./nanofile adduser --email admin@example.com
./nanofile adduser --email someone@example.com --regular
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

### Invitation codes

**Settings → Invitations** (admins only) generates them:

- a code generated without an email address can be used by **anybody**;
- one generated with an email address can only be used by **that address**;
- the list shows whether each code is unused, or who used it and when; codes can
  be copied and deleted.

### User management (admins)

**System Settings → User Management** lists every account and can create, edit
and delete one: admin rights, active/inactive and an individual space quota are
all in the edit dialog. `adduser` is the command-line equivalent, see
[Command line](/reference/cli).

### Space quota

Every account has one of three quota states, stored per user:

| State | `users.storage_quota` | Meaning |
|---|---|---|
| Own quota | `n` | At most `n` bytes |
| Unlimited | `0` | No limit for this account |
| Default quota | empty | Follows the server's default space quota |

**System Settings → Storage → Default space quota**
(`storage.default_user_quota_bytes`, default `0`) is the state a new account
starts in. Registration never copies a quota into the account, so a new
registration — by invitation or open — is bounded by this setting, and so is every
existing account that was never given a quota of its own. Environment variable:
`NANOFILE_STORAGE_DEFAULT_USER_QUOTA_BYTES`; `0` means unlimited.

Because the default is read on every upload, changing it re-bounds all of those
accounts at once, administrators included. Give one account its own quota (or `0`)
on the user page to exempt it. The old key `storage.max_storage_bytes` and its
variable `NANOFILE_STORAGE_MAX_STORAGE_BYTES` are still accepted as deprecated
aliases; the name was wrong, not the value — this is the quota of one account, not
a cap on every account together, and a saved setting row is moved to the new key on
upgrade.

## Share links

Select a file or folder in the web file browser and choose **Share**. The
resulting link supports: (an administrator can also see everybody's links under
**System Settings → Share Management** and delete the dead ones.)

- a password,
- an expiry time,
- a view count.

Anonymous **upload links** accept uploads into a library without an account.

Sharing has two switches, on **System Settings → Security → Sharing**:

| Direction | Key | Environment |
|---|---|---|
| Download sharing (share links) | `server.share_download_enabled` | `NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED` |
| Upload sharing (upload links) | `server.share_upload_enabled` | `NANOFILE_SERVER_SHARE_UPLOAD_ENABLED` |

Both default to `true` and apply immediately. Turning one off hides that
direction's controls in the web UI and in the file browser's row menu, closes
its public pages (an existing `/f/`, `/d/` or `/u/` link answers as gone rather
than falling back to signed-in access), and rejects creating new links of that
kind. The other direction keeps working, and the **Shares** page keeps listing
whichever direction is still open; with both off its link disappears and the
page answers 404. Administrative sharing management stays available so dead
links can still be deleted.

Desktop and mobile clients have a single capability bit, `share-link-disabled`,
which follows download sharing: while share links are off, clients hide their
sharing menus entirely. Upload links created before the switch are refused on
use as well.

The single switch from older releases, `share_link_enabled` /
`NANOFILE_SERVER_SHARE_LINK_ENABLED`, is still read and closes both directions;
a saved value is moved to the two keys on upgrade.

## Encrypted libraries

An encrypted library requires its password for uploads as well as reads. Without
it:

- web preview and download return 440,
- anonymous upload links cannot be created for, or used against, the library.

See [Security](/guide/security) for the server-side settings and
[Account security](/guide/account-security) for credentials and API keys.
