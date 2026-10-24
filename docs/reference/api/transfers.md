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
and is valid only for that block.

## Upload a file {#post-upload-api-token}

`POST /upload-api/{token}`

Uploads one or more files to the directory bound into the token. The body is
`multipart/form-data`:

| Field | Required | Meaning |
| --- | --- | --- |
| `file` | yes | File contents; the part's filename is the entry name. |
| `parent_dir` | no | Overrides the token's directory. |
| `relative_path` | no | Sub-path below the target directory. |
| `replace` | no | `1`/`true` replaces an existing file. |

**Response `200`**

```json
[{ "id": "…", "name": "notes.txt", "size": 1204 }]
```

While a resumable upload is still receiving chunks the answer is instead
`[{"name": "notes.txt", "uploaded": true}]`.

**Resumable uploads** — send `Content-Range: bytes <start>-<end>/<total>` and
`Content-Length`; each chunk is staged in a temp file and the final chunk
commits. See [uploaded bytes](#get-api-v2-1-repos-repo-id-file-uploaded-bytes).

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
instead of a token; it names the library through the `repo_id` field.

## Update a file {#post-update-api-token}

`POST /update-api/{token}`

Replaces an existing file's contents. Multipart body with:

| Field | Required | Meaning |
| --- | --- | --- |
| `file` | yes | New contents. |
| `target_file` | yes | Absolute path of the file to replace. |
| `relative_path` | no | Sub-path below the file's directory. |
| `replace` | no | Kept for symmetry; an update always replaces. |

**Response `200`** — `[{"id": "…", "name": "notes.txt", "size": 1204}]`.

A request without `target_file` is a no-op that answers `{"success": true}` and
creates nothing. Quota accounting charges the size difference, not the whole new
file.

`POST /update-aj/{token}`

`POST /update-aj/`

`POST /update-api/`

The browser variants (token and session respectively) and the session form the
web interface uses; same fields and response as `/update-api/{token}`.

## Upload blocks {#post-upload-blks-api-token}

`POST /upload-blks-api/{token}`

The mobile clients' block-level upload, minted by the upload-blocks or
update-blocks link.

Without `commitonly`, each `file` part is one block: the part's filename is the
block id and must equal `sha1(contents)`; a mismatch is a `400`. Every block is
charged to the quota as it is stored (`443` when over).

With `commitonly`, the request is a form with `parent_dir`, `file_name`,
`blockids` (a JSON array), `file_size` and an optional `replace`; the server
assembles the file from blocks already uploaded and answers
`{"id": "<file_fs_id>"}`.

Encrypted libraries are refused: this API names blocks by the plaintext SHA-1,
which cannot hold for server-side encryption.

## Uploaded bytes {#get-api-v2-1-repos-repo-id-file-uploaded-bytes}

`GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/?file_name=<name>&parent_dir=<dir>&blockids=<json>`

Resumable-upload state: how many bytes of this file the server already holds.
A staged temp file's offset wins over the count of uploaded blocks.

**Response `200`** — `{"uploadedBytes": 1048576}`, with
`Accept-Ranges: bytes`.

Requires `file.write`.

### Uploaded bytes (legacy path) {#get-api2-repos-repo-id-file-uploaded-bytes}

`GET /api2/repos/{repo_id}/file-uploaded-bytes/`

Identical to the v2.1 endpoint above, with the same query parameters and
response.

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

**Seafile** — the transfer flow and field names match seahub. Two differences:
the returned upload/update URLs are `/upload-api/<token>` and
`/update-api/<token>` rather than `/seafhttp/upload-api/<token>` (clients use the
returned URL verbatim), and `POST /api2/repos/{repo_id}/file/` also accepts a
direct multipart upload, which the official reference does not document.
