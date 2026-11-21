# Uploads and downloads

The endpoints that move bytes. Most are not authenticated with an account token
directly: the API mints a short-lived **capability URL** (`/download-api/<token>`,
`/upload-api/<token>`, `/blks/<token>/…`) and the client fetches or posts to it.
The link endpoints live on [Libraries](/reference/api/libraries#get-api2-repos-repo-id-upload-link).

Source: `server/src/handler/web/upload.rs`,
`server/src/handler/web/download.rs`,
`server/src/handler/web/zip_download.rs`,
`server/src/handler/file.rs`; tests: `server/tests/sync_test.rs`,
`server/tests/upload_link_test.rs`.

## How a transfer works

1. The client calls `GET /api2/repos/{repo_id}/upload-link/?p=/dir` (or
   `.../update-link/`), which returns an absolute URL carrying a token bound to
   that library, user and directory.
2. The client posts `multipart/form-data` to that URL. The link endpoints accept
   the same form: an optional `parent_dir` (upload) or `target_file` (update), a
   `relative_path`, a `replace` flag, and one `file` part per file.
3. For a download, `GET /api2/repos/{repo_id}/file/?p=…` returns a
   `/download-api/<token>` URL; fetching it streams the bytes and supports
   `Range` and `If-None-Match`.

Tokens are in-memory and short lived. They are re-checked against the library's
current membership on use, so revoking access invalidates outstanding links.

## Download file bytes {#get-repos-repo-id-files-path}

`GET /repos/{repo_id}/files/{path}?dl=<0|1>`

Streams a file's bytes with the caller's own credential (account token, session
or API key). `dl=1` forces `Content-Disposition: attachment`; the default is
inline, which is what the preview uses.

Supports `Range` (single range; `416` carries `Content-Range: bytes */<size>`)
and `If-None-Match` against the file's FS object id. Requires `file.read`.

## Download a file by link {#get-download-api-token}

`GET /download-api/{token}`

Streams the file named when the token was minted. Same `Range` and
`If-None-Match` handling as above. The token's permission is re-checked on every
request; an invalid or expired token is a `400`.

## Download one block {#get-blks-token-file-id-block-id}

`GET /blks/{token}/{file_id}/{block_id}`

Serves one block of a file, for clients that decrypt block by block. The token
is minted by [`GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/`](/reference/api/libraries#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link)
and is bound to the library and caller, not to a single block: while it is valid
it can serve any block of any file the caller may read in that library. The
endpoint still checks that the requested block belongs to the file named in the
path.

## Upload a file {#post-upload-api-token}

`POST /upload-api/{token}`

Uploads one or more files to the directory named by `parent_dir`, which
defaults to the directory bound into the token when omitted. The body is
`multipart/form-data`:

| Field | Required | Meaning |
| --- | --- | --- |
| `file` | yes | File contents; the part's filename is the entry name. Repeat the part for multiple files. |
| `parent_dir` | no | Target directory; defaults to the token's directory (the library root in the session form). It must exist or be created below the library root. |
| `relative_path` | no | Sub-path below the target; must not start with `/`. `//` in `parent_dir` is rejected. |
| `replace` | no | `1`/`true` replaces an existing file; a directory at that path answers `409`. |
| `last_modify` | no | RFC 3339 mtime for a single file. |
| `last_modify_list` | no | Comma-separated RFC 3339 mtimes, one per file. |

`POST /upload-aj/{token}` answers a JSON array of the committed files. The
`-api` form answers the file id as plain text, or tab-separated ids for a
multi-file request; `ret-json` switches it to the same JSON array.

```json
[{ "id": "…", "name": "notes.txt", "size": 1204 }]
```

**Resumable uploads** — send `Content-Range: bytes <start>-<end>/<total>` and
`Content-Length`; each chunk is staged in a temp file and the final chunk
commits. A chunked request carries exactly one file. `/upload-aj/`,
`/upload-aj/{token}`, `/upload-api/{token}`, `/update-aj/`,
`/update-aj/{token}`, `/update-api/` and `/update-api/{token}` implement this;
`/api2/repos/{repo_id}/file/` rejects a request carrying `Content-Range` with
`400`. See
[uploaded bytes](#get-api-v2-1-repos-repo-id-file-uploaded-bytes).

Requires a write token; encrypted libraries need the library key to be cached
first (the upload link request does that; a shareable upload link is refused for
an encrypted library).

## Upload a file from the browser {#post-upload-aj-token}

`POST /upload-aj/{token}`

The browser form target, same multipart fields and response as
[`POST /upload-api/{token}`](#post-upload-api-token). The web interface reaches
it through the `from=web` upload link.

`POST /upload-aj/`

The no-token variant of the same handler, authenticated by the browser session
instead of a token; it names the library through the `repo_id` field. Without a
minted token there is no bound directory, so a missing `parent_dir` means the
library root.

## Update a file {#post-update-api-token}

`POST /update-api/{token}`

Replaces an existing file's contents. Multipart body with:

| Field | Required | Meaning |
| --- | --- | --- |
| `file` | yes | New contents; exactly one file part. |
| `target_file` | yes | Absolute path of the existing file. A missing target answers `441`. |
| `relative_path` | no | Sub-path below the file's directory. |
| `last_modify` | no | RFC 3339 mtime. |
| `head` | no | Accepted for the desktop client's optimistic base; the commit is serialized on the current HEAD. |

`replace` is kept for symmetry; an update always replaces. A request without
`file` (or `target_file`) answers `400` and changes nothing. The default success
body is the file id as plain text; `ret-json` answers
`[{"id": "…", "name": "…", "size": 1204}]`.

`POST /update-aj/{token}` and `POST /update-aj/` (the session form) take the
same fields and answer the JSON array.

`POST /update-api/` is the session counterpart; it names the file with `p` /
`path` instead of `target_file` and answers the JSON array. It is a Nanofile
browser extension, not part of the upstream fileserver surface.

`/update-api/{token}`, `/update-aj/{token}`, `/update-aj/` and `/update-api/`
also implement resumable uploads (`Content-Range` with one file part); see
[Upload a file](#post-upload-api-token).

`POST /update-aj/{token}`

`POST /update-aj/`

`POST /update-api/`

## Upload blocks {#post-upload-blks-api-token}

`POST /upload-blks-api/{token}`

The mobile clients' block-level upload, minted by the upload-blocks link.

Without `commitonly`, each `file` part is one block: the part's filename is the
block id and must equal `sha1(contents)`; a mismatch is a `400`. Every block is
charged to the quota as it is stored (`443` when over).

With `commitonly`, the request is a form with `parent_dir`, `file_name`,
`blockids` (a JSON array), `file_size` and an optional `replace`; the server
assembles the file from blocks already uploaded. `blockids` may be empty only
for a zero-byte file. The success body is the file id as plain text — or
`[{"id": "…", "name": "…", "size": 0}]` when `ret-json` is present. With
`replace=1`, a `file_name` that names an existing directory is refused with
`409`: a file never replaces a collection.

Encrypted libraries are refused: this API names blocks by the plaintext SHA-1,
which cannot hold for server-side encryption.

### Raw blocks {#post-upload-raw-blks-api-token}

`POST /upload-raw-blks-api/{token}`

The raw half of the mobile block flow, minted by `POST
/api2/repos/{repo_id}/upload-blks-link/` together with the commit URL. Each
`file` part is one block: the part's filename is the block id and must equal
`sha1(contents)`. The response is `{"success": true}`; the block list is
committed through `/upload-blks-api/{token}?commitonly=true`.

## Update blocks {#post-update-blks-api-token}

`POST /update-blks-api/{token}`

The update counterpart, minted by the update-blocks link (`GET
/api2/repos/{repo_id}/update-blks-link/`). The body is the commit form:
`target_file` (the existing file's absolute path), `blockids` (a JSON array)
and `file_size`; `commitonly` goes in the query string. The server replaces
that file in place. A missing target answers `441`
(`SEAF_HTTP_RES_NOT_EXISTS`), and a target that is a directory is refused with
`409`.

The block ids must already be in the store (upload them through the
`/upload-blks-api/{token}` block phase first). Encrypted libraries are refused
for the same reason as the upload form.

## Uploaded bytes {#get-api-v2-1-repos-repo-id-file-uploaded-bytes}

`GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/?file_name=<name>&parent_dir=<dir>&blockids=<json>`

Resumable-upload state: how many bytes of this file the server already holds.
A staged temp file's offset wins over the count of uploaded blocks.

**Response `200`** — `{"uploadedBytes": 1048576}`, with
`Accept-Ranges: bytes`.

Requires `file.write`.

## Upload progress {#get-upload-progress}

`GET /upload_progress`

A compatibility shim for the web interface's progress polling. The answer is
always `{"progress": 100}`: uploads are synchronous per request.

## Index progress {#get-idx-progress}

`GET /idx_progress`

The indexing counterpart of the progress shim; always `{"progress": 100}` when
full-text indexing is enabled.

## Start a ZIP download {#post-api-v2-1-repos-repo-id-zip-task}

`POST /api/v2.1/repos/{repo_id}/zip-task/`

Creates a token for a zip archive of selected entries.

**Body** — JSON `{"parent_dir": "/", "dirents": ["a", "b"]}`, or
`multipart/form-data` with a `parent_dir` field and repeated `dirents` fields
(what the web interface posts).

**Response `200`** — `{"zip_token": "…"}`. The archive is fetched from
[`GET /zip/{token}`](#get-zip-token).

The requested names are de-duplicated and bounded; an empty list is a `400`, a
selection with no files a `404`, and exceeding the per-archive entry/byte caps
`429`. Requires `file.read`.

## Download the ZIP archive {#get-zip-token}

`GET /zip/{token}`

Streams the archive and consumes the token. The entry list and the user's
permission are re-checked when the token is used, so a revoked user cannot
download an archive minted before the revocation. One download per token.

**Seafile** — the transfer flow and field names match seahub. Differences:
the returned upload/update URLs are `/upload-api/<token>` and
`/update-api/<token>` rather than `/seafhttp/upload-api/<token>` (clients use the
returned URL verbatim); `POST /api2/repos/{repo_id}/file/` also accepts a direct
multipart upload, which the official reference does not document; the
session-only `/upload-aj/`, `/update-aj/` and `/update-api/` endpoints are
Nanofile extensions; a missing `parent_dir` falls back to the directory bound
into the token, which the fileserver instead rejects; `head` on an update is
accepted but the commit is serialized on the current HEAD; `replace` also accepts
`true`.
