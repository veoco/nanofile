# Libraries

Libraries (Seafile's `repo`) are the top-level trees users sync and share. This
page covers listing, creating and modifying a library, its transfer links, and
the password of an encrypted library. Directory and file contents are on
[Directories](/reference/api/directories) and [Files](/reference/api/files).

Source: `server/src/handler/repos.rs`, `server/src/handler/password.rs`,
`server/src/handler/chunked_upload.rs`,
`server/src/service/repo/service.rs`, `server/src/service/repo/password.rs`.

## List libraries {#get-api2-repos}

`GET /api2/repos/`

Returns every library the caller can see, in membership (creation) order; the
official clients apply their own ordering. A library-scoped API key only sees its
bound libraries.

**Response `200`**

```json
[
  {
    "id": "…",
    "name": "Docs",
    "desc": "",
    "owner": "user@example.com",
    "owner_name": "User",
    "encrypted": false,
    "size": 22020651,
    "mtime": 1760000000,
    "permission": "rw",
    "head_commit_id": "…",
    "type": "repo",
    "virtual": false,
    "root": "…",
    "salt": null
  }
]
```

`permission` is `"rw"` or `"r"`. For an encrypted library the response also
carries `enc_version`, and `magic`/`random_key`/`salt` where the caller is
entitled to them.

## Create a library {#post-api2-repos}

`POST /api2/repos/`

Creates a library owned by the caller. Bodies may be JSON, URL-encoded forms
(desktop client) or `multipart/form-data` (Android), chosen by `Content-Type`.

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Library name. |
| `desc` | no | Description. |
| `repo_id` | no | Client-chosen UUID for a cloned library. |
| `encrypted` | no | `1` for an encrypted library. |
| `enc_version` | no | Encryption version (`2` or `4`); ignored when not encrypted. |
| `magic`, `random_key`, `salt` | no | Encryption material a client computes itself. |
| `pwd_hash`, `pwd_hash_algo`, `pwd_hash_params` | no | The Seafile 11+ password verifier triple, sent when `/api2/server-info/` advertised an algorithm. |
| `passwd` | no | A plaintext password the server hashes itself. |

**Response `201`** — the created library in the `GET /api2/repos/{repo_id}/`
shape. Requires `library.create`.

An encrypted library is created by a client, which alone holds the password; the
server stores `magic`/`random_key` (or the `pwd_hash` triple) and can never
decrypt its blocks. `passwd` is for clients that let the server derive those
values.

## Get a library {#get-api2-repos-repo-id}

`GET /api2/repos/{repo_id}/`

Returns one library's metadata in the list shape, including `permission` and
`head_commit_id`. A library with no commit yet has `head_commit_id: null` and
`root: null`.

## Modify a library {#post-api2-repos-repo-id}

`POST /api2/repos/{repo_id}/?op=…`

Seafile overloads this path; the query's `op` selects the operation:

| `op` | Body | Meaning |
| --- | --- | --- |
| `rename` | `repo_name` (JSON, form or multipart) | Rename; owner only. |
| `update` | JSON `{repo_name?, description?, history_limit?, history_ttl_days?}` | Update metadata; owner only. |
| `setpassword` | `password` (JSON, form or multipart) | Verify an encrypted library's password; wrong password is `400` `Incorrect password`. |
| `checkpassword` | `magic` (JSON, form or multipart) | Check a client-computed `magic`. |
| *omitted* | `password` | The desktop client's password form; same as `setpassword`, but the response differs for a missing field. |

**Response `200`** — `"success"` (a JSON string) for `rename`/`update`; the
password operations return `{"success": true}`.

Wrong-password checks share one per-(user, library) limiter; repeated failures
answer `429`. Requires `library.update` (or `library.password` for the password
operations); a key also needs write permission on the library.

## Delete a library {#delete-api2-repos-repo-id}

`DELETE /api2/repos/{repo_id}/`

Deletes the library and its data. **Response `200`** — the JSON string
`"success"`. Requires `library.delete`; owner only.

## List libraries (v2.1) {#get-api-v2-1-repos}

`GET /api/v2.1/repos/`

The v2.1 shape used by the mobile clients and the web interface.

**Response `200`**

```json
{
  "repos": [
    {
      "repo_id": "…",
      "repo_name": "Docs",
      "repo_desc": "",
      "permission": "rw",
      "encrypted": false,
      "type": "repo",
      "size": 22020651,
      "last_modified": "2026-10-24T09:00:00+00:00",
      "mtime": 1760000000,
      "owner_email": "user@example.com",
      "owner_name": "User",
      "owner_contact_email": "user@example.com"
    }
  ]
}
```

`last_modified` is RFC 3339; `mtime` is the same instant as Unix seconds.
Requires `library.read`.

## Get a library (v2.1) {#get-api-v2-1-repos-repo-id}

`GET /api/v2.1/repos/{repo_id}/`

One item in the list shape above. Requires `library.read`.

## Delete a library (v2.1) {#delete-api-v2-1-repos-repo-id}

`DELETE /api/v2.1/repos/{repo_id}/`

Same operation as the legacy path. **Response `200`** — `"success"`. Requires
`library.delete`.

## Get the default library {#get-api2-default-repo}

`GET /api2/default-repo/`

Seahub's primary-library lookup.

**Response `200`** — `{"exists": true, "repo_id": "…"}` or `{"exists": false}`.
Requires `library.read`.

## Create the default library {#post-api2-default-repo}

`POST /api2/default-repo/`

Finds the user's default library, creating one named "My Library" when the user
owns none.

**Response `200`** — `{"repo_id": "…"}`. Requires `library.create`.

## Mint sync tokens {#get-api2-repo-tokens}

`GET /api2/repo-tokens/?repos=<id>,<id>`

Returns one repository sync token per requested library the caller can read.
Tokens the caller cannot mint are omitted rather than failing the batch.

**Response `200`**

```json
{ "…": "…" }
```

The map is `repo_id → token`. Tokens are accepted only by `/seafhttp/`
(`Seafile-Repo-Token`), expire after `auth.sync_token_ttl_days`, and die with
their library. Requires the `sync.token` capability, which implies `sync.write`
and `sync.read`. Because minting is a state change served on `GET`, a browser
session must send `X-CSRFToken`; bearer credentials need nothing extra.

**Seafile** — this is seahub's legacy `/api2/repo-tokens/`; the official API
reference documents the newer `/api/v2.1/repos/{repo_id}/repo-api-tokens/`
surface instead, which Nanofile does not implement. Use an API key when you need
a long-lived, scoped HTTP credential.

## Download info {#get-api2-repos-repo-id-download-info}

`GET /api2/repos/{repo_id}/download-info/`

The sync client's handshake: it returns the server-side library metadata plus a
fresh sync token.

**Response `200`**

```json
{
  "repo_id": "…",
  "repo_name": "Docs",
  "token": "…",
  "email": "user@example.com",
  "relay_id": null,
  "relay_addr": null,
  "relay_port": null,
  "enc_version": 0,
  "encrypted": "",
  "repo_version": 1
}
```

For an encrypted library `encrypted` is `1` and `magic`, `random_key`, `salt`
appear as applicable. Requires read permission and the `sync.token` capability.

## Upload link {#get-api2-repos-repo-id-upload-link}

`GET /api2/repos/{repo_id}/upload-link/?p=<dir>&from=web&replace=1`

Returns an absolute URL the caller posts files to.

| Query | Meaning |
| --- | --- |
| `p` | Target directory, default `/`. |
| `from` | `web` returns the browser form endpoint (`/upload-aj/<token>`); otherwise the client endpoint (`/upload-api/<token>`). |
| `replace` | `1` asks the uploader to replace an existing file instead of failing. |

**Response `200`** — the URL as a JSON string. Requires write permission and
`file.write`. Encrypted libraries refuse upload links.

The link's token is valid for one directory; see
[Uploads and downloads](/reference/api/transfers#post-upload-api-token).

## Update link {#get-api2-repos-repo-id-update-link}

`GET /api2/repos/{repo_id}/update-link/?p=<dir>&from=web`

Returns an absolute URL for updating existing files; the body is a JSON string,
as for the upload link. Requires write permission and `file.write`.

## Upload blocks link {#get-api2-repos-repo-id-upload-blks-link}

`GET /api2/repos/{repo_id}/upload-blks-link/?p=<dir>`

Returns the URL for the chunked block upload endpoints used by the mobile
clients. Encrypted libraries refuse it. Requires write permission and
`file.write`.

## Update blocks link {#get-api2-repos-repo-id-update-blks-link}

`GET /api2/repos/{repo_id}/update-blks-link/?p=<dir>`

The update counterpart of the upload-blocks link, for updating an existing file
block by block. Requires write permission and `file.write`.

## Block download link {#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link}

`GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/`

Returns an absolute URL that serves one block of a file, used by mobile clients
that download blocks individually.

**Response `200`** — the URL as a JSON string. Requires read permission and the
`file.write` capability (the minted link is a transfer credential).

## Set an encrypted library's password {#post-api-v2-1-repos-repo-id-set-password}

`POST /api/v2.1/repos/{repo_id}/set-password/`

Verifies the password of an encrypted library before a client downloads from it.

**Body** (JSON) — `{"password": "…"}`.

**Response `200`** — `{"success": true}`. A wrong password answers `440` with
`{"error_msg": "repo password is required"}`. Repeated failures answer `429`.
Requires read permission and the `library.password` capability.

**Seafile** — same route. The legacy `/api2/repos/{repo_id}/?op=setpassword`
form answers `400` instead, because the desktop client branches on status and iOS
matches the message.

## Change an encrypted library's password {#put-api-v2-1-repos-repo-id-set-password}

`PUT /api/v2.1/repos/{repo_id}/set-password/?operation=change-password`

Rotates the library-wide encryption password.

**Body** (JSON) — `{"old_password": "…", "new_password": "…"}`.

**Response `200`** — `{"success": true}`. Owner only, because rotating the
password rewrites the library-wide key material for every member. Repeated wrong
old passwords answer `429`.

With `?operation=check-password` the endpoint instead reports whether the current
user has already supplied the password: `{"is_set": true|false}`. Any other
operation is a `400`.

**Seafile** — same route and operations.
