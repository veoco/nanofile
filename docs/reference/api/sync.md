# Sync protocol

The `/seafhttp/` API the desktop and mobile clients' daemons speak. It is a
different credential world from the REST API: requests carry a **repository sync
token**, never an account token or an API key.

The protocol rules — commit ids, FS object ids, directory order, the empty-head
sentinel — are on [Seafile compatibility](/dev/protocol); this page documents the
wire endpoints.

Source: `server/src/handler/sync/`, `server/src/middleware/auth.rs`
(`SyncAuth`); tests: `server/tests/sync_test.rs`,
`server/tests/sync_merge_test.rs`, `server/tests/client_compat_test.rs`.

## Authentication

| Header | Meaning |
| --- | --- |
| `Seafile-Repo-Token: <token>` | The repository sync token. |
| `Authorization: Token <token>` | The same token, for clients that use the standard header. |

A sync token is bound to exactly one library. A token presented for another
library's URL is `403`, as is an expired token or one whose account is no longer
active. `head-commits-multi` is the one unauthenticated endpoint (see below).

## Status codes

Besides the usual codes, the daemon depends on these:

| Status | Meaning |
| --- | --- |
| `400` | Malformed request, or a missing `token` header (`token is null`). |
| `403` | No permission, cross-library token, or `File <path> is locked`. |
| `443` | The upload would exceed the owner's storage quota. |
| `444` | The library was deleted. |
| `446` | Blocks are missing for an uploaded file. |
| `441` | The write target does not exist. |

## Protocol version {#get-seafhttp-protocol-version}

`GET /seafhttp/protocol-version`

**Response `200`** — `{"version": 2}`.

## Head commit {#get-seafhttp-repo-repo-id-commit-head}

`GET /seafhttp/repo/{repo_id}/commit/HEAD`

**Response `200`** — `{"is_corrupted": 0, "head_commit_id": "…"}`.

A library with no commit yet answers the all-zero SHA-1 (`000…0`); a deleted
library answers `444`, which the client maps to "library deleted on server"
instead of retrying a `404` forever.

## Update the branch {#put-seafhttp-repo-repo-id-commit-head}

`PUT /seafhttp/repo/{repo_id}/commit/HEAD?head=<commit_id>`

Moves the library's HEAD to `commit_id`, which must already have been uploaded.
This is the commit point of a sync: the blocks and FS objects uploaded before
this call become reachable.

**Response `200`** — empty body. Requires write permission. If HEAD moved since
the client fetched it, the server performs the same-server merge seafile does: a
clean merge produces a merge commit (description `Auto merge by system`) and the
branch ends at the merged tree, never at a lost update. Missing blocks answer
`446` before the swap.

## Get a commit {#get-seafhttp-repo-repo-id-commit-commit-id}

`GET /seafhttp/repo/{repo_id}/commit/{commit_id}`

Returns the commit object as JSON (`application/json`), in Seafile's
`CommitData` shape including the encrypted-library metadata when applicable.
The all-zero id is answered with a synthesized empty commit, which is what a
headless library's clients read.

## Upload a commit {#put-seafhttp-repo-repo-id-commit-commit-id}

`PUT /seafhttp/repo/{repo_id}/commit/{commit_id}`

Stores one commit object. The body is the JSON the client computed the id from.

**Response `200`** — empty body. The server rejects `400` when the body's
`repo_id` disagrees with the URL, the id is malformed, the id disagrees with the
URL, or the id is not the hash of the commit's fields. The root id is not
validated here; it is checked at branch update.

## Head commits for many libraries {#post-seafhttp-repo-head-commits-multi}

`POST /seafhttp/repo/head-commits-multi/`

Body: a JSON array of library ids (at most 4096). **Response `200`** — an object
mapping each id to its head commit id.

This endpoint is **unauthenticated** by protocol: the daemon calls it with no
credentials, so it validates ids strictly instead of trusting the caller.

## List FS object ids {#get-seafhttp-repo-repo-id-fs-id-list}

`GET /seafhttp/repo/{repo_id}/fs-id-list/?server-head=<id>&client-head=<id>&dir-only=<0|1>`

The FS object ids the client must download. With a `client-head`, returns only
the difference between the two trees; without it, the whole tree. `dir-only`
limits the answer to directory objects.

**Response `200`** — a JSON array of 40-hex ids. A missing `server-head` or an
invalid id is a `400`; an empty head returns `[]`.

## Pack FS objects {#post-seafhttp-repo-repo-id-pack-fs}

`POST /seafhttp/repo/{repo_id}/pack-fs/`

Body: a JSON array of FS object ids (older clients send a URL-encoded
`fs_ids` form). **Response `200`** — `application/octet-stream`, a zlib-packed
bundle of the requested objects.

A requested id the server cannot read fails the request with `500` rather than
returning a partial pack: the daemon retries a short pack immediately and
without a bound, so a silent subset would loop forever.

## Check FS objects {#post-seafhttp-repo-repo-id-check-fs}

`POST /seafhttp/repo/{repo_id}/check-fs/`

Body: a JSON array of FS object ids. **Response `200`** —
`{"missing": ["…"]}`.

## Receive FS objects {#post-seafhttp-repo-repo-id-recv-fs}

`POST /seafhttp/repo/{repo_id}/recv-fs/`

Body: a packed bundle of FS objects (the same encoding `pack-fs` produces).
Objects are stored under the id they were sent with; with
`sync.verify_fs_objects = strict` (the default) an object whose id does not
match its content is rejected with `400`.

**Response `200`** — empty body.

## Check blocks {#post-seafhttp-repo-repo-id-check-blocks}

`POST /seafhttp/repo/{repo_id}/check-blocks/`

Body: a JSON array of block ids (or a URL-encoded `block_ids` form).
**Response `200`** — the ids the server does not have, as a JSON array.
Requires write permission, because the answer is a block-existence oracle.

## Get a block {#get-seafhttp-repo-repo-id-block-block-id}

`GET /seafhttp/repo/{repo_id}/block/{block_id}`

**Response `200`** — the block's raw bytes. A block that exists for another
library but not this one is a `404`: blocks are stored per library, and a token
for library A must not read library B's content.

## Upload a block {#put-seafhttp-repo-repo-id-block-block-id}

`PUT /seafhttp/repo/{repo_id}/block/{block_id}`

Body: the block's raw bytes. The id must equal the SHA-1 of the body (`400`
otherwise). The upload is charged to the user's quota as an uncommitted write
(`443` when over); the reservation is released when the branch is updated.

**Response `200`** — empty body. Requires write permission.

## Block map {#get-seafhttp-repo-repo-id-block-map-file-id}

`GET /seafhttp/repo/{repo_id}/block-map/{file_id}`

The block offsets of one file object. **Response `200`** — a JSON array of
integers.

## Permission check {#get-seafhttp-repo-repo-id-permission-check}

`GET /seafhttp/repo/{repo_id}/permission-check/?op=<upload|download>&client_id=<id>`

**Response `200`** — empty body. A read-only member asking for `upload` gets
`403` with `{"reason": "no write permission"}`, which the daemon distinguishes
from a generic access-denied. A deleted library is `444`; `op` must be
`upload` or `download`, and `client_id`, when sent, must be 40 characters.

## Quota check {#get-seafhttp-repo-repo-id-quota-check}

`GET /seafhttp/repo/{repo_id}/quota-check/?delta=<bytes>`

Called before an upload. **Response `200`** when the delta fits; `443` when it
does not; `400` for a missing or invalid delta.

## Notification JWT {#get-seafhttp-repo-repo-id-jwt-token}

`GET /seafhttp/repo/{repo_id}/jwt-token`

**Response `200`** — `{"jwt_token": "…"}` for the notification WebSocket
([Notification WebSocket](/reference/api/notification)). The token is HS256,
lives 3 days, and carries `repo_id`, `username` and `exp`.
`404` when the notification service is not configured.

## Lock a file {#put-seafhttp-repo-repo-id-lock-file}

`PUT /seafhttp/repo/{repo_id}/lock-file?p=<path>`

**Response `200`** — `{"success": true}`. Requires write permission. Locks are
advisory; see [Files](/reference/api/files#put-api2-repos-repo-id-file).

## Unlock a file {#put-seafhttp-repo-repo-id-unlock-file}

`PUT /seafhttp/repo/{repo_id}/unlock-file?p=<path>`

**Response `200`** — `{"success": true}`. Requires write permission.

## List locked files {#post-seafhttp-repo-locked-files}

`POST /seafhttp/repo/locked-files`

Body: a JSON array of `{"repo_id", "token", "ts"}` entries.

**Response `200`** — an array of
`{"repo_id", "ts", "locked_files": [{"path", "by_me"}]}`, where `by_me` is `1`
for the caller's own locks and `0` otherwise.

## Folder permissions {#post-seafhttp-repo-folder-perm}

`POST /seafhttp/repo/folder-perm`

Body: a JSON array of `{"repo_id", "token", "ts"}` entries.

**Response `200`** — an array of
`{"repo_id", "ts", "user_perms": [{"path": "/", "permission": "rw"}], "group_perms": []}`.
Nanofile has no per-folder shares, so the only entry is the root permission and
`group_perms` is always empty. An invalid token yields empty permission lists
rather than an error.

## Accessible libraries {#get-seafhttp-accessible-repos}

`GET /seafhttp/accessible-repos`

Every library the token's user can sync, with a usable token for each.

**Response `200`**

```json
[
  { "repo_id": "…", "repo_name": "Docs", "repo_desc": "", "owner_email": "user@example.com", "token": "…", "permission": "rw" }
]
```

**Seafile** — the `/seafhttp/` paths, parameters and status codes match the
seafile-server fileserver. Nanofile does not implement the relay/`jwt-token`
variants for a separate seafile-server, and `folder-perm` always reports
root-level permissions because per-folder sharing does not exist here.
