# Files and batch operations

File metadata, single-file operations, the batch copy/move/delete APIs and the
asynchronous task endpoints. Uploading and downloading bytes is on
[Uploads and downloads](/reference/api/transfers); directory listings are on
[Directories](/reference/api/directories).

Source: `server/src/handler/file.rs`, `server/src/handler/fileops.rs`,
`server/src/handler/batch.rs`, `server/src/handler/async_batch.rs`,
`server/src/handler/thumbnail.rs`, `server/src/handler/exif.rs`,
`server/src/service/fs/file.rs`; tests: `server/tests/client_compat_test.rs`,
`server/tests/api_v21_test.rs`.

## Download a file {#get-api2-repos-repo-id-file}

`GET /api2/repos/{repo_id}/file/?p=<path>&op=<download|view|downloadblks>`

Returns a time-limited download URL, not the bytes (the bytes come from
[`GET /repos/{repo_id}/files/{path}`](/reference/api/transfers#get-repos-repo-id-files-path)).

| Query | Meaning |
| --- | --- |
| `p` | File path. |
| `op` | `download` (default) and `view` both return the URL; `downloadblks` returns the block list. |

**Response `200`**

```json
"/download-api/<token>"
```

The `oid` response header is the file's FS object id. With
`op=downloadblks` the body is instead:

```json
{ "file_id": "…", "blklist": ["…"], "encrypted": false, "enc_version": 0 }
```

Requires read permission and `file.read`. Encrypted libraries return encrypted
blocks; the client decrypts them with the library password.

**Seafile** — same route and operations.

## Create, upload, rename or move a file {#post-api2-repos-repo-id-file}

`POST /api2/repos/{repo_id}/file/?p=<path>&reloaddir=<true|false>`

Seafile's overloaded file endpoint. The body's shape selects the operation:

- **`multipart/form-data` with a `file` field** — uploads the file's bytes
  directly (the web interface and the Android client). `parent_dir` names the
  target directory, `replace=1` overwrites, and `operation=rename` with
  `newname` renames `p` instead.
- **URL-encoded form** — one of:
  - `operation=create` — creates an empty file at `?p=`.
  - `operation=rename` + `newname` — renames `?p=`.
  - `operation=move` + `dst_repo` + `dst_dir` — moves `?p=` to another directory.

`reloaddir=true` answers with the affected directory's listing (the destination
directory for a move) instead of `"success"`.

**Response `200`** — `{"success": true}` or `"success"`, or the bare listing when
`reloaddir=true`.

Requires write permission and `file.write`.

## Lock or unlock a file {#put-api2-repos-repo-id-file}

`PUT /api2/repos/{repo_id}/file/?p=<path>`

Locks or unlocks a file for other sync clients. The request body is a
URL-encoded form with `operation=lock` or `operation=unlock`.

**Response `200`** — `{"success": true}`. Requires write permission and
`file.write`. Locks are advisory: the server records who holds them and rejects
conflicting writes from other clients with `403` and the plain-text body
`File <path> is locked`, but any writer may lock or unlock.

## Delete a file {#delete-api2-repos-repo-id-file}

`DELETE /api2/repos/{repo_id}/file/?p=<path>`

Deletes a file and records it in the library's trash.

**Response `200`** — `{"success": true}`. Requires write permission and
`file.delete`.

## Create a file (v2.1) {#post-api-v2-1-repos-repo-id-file}

`POST /api/v2.1/repos/{repo_id}/file/`

Creates an empty file. The path is `{"p": "/a.txt"}` in a JSON body, or `?p=`;
the iOS client sends `operation=create` in a form body. With
`operation=rename` and `newname` the endpoint renames `p` instead.

**Response `200`**

```json
{
  "type": "file",
  "repo_id": "…",
  "parent_dir": "/",
  "obj_name": "a.txt",
  "obj_id": "…",
  "size": 0,
  "mtime": "2026-10-24T09:00:00+00:00",
  "is_locked": false,
  "can_preview": true,
  "can_edit": true
}
```

Requires write permission and `file.write`.

## Delete a file (v2.1) {#delete-api-v2-1-repos-repo-id-file}

`DELETE /api/v2.1/repos/{repo_id}/file/?p=<path>`

Same operation and response as the legacy path. Requires `file.delete`.

## File detail {#get-api2-repos-repo-id-file-detail}

`GET /api2/repos/{repo_id}/file/detail/?p=<path>`

Metadata for one file, including its last modifier and star state.

**Response `200`**

```json
{
  "id": "…",
  "type": "file",
  "name": "notes.txt",
  "permission": "rw",
  "mtime": 1760000000,
  "size": 1204,
  "last_modified": "2026-10-24T09:00:00+00:00",
  "last_modifier_name": "User",
  "last_modifier_email": "user@example.com",
  "last_modifier_contact_email": "user@example.com",
  "last_modifier_avatar": "https://files.example.com/avatars/user/user@example.com/resized/72/",
  "starred": false,
  "comment_total": 0,
  "can_edit": true
}
```

`comment_total` is always `0` (Nanofile has no comments). Requires `file.read`.

## Rename a file {#post-api2-repos-repo-id-file-rename}

`POST /api2/repos/{repo_id}/file/rename/`

**Body** (JSON) — `{"repo_id": "…", "p": "/old.txt", "new_name": "new.txt"}`.

**Response `200`** — empty body. Requires write permission and `file.write`.

## Move a file {#post-api2-repos-repo-id-file-move}

`POST /api2/repos/{repo_id}/file/move/`

**Body** (JSON) — `{"repo_id": "…", "p": "/a.txt", "new_parent_dir": "/target"}`.
A move inside one library is a single commit; cross-library moves are not
supported.

**Response `200`** — empty body. Requires `file.write`.

## EXIF metadata {#get-api2-repos-repo-id-file-exif}

`GET /api2/repos/{repo_id}/file/exif/?p=<path>`

Returns the image's EXIF tags as a JSON object, or `null` when it has none (a
non-image, or a file the sandbox could not parse).

Requires `file.read`.

## Thumbnail {#get-api2-repos-repo-id-thumbnail}

`GET /api2/repos/{repo_id}/thumbnail/?p=<path>&size=<px>`

Returns an image thumbnail (JPEG when opaque, PNG when it has transparency).
`size` defaults to 48 and may not exceed 1024. The response carries a strong
`ETag` and `Cache-Control: private, max-age=604800`; `If-None-Match` matching
that ETag answers `304`.

Requires `file.read`. An encrypted library cannot be thumbnailed: its stored
bytes are ciphertext, so the attempt fails rather than revealing anything.

## Delete several files {#post-api2-repos-repo-id-fileops-delete}

`POST /api2/repos/{repo_id}/fileops/delete/?p=<dir>&reloaddir=<bool>`

Deletes several entries of one directory. The body is a URL-encoded form with
`file_names`, a colon-separated list of names.

**Response `200`** — empty body, or the directory listing when
`reloaddir=true`. An empty name list is a no-op returning `{}`.

Requires write permission and `file.delete`.

## Copy several files {#post-api2-repos-repo-id-fileops-copy}

`POST /api2/repos/{repo_id}/fileops/copy/?p=<src-dir>&reloaddir=<bool>`

**Body** (URL-encoded form) — `file_names` (colon-separated), `dst_repo` (must be
the same library) and `dst_dir`.

**Response `200`** — an array of the new entries:

```json
[{ "repo_id": "…", "parent_dir": "/target", "obj_name": "a.txt" }]
```

With `reloaddir=true` the response is the **source** directory's listing.
Requires `file.write`.

## Move several files {#post-api2-repos-repo-id-fileops-move}

`POST /api2/repos/{repo_id}/fileops/move/?p=<src-dir>&reloaddir=<bool>`

Same request and response as copy, but moves. Moving inside one library is one
commit. Requires `file.write`.

## Batch move (synchronous, v2.1) {#post-api-v2-1-repos-sync-batch-move-item}

`POST /api/v2.1/repos/sync-batch-move-item/`

**Body** (JSON)

```json
{
  "src_repo_id": "…",
  "src_parent_dir": "/a",
  "src_dirents": ["one.txt", "two"],
  "dst_repo_id": "…",
  "dst_parent_dir": "/b"
}
```

`src_repo_id` must equal `dst_repo_id`; cross-library moves answer `400`.
**Response `200`** — `{"success": true}`. Requires `file.write`.

## Batch copy (synchronous, v2.1) {#post-api-v2-1-repos-sync-batch-copy-item}

`POST /api/v2.1/repos/sync-batch-copy-item/`

Same body and limits as batch move. **Response `200`** — `{"success": true}`.
Requires `file.write`.

## Batch delete {#post-api-v2-1-repos-batch-delete-item}

`POST /api/v2.1/repos/batch-delete-item/`

`DELETE /api/v2.1/repos/batch-delete-item/`

Both methods are accepted; the body is the same.

**Body** (JSON) — `{"repo_id": "…", "parent_dir": "/a", "dirents": ["one.txt"]}`.

**Response `200`** — `{"success": true}`. Requires `file.delete`.

**Seafile** — the official reference documents the `DELETE` form; Nanofile
accepts both because the desktop client uses `DELETE` and the web UI `POST`.

## Batch copy (asynchronous, v2.1) {#post-api-v2-1-repos-async-batch-copy-item}

`POST /api/v2.1/repos/async-batch-copy-item/`

Same body as the synchronous copy. Starts a background task.

**Response `200`** — `{"task_id": "…"}`. Poll it with
[`query-copy-move-progress`](#get-api-v2-1-query-copy-move-progress). Requires
`file.write`.

## Batch move (asynchronous, v2.1) {#post-api-v2-1-repos-async-batch-move-item}

`POST /api/v2.1/repos/async-batch-move-item/`

Same body as the synchronous move. **Response `200`** — `{"task_id": "…"}`.
Requires `file.write`.

## Copy or move task {#post-api-v2-1-copy-move-task}

`POST /api/v2.1/copy-move-task/`

The desktop client's combined entry point.

**Body** (JSON) — the batch body plus `"operation": "copy"` (default) or
`"move"`.

**Response `200`** — `{"task_id": "…"}`. Requires `file.write`.

## Query copy or move progress {#get-api-v2-1-query-copy-move-progress}

`GET /api/v2.1/query-copy-move-progress/?task_id=<id>`

Reports one asynchronous copy/move task. Only the task's owner can read it; an
unknown, expired or foreign id is a `404`.

**Response `200`**

```json
{
  "state": "processing",
  "done": false,
  "failed": false,
  "successful": false,
  "description": "",
  "total": 3,
  "done_count": 1,
  "failed_count": 0,
  "cancelable": false
}
```

`state` is `processing`, `completed` or `failed`. `cancelable` is always `false`:
a move is a single commit, and interrupting it would lose the entry, so there is
nothing safe to cancel.

**Seafile** — same route. The official reference also documents
`DELETE /api/v2.1/copy-move-task/` to cancel; Nanofile does not implement
cancellation.
