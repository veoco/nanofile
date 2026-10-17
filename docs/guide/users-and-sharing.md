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
