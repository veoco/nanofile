# Sharing

Share links (download a file or folder), upload links (let anonymous visitors
upload), starring, the activity feed, and the group stubs the official clients
require. Nanofile has no user-to-user library shares or group management, so the
sharing surface is entirely link based.

Source: `server/src/handler/links.rs`, `server/src/handler/share.rs`,
`server/src/handler/upload_link.rs`, `server/src/handler/starred.rs`,
`server/src/handler/activities.rs`, `server/src/handler/groups.rs`,
`server/src/handler/smart_link.rs`, `server/src/service/sharing/`;
tests: `server/tests/share_link_test.rs`, `server/tests/upload_link_test.rs`.

## Public pages

A share link's `link` field points at an HTML page, not at an API endpoint:

| URL | Serves |
| --- | --- |
| `/f/{token}` | A shared file's preview and download page. `POST` submits the password form. |
| `/d/{token}` | A shared folder's listing page. `POST` submits the password form. |
| `/d/{token}/files/{path}` | One file inside a shared folder. |
| `/u/{token}` | An upload link's anonymous upload page. `POST` uploads (with the password form). |

These pages are for browsers; API clients should use the JSON endpoints below.

## List share links {#get-api2-shared-links}

`GET /api2/shared-links/`

Every share link the caller created, restricted to libraries an API key may see.

**Response `200`**

```json
[
  {
    "token": "…",
    "link": "https://files.example.com/d/…/",
    "repo_id": "…",
    "path": "/photos",
    "created_at": 1760000000,
    "has_password": true,
    "expire_at": 1762592000,
    "s_type": "d",
    "view_cnt": 4
  }
]
```

`s_type` is `d` for a folder link and `f` for a file link. Requires
`share_link.read`.

## Create a share link {#post-api2-shared-links}

`POST /api2/shared-links/`

The legacy creation endpoint. **Body** (JSON): `repo_id`, `path`, optional
`password` and optional `expires_at` (Unix seconds).

**Response `200`** — one item in the list shape above. Requires
`share_link.write` and read access to the library; an encrypted library is refused. Refused when share downloads
are disabled (`server.share_download_enabled`) or when the library is encrypted.

## Delete a share link {#delete-api2-shared-links-token}

`DELETE /api2/shared-links/{token}`

Deletes one of the caller's links. **Response `200`** — `{"success": true}`.
Requires `share_link.write`.

## List share links (v2.1) {#get-api-v2-1-share-links}

`GET /api/v2.1/share-links/?repo_id=<id>&path=<path>`

Lists the caller's links. Both filters are optional; `path` is only meaningful
together with `repo_id`, and then the caller must be able to read that library.

**Response `200`** — an array of the same items as the legacy list. Requires
`share_link.read`.

## Create a share link (v2.1) {#post-api-v2-1-share-links}

`POST /api/v2.1/share-links/`

Accepts JSON or `application/x-www-form-urlencoded` (the desktop client posts a
form).

| Field | Required | Meaning |
| --- | --- | --- |
| `repo_id` | yes | Library id. |
| `path` | yes | Shared file or directory path. |
| `password` | no | Access password. |
| `expire_days` | no | Days until expiry (1 minimum). |
| `expiration_time` | no | RFC 3339 expiry, used when `expire_days` is absent. |
| `description` | no | Free text stored with the link. |

**Response `200`**

```json
{
  "token": "…",
  "link": "https://files.example.com/d/…/",
  "repo_id": "…",
  "repo_name": null,
  "path": "/photos",
  "obj_name": "photos",
  "s_type": "d",
  "is_dir": true,
  "username": null,
  "view_cnt": 0,
  "ctime": 1760000000,
  "expire_date": 1762592000,
  "is_expired": false,
  "has_password": false,
  "permissions": { "can_edit": false, "can_download": true, "can_upload": false },
  "password": null,
  "description": null
}
```

`repo_name`, `username` and `password` are always `null`; `permissions` is fixed
(links are download-only). Requires `share_link.write`.

## Create multiple share links {#post-api-v2-1-multi-share-links}

`POST /api/v2.1/multi-share-links/`

Identical to `POST /api/v2.1/share-links/`; the Android client posts here.

## Get a share link {#get-api-v2-1-share-links-token}

`GET /api/v2.1/share-links/{token}/`

One of the caller's links, in the creation shape plus its `view_cnt`. Requires
`share_link.read`; another user's link is a `404`.

## Update a share link {#put-api-v2-1-share-links-token}

`PUT /api/v2.1/share-links/{token}/`

Updates `password`, `expire_days` and/or `description` (JSON). Sending `null`
clears the field; omitting it leaves it alone.

**Response `200`** — the link in the legacy list shape (not the creation shape).
Requires `share_link.write`.

## Delete a share link (v2.1) {#delete-api-v2-1-share-links-token}

`DELETE /api/v2.1/share-links/{token}/`

**Response `200`** — `{"success": true}`. Requires `share_link.write`; another
user's link is a `404`.

## List upload links {#get-api2-upload-links}

`GET /api2/upload-links/`

The caller's upload links. **Response `200`**

```json
[
  {
    "token": "…",
    "link": "https://files.example.com/u/…/",
    "repo_id": "…",
    "path": "/incoming",
    "created_at": 1760000000
  }
]
```

Requires `upload_link.read`.

## Create an upload link {#post-api2-upload-links}

`POST /api2/upload-links/`

**Body** (JSON) — `repo_id`, `path`, optional `password` and `expires_at` (Unix
seconds).

**Response `200`** — one item in the legacy list shape. Requires
`upload_link.write` and write access to the library. Upload links cannot target
an encrypted library, and creation is refused when `server.share_upload_enabled`
is off.

## Delete an upload link {#delete-api2-upload-links-token}

`DELETE /api2/upload-links/{token}`

**Response `200`** — `{"success": true}`. Requires `upload_link.write`.

## List upload links (v2.1) {#get-api-v2-1-upload-links}

`GET /api/v2.1/upload-links/?repo_id=<id>&path=<path>`

Same optional filters as the share-link list.

**Response `200`** — an array of upload-link items:

```json
[{ "token": "…", "repo_id": "…", "path": "/incoming", "has_password": false, "expire_at": null, "view_cnt": 0, "description": null }]
```

Requires `upload_link.read`.

## Create an upload link (v2.1) {#post-api-v2-1-upload-links}

`POST /api/v2.1/upload-links/`

Same fields and encodings as the share-link creation.

**Response `200`** — the created link with `link`, `ctime`, `obj_name` and the
rest of the v2.1 fields. Requires `upload_link.write`.

## Get an upload link {#get-api-v2-1-upload-links-token}

`GET /api/v2.1/upload-links/{token}/`

One of the caller's upload links. Requires `upload_link.read`.

## Update an upload link {#put-api-v2-1-upload-links-token}

`PUT /api/v2.1/upload-links/{token}/`

Updates `password`, `expire_days` and/or `description`. **Response `200`** —
`{"success": true}`. Requires `upload_link.write`.

## Delete an upload link (v2.1) {#delete-api-v2-1-upload-links-token}

`DELETE /api/v2.1/upload-links/{token}/`

**Response `200`** — the bare JSON value `true` (the Android client parses it as
a boolean). Requires `upload_link.write`.

## Clean invalid upload links {#delete-api-v2-1-upload-links-clean-invalid}

`DELETE /api/v2.1/upload-links/clean-invalid/`

Deletes the caller's links whose library or path no longer exists.

**Response `200`** — `{"success": true, "deleted": 2}`. Requires
`upload_link.write`.

## Get the anonymous upload URL {#get-api-v2-1-upload-links-token-upload}

`GET /api/v2.1/upload-links/{token}/upload/`

Mints the short-lived upload URL for a link holder, no account credential
needed.

**Response `200`** — `{"upload_link": "/upload-aj/<token>"}`.

A password-protected link needs the `visited_ufs_<token>` cookie that the
password page sets; otherwise `403`. Expired links are `404`; encrypted
libraries are refused. Refused when `server.share_upload_enabled` is off.

## List a library's upload links {#get-api-v2-1-repos-repo-id-upload-links}

`GET /api/v2.1/repos/{repo_id}/upload-links/`

Every upload link created for one library. **Response `200`** — an array of
upload-link items. Requires read access and `upload_link.read`.

## List starred files {#get-api2-starredfiles}

`GET /api2/starredfiles/`

The legacy star list, intersected with libraries the caller can still access.

**Response `200`**

```json
[{ "repo_id": "…", "path": "/notes.txt", "size": 1204, "last_modified": 1760000000, "is_dir": false }]
```

Requires `star.read`.

## List starred items {#get-api-v2-1-starred-items}

`GET /api/v2.1/starred-items/`

The v2.1 star list, grouped as libraries, folders and files.

**Response `200`**

```json
{
  "starred_item_list": [
    {
      "repo_id": "…",
      "repo_name": "Docs",
      "repo_encrypted": false,
      "is_dir": false,
      "path": "/notes.txt",
      "obj_name": "notes.txt",
      "mtime": "2026-10-24T09:00:00+00:00",
      "deleted": false,
      "user_email": "user@example.com",
      "user_name": "User",
      "user_contact_email": "user@example.com"
    }
  ]
}
```

Requires `star.read`.

## Star an item {#post-api-v2-1-starred-items}

`POST /api/v2.1/starred-items/`

**Body** — JSON `{"repo_id": "…", "path": "/notes.txt"}` or a multipart body with
the same fields. The path must exist; starring `/` stars the library.

**Response `200`** — the starred item in the list shape. Requires
`star.write` and read access to the library.

## Unstar an item {#delete-api-v2-1-starred-items}

`DELETE /api/v2.1/starred-items/?repo_id=<id>&path=<path>`

**Response `200`** — `{"success": true}`. An item that is not starred is a
`404`. Requires `star.write`.

## Activity feed {#get-api-v2-1-activities}

`GET /api/v2.1/activities/?page=<n>&per_page=<n>&repo_id=<id>&op_user=<email>`

File and library events visible to the caller. `page` defaults to 1 and
`per_page` to 25 (clamped to 100); `repo_id` and `op_user` filter the feed.

**Response `200`**

```json
{
  "events": [
    {
      "id": 1,
      "op_type": "create",
      "obj_type": "file",
      "repo_id": "…",
      "repo_name": "Docs",
      "path": "/notes.txt",
      "old_path": null,
      "commit_id": "…",
      "name": "notes.txt",
      "old_name": null,
      "author_email": "user@example.com",
      "author_name": "User",
      "author_contact_email": "user@example.com",
      "login_id": "",
      "avatar_url": "https://files.example.com/avatars/user/user@example.com/resized/256/",
      "user_name": "User",
      "user_email": "user@example.com",
      "details": [{ "repo_name": "Docs" }],
      "count": 1,
      "old_repo_name": null,
      "time": "2026-10-24T09:00:00+00:00"
    }
  ],
  "total_count": 1
}
```

The avatar URL is absolute because the Android client loads it directly.
`old_path`, `old_name` and `old_repo_name` are emitted on every event (seahub
emits them conditionally); the Android client expects `old_name` to exist on
rename events. Requires `activity.read`.

**Seafile** — same route and envelope. Nanofile records no comment or share
events, so the feed only carries file and library operations.

## List groups {#get-api2-groups}

`GET /api2/groups/`

The desktop client's group dialog calls this before showing a share form. It
always answers an empty array: Nanofile has no groups and no library shares.

Requires `library.read`.

## List groups (v2.1) {#get-api-v2-1-groups}

`GET /api/v2.1/groups/?with_repos=<0|1>`

The Android client's library-list chain calls this before loading libraries; a
`404` would reject the whole chain. Always an empty array. `with_repos` accepts
only `0` or `1`; anything else is a `400`. Requires `library.read`.

## Smart link {#get-api-v2-1-smart-link}

`GET /api/v2.1/smart-link/?repo_id=<id>&path=<path>`

Returns a URL that resolves a file's current download endpoint, so a shared
"smart link" always points at the latest version.

**Response `200`** — `{"url": "https://files.example.com/api2/repos/<id>/file/?p=/notes.txt"}`.
The URL requires the reader's credential; it is not a public link. Requires
`file.read`.

**Seafile** — the official reference also documents a token-based smart-link
follow-up (`/api/v2.1/smart-links/{token}/`), which Nanofile does not implement.
