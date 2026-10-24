# Directories

Listing a directory and creating, renaming, moving, deleting or promoting a
folder. File contents are on [Files and batch operations](/reference/api/files)
and transfers on [Uploads and downloads](/reference/api/transfers).

Directory listings are where the Seafile clients are most sensitive: the legacy
response carries an `oid` header and a `dir_perm` header, and every entry carries
the caller's `permission`. See
[Seafile compatibility](/dev/protocol#listing-responses) for why.

Source: `server/src/handler/dir.rs`, `server/src/service/fs/dir.rs`;
tests: `server/tests/client_compat_test.rs`, `server/tests/api_v21_test.rs`.

## List items in a directory {#get-api2-repos-repo-id-dir}

`GET /api2/repos/{repo_id}/dir/?p=<path>&t=<f|d>&recursive=<0|1>`

Lists one directory. The path is absolute; `/` is the root.

| Query | Meaning |
| --- | --- |
| `p` | Directory path, default `/`. |
| `t` | `f` files only, `d` directories only. Any other value is a `400`. |
| `recursive` | `1` walks the whole subtree depth-first. `_Internal` is hidden. |
| `reloaddir` | Ignored on this endpoint; see the write operations below. |

**Response `200`** — a bare JSON array of dirents:

```json
[
  { "id": "…", "type": "dir", "name": "photos", "size": 0, "mtime": 1760000000, "permission": "rw", "modifier_email": "" },
  { "id": "…", "type": "file", "name": "notes.txt", "size": 1204, "mtime": 1760000000, "permission": "rw", "modifier_email": "user@example.com" }
]
```

Directories come first, then files, each sorted by lowercased name — seahub's
display order, not the stored object order. Recursive listings are depth-first
and additionally carry `parent_dir`; their file entries also carry
`modifier_name` and `modifier_contact_email`.

**Headers** — `oid` is the listed directory's FS object id (40 hex characters),
and `dir_perm` is `"rw"` or `"r"`. A library with no commit yet answers an empty
list with `oid: 0000000000000000000000000000000000000000`.

Requires read permission and `file.read`.

**Seafile** — same route, order and headers. `modifier_email` is present on every
entry, where seahub records it for files only; the official clients ignore it on
a directory.

## Create or rename a directory {#post-api2-repos-repo-id-dir}

`POST /api2/repos/{repo_id}/dir/`

Creates a directory, or renames one when `operation=rename`. The body may be
JSON, URL-encoded form or multipart.

| Field | Required | Meaning |
| --- | --- | --- |
| `p` | yes | The new directory's absolute path (may also be the `p` query parameter). |
| `operation` | no | `rename` renames `p` to `newname` in place. |
| `newname` | for rename | The new basename. |

**Response `201`** for creation, with a `Location` header pointing at the new
directory and the JSON string `"success"`. With `reloaddir=true` the response is
instead the parent directory's listing (the bare array the iOS client expects),
with the same `oid`/`dir_perm` headers. Rename answers `"success"` with `200`.

Requires write permission and `file.write`.

## Delete a directory {#delete-api2-repos-repo-id-dir}

`DELETE /api2/repos/{repo_id}/dir/?p=<path>`

Deletes the directory and everything under it, recording the deletion in the
library's trash. With `reloaddir=true` the response is the parent's listing;
otherwise `{"success": true}`.

Requires write permission and `file.delete`.

## List items in a directory (v2.1) {#get-api-v2-1-repos-repo-id-dir}

`GET /api/v2.1/repos/{repo_id}/dir/?p=<path>&t=<f|d>&recursive=<0|1>&with_thumbnail=<bool>`

The listing shape the mobile clients and the web interface use.

**Response `200`**

```json
{
  "user_perm": "rw",
  "dir_id": "…",
  "dirent_list": [
    {
      "type": "dir",
      "id": "…",
      "name": "photos",
      "mtime": 1760000000,
      "permission": "rw",
      "parent_dir": "/",
      "starred": false
    },
    {
      "type": "file",
      "id": "…",
      "name": "notes.txt",
      "size": 1204,
      "mtime": 1760000000,
      "permission": "rw",
      "parent_dir": "/",
      "starred": true,
      "modifier_email": "user@example.com",
      "modifier_name": "User",
      "modifier_contact_email": "user@example.com"
    }
  ]
}
```

`user_perm` equals every entry's `permission` (Nanofile has no per-folder
shares). With `with_thumbnail=true` each file entry gains
`encoded_thumbnail_src`, which is always the empty string; the clients treat
that as "build the thumbnail URL yourself". The recursive shape is the same.
The `oid`/`dir_perm` headers are set here too, with `dir_id` falling back to the
all-zero sentinel for a headless library.

Requires read permission and `file.read`.

## Create a directory (v2.1) {#post-api-v2-1-repos-repo-id-dir}

`POST /api/v2.1/repos/{repo_id}/dir/`

**Body** (JSON) — `{"p": "/new/folder"}`, or `p` as a query parameter.

**Response `200`** — `{"success": true}`. Requires write permission and
`file.write`.

## Delete a directory (v2.1) {#delete-api-v2-1-repos-repo-id-dir}

`DELETE /api/v2.1/repos/{repo_id}/dir/?p=<path>`

Same operation and response as the legacy path, including `reloaddir=true`.
Requires `file.delete`.

## Directory detail {#get-api-v2-1-repos-repo-id-dir-detail}

`GET /api/v2.1/repos/{repo_id}/dir/detail/?path=<path>`

Metadata for one directory. The parameter is `path`, not `p`, and the root is
not accepted.

**Response `200`**

```json
{ "repo_id": "…", "path": "/photos", "name": "photos", "mtime": 1760000000, "permission": "rw" }
```

`mtime` is Unix seconds (a number). Requires `file.read`.

**Seafile** — same route; seahub reports `mtime` as an ISO 8601 string. The
Android model declares it as a number and never calls this endpoint.

## Rename a directory {#post-api2-repos-repo-id-dir-rename}

`POST /api2/repos/{repo_id}/dir/rename/`

**Body** (JSON) — `{"repo_id": "…", "p": "/old/name", "new_name": "new-name"}`.
The library id is part of the body, unlike the other directory endpoints.

**Response `200`** — empty body. Requires write permission and `file.write`.

## Move a directory {#post-api2-repos-repo-id-dir-move}

`POST /api2/repos/{repo_id}/dir/move/`

**Body** (JSON) — `{"repo_id": "…", "p": "/old/name", "new_parent_dir": "/target"}`.
Moving to the same library is one commit. Requires `file.write`.

**Response `200`** — empty body.

## Create a library from a directory {#get-api2-repos-repo-id-dir-sub-repo}

`GET /api2/repos/{repo_id}/dir/sub_repo/?p=<path>`

Promotes one directory into a new library owned by the caller, copying its tree
and blocks. Because it is a state change on `GET`, a browser session must send
`X-CSRFToken`; bearer credentials are unaffected.

**Response `200`**

```json
{ "sub_repo_id": "…", "id": "…", "name": "photos", "desc": "", "size": 0, "encrypted": 0, "enc_version": 0, "owner": "user@example.com", "permission": "rw", "mtime": 1760000000 }
```

The new library starts with one commit. Requires `library.create` and write
permission on the source library.

## List share links in a directory {#get-api2-repos-repo-id-dir-shared-items}

`GET /api2/repos/{repo_id}/dir/shared_items/?p=<path>`

The share links created for exactly that directory path.

**Response `200`**

```json
{
  "shared_items": [
    { "share_type": "download", "token": "…", "path": "/photos", "repo_id": "…", "creator_email": "", "created_at": 1760000000 }
  ]
}
```

Requires `file.read`. Nanofile has no user/group shares, so only download links
appear.

**Seafile** — `dir/`, `dir/detail/` and `dir/shared_items/` are the documented
upstream routes. `dir/rename/`, `dir/move/` and the body-shaped `repo_id` are
Nanofile conveniences; seahub renames through `fileops` and its `dir/` POST.
