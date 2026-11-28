# Uploads and downloads

The endpoints that move bytes. Most are not authenticated with an account token
directly: the API mints a short-lived **capability URL**
(`/api/transfers/download/<token>`, `/upload-api/<token>`, `/blks/<token>/…`)
and the client fetches or posts to it.
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
   `/api/transfers/download/<token>` URL; fetching it streams the bytes and
   supports `Range` and `If-None-Match`.

The session forms, the token download and the progress shims below are
Nanofile-only; they live under `/api/`, beside the Seafile-compatible transfer
paths at the root.

Tokens are in-memory and short lived. They are re-checked against the library's
current membership on use, so revoking access invalidates outstanding links.

## Download file bytes {#get-repos-repo-id-files-path}

`GET /repos/{repo_id}/files/{path}?dl=<0|1>`

Streams a file's bytes with the caller's own credential (account token, session
or API key). `dl=1` forces `Content-Disposition: attachment`; the default is
inline, which is what the preview uses.

Supports `Range` (single range; `416` carries `Content-Range: bytes */<size>`)
and `If-None-Match` against a hash of the file's block list. A matching
validator answers `304` before `Range` is considered. Because the URL is stable,
the response is `Cache-Control: private, no-cache`: an edit is revalidated, never
served from a week-old cache. Requires `file.read`.

## Download a file by link {#get-download-api-token}

`GET /api/transfers/download/{token}`

Streams the file named when the token was minted. Same `Range` and
`If-None-Match` handling as above. The token's permission is re-checked on every
request; an invalid or expired token is a `400`.

The token URL is unique per issuance, so it is cacheable for an hour
(`Cache-Control: max-age=3600`).

## Download one block {#get-blks-token-block-id}

`GET /blks/{token}/{block_id}`

Serves one block of a file, for clients that decrypt block by block. Blocks are
content-addressed and immutable, so they carry `Cache-Control: max-age=3600`.

The URL keeps the fileserver's two-segment shape; the token is minted by
[`GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/`](/reference/api/libraries#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link),
is bound to the library and caller, and names the file whose blocks it may
serve. The endpoint still checks that the requested block belongs to that file.

The earlier self-contained spelling, with the file id in the path, is still
served from the Nanofile namespace for links handed out before the change:

`GET /api/transfers/blks/{token}/{file_id}/{block_id}`

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
commits. A chunked request carries exactly one file.
`/api/transfers/upload/`, `/upload-aj/{token}`, `/upload-api/{token}`,
`/api/transfers/update/`, `/update-aj/{token}` and `/update-api/{token}`
implement this; `/api2/repos/{repo_id}/file/` rejects a request carrying
`Content-Range` with `400`. See
[uploaded bytes](#get-api-v2-1-repos-repo-id-file-uploaded-bytes).

Requires a write token. For an encrypted library the library key must be cached
first: supply the library password through `?op=setpassword` (or the v2.1
`set-password/`); minting the repo upload link itself does not do that. A
shareable upload link (`/api/v2.1/upload-links/`) is refused for an encrypted
library.

## Upload a file from the browser {#post-upload-aj-token}

`POST /upload-aj/{token}`

The browser form target, same multipart fields and response as
[`POST /upload-api/{token}`](#post-upload-api-token). The web interface reaches
it through the `from=web` upload link.

`POST /api/transfers/upload/`

The Nanofile-only session form of the same handler, authenticated by the browser
session instead of a token; it names the library through the `repo_id` field.
Without a minted token there is no bound directory, so a missing `parent_dir`
means the library root. The web interface reaches this form when it cannot mint
an upload link.

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

`POST /update-aj/{token}` takes the same fields and answers the JSON array.

`POST /api/transfers/update/` is the Nanofile-only session counterpart: it takes
`repo_id`, `csrf_token` and one `file` part, and names the target with
`target_file` or with `p` / `path`; it answers the JSON array.

`/update-api/{token}`, `/update-aj/{token}` and `/api/transfers/update/` also
implement resumable uploads (`Content-Range` with one file part); see
[Upload a file](#post-upload-api-token).

`POST /update-aj/{token}`

`POST /api/transfers/update/`

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

`GET /api/transfers/upload-progress/`

A Nanofile-only compatibility shim for the web interface's progress polling. The
answer is always `{"progress": 100}`: uploads are synchronous per request.

## Index progress {#get-idx-progress}

`GET /api/transfers/index-progress/`

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

Streams the archive and consumes the token. The entry list is fixed when the
token is minted; on use the server only re-checks the user's permission, so a
revoked user cannot download an archive minted before the revocation. One
download per token.

**Seafile** — the transfer flow and field names match seahub. Differences:
the returned upload/update URLs are `/upload-api/<token>` and
`/update-api/<token>` rather than `/seafhttp/upload-api/<token>` (clients use the
returned URL verbatim); `POST /api2/repos/{repo_id}/file/` also accepts a direct
multipart upload, which the official reference does not document; the session
forms, the token download and the progress shims are Nanofile extensions under
`/api/transfers/`; a missing `parent_dir` falls back to the directory bound
into the token, which the fileserver instead rejects; `head` on an update is
accepted but the commit is serialized on the current HEAD; `replace` also accepts
`true`.
