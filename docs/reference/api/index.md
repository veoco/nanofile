# API reference

This section documents every HTTP surface Nanofile serves: the REST API the
Seafile clients and scripts use, the sync protocol, WebDAV, the notification
WebSocket, and the transfer endpoints the web interface calls. Each page
describes the request, the response and the errors; the
[compatibility page](/reference/api/compatibility) records how each endpoint
relates to Seafile's official API reference.

The reference describes what this server actually answers. Where an endpoint is
deliberately different from upstream Seafile, the difference is stated on the
endpoint's page and collected on the compatibility page.

## Surfaces

| Prefix | What it serves |
| --- | --- |
| `/api2/` | The legacy Seafile REST API. The official desktop and mobile clients still use much of it. |
| `/api/v2.1/` | The newer REST API used by the web interface and the mobile clients. |
| `/seafhttp/` | The sync protocol between a client daemon and the server. See [Sync protocol](/reference/api/sync). |
| `/dav/{repo_id}/` | WebDAV. See [WebDAV](/reference/api/webdav). |
| `/notification` | The WebSocket the clients subscribe to for library updates. See [Notification WebSocket](/reference/api/notification). |
| `/upload-aj/`, `/upload-api/`, `/update-aj/`, `/update-api/`, `/download-api/`, `/blks/`, `/zip/`, `/repos/{repo_id}/files/` | Transfers. These are the URLs the API returns or the web interface calls; see [Uploads and downloads](/reference/api/transfers). |
| `/health` | Operational health probe; answers `200` with the process generation header. Not part of the API. |

`GET /api2/server-info/` reports the Seafile version this server advertises
(`server.version`, `8.0.0` by default). Clients gate features on that string;
the reference describes the routes this server actually serves, not only the
ones that version implies.

## Terminology

- **Library** (`repo`) — a top-level synced tree. Its id is a 36-character UUID
  and appears in every path as `{repo_id}`.
- **Dirent** — one directory entry. A dirent is a file or a directory, with its
  name, size, modification time and permission.
- **Commit** — an immutable snapshot of a library. Its id is the 40-character
  SHA-1 computed as described in [Seafile compatibility](/dev/protocol#commit-ids).
- **FS object** — a stored file or directory object; its id (`oid`, `fs_id`) is
  the SHA-1 of its serialized form. Directory objects are Seafile version 1.
- **Permission** — read-only `"r"` or read-write `"rw"`. Nanofile grants
  permissions per library (owner or member); there are no per-folder shares.

## Authentication {#authentication}

Every endpoint requires one of four credentials, unless it is marked public.

| Credential | How it is sent | Where it is accepted |
| --- | --- | --- |
| Account token | `Authorization: Bearer <token>`; the legacy `Authorization: Token <token>` is accepted too | Every `/api2/` and `/api/v2.1/` endpoint |
| Session cookie | `seahub-session` cookie, set by signing in at `/accounts/login/`; state-changing requests also need `X-CSRFToken` matching the `sfcsrftoken` cookie | The same API surfaces, and the only credential the key-management endpoints accept |
| API key | `Authorization: Bearer <key>` | The API surfaces, limited to the key's capabilities and library scope; never `/seafhttp/` |
| Sync token | `Seafile-Repo-Token: <token>` (or `Authorization: Token <token>`) | `/seafhttp/` only, bound to one library |

An account token is minted by [`POST /api2/auth-token/`](/reference/api/authentication#post-api2-auth-token)
with the account's username and password. API keys are Nanofile's own
credential; see [API keys and credentials](/reference/api/api-keys).

Public endpoints are `POST /api2/auth-token/`, `GET /api2/ping/`,
`GET /api2/server-info/`, both `/api2/client-sso-link/` endpoints and
`POST /api2/device-wiped/`. Everything else answers `401` without a credential.

A read-only library membership (`r`) satisfies read operations only; writes
answer `403`. An API key additionally needs the matching capability — an
endpoint's page names it.

## Requests

- Bodies are JSON (`application/json`), URL-encoded forms, or
  `multipart/form-data` where the endpoint documents it. An endpoint that
  accepts several encodings parses by `Content-Type`.
- Path parameters are shown as `{repo_id}`; query parameters are separate. Path
  values must be URL-encoded.
- File and directory paths are absolute (`/a/b.txt`) and are passed as `p` (or
  `path`) query parameters, not as URL segments.
- Seafile overloads `POST` on a resource path: `POST /api2/repos/{repo_id}/file/`
  creates, renames, moves, copies or reverts depending on the form's
  `operation` field. The endpoint pages list the accepted operations.
- Boolean query parameters accept `true`/`false` case-insensitively, matching
  seahub's comparison.

## Responses

Successful bodies are JSON unless the endpoint streams bytes. Errors are JSON
with an `error_msg` field:

```json
{ "error_msg": "Library not found." }
```

Two shapes are special:

- A failed authentication answers `401` with both DRF's `detail` and Seafile's
  `error_msg`, so either client style reads a message:
  `{"detail": "…", "error_msg": "…"}`.
- The login endpoint reports failures as DRF serializer errors:
  `{"non_field_errors": ["…"]}`. A locked file answers `403` with the plain
  text `File <path> is locked` because the sync daemon parses that body with a
  regular expression.

### Status codes

| Status | Meaning |
| --- | --- |
| `200` | Success. |
| `201` | Created (library creation, share-link creation). |
| `204` | Success with no body (several `DELETE`s). |
| `400` | Malformed request, missing parameter, or invalid value. |
| `401` | Missing or invalid credential. |
| `403` | Authenticated but not allowed: read-only library, missing capability, disabled account. |
| `404` | The object does not exist or is not visible to the caller. |
| `409` | Conflict, such as creating a directory that already exists. |
| `413` | The request body exceeds the configured limit. |
| `429` | Rate limited. |
| `440` | Seafile: the library password is required. |
| `441` | Seafile: the library password hash (`magic`) is required, or the write target does not exist. |
| `442` | Seafile: the uploaded entry exceeds the size limit. |
| `443` | Seafile: the storage quota is exceeded. |
| `444` | Seafile: the library was deleted. |
| `445` | Seafile: the library is corrupted. |
| `446` | Seafile: blocks are missing for an uploaded file. |
| `500` | Internal error. |
| `503` | Temporarily unavailable (maintenance or connection limit). |
| `520` | Seafile: the operation failed. |

## Ids and timestamps

- `repo_id` is a 36-character UUID. `commit_id`, `file_id`, `fs_id`, `oid` and
  `block_id` are 40-character lowercase SHA-1 hex strings.
- Most timestamps are Unix epoch **seconds** encoded as JSON numbers. A few
  newer endpoints use RFC 3339 strings (`file/detail/`, `search-file`), and
  `dir/detail/` reports a numeric `mtime` where seahub reports a string. Each
  endpoint page states which one it returns.

## Limits and policies

- JSON and form bodies are capped by `server.max_json_body_mb` (64 MiB by
  default); upload routes use `server.max_upload_size_mb` (4096 MiB).
- A request times out after `server.request_timeout_secs` (600 seconds).
- Search is limited per user per minute (`auth.search_max_per_minute`); login
  failures trigger a per-address lockout (`auth.max_login_attempts`).
- Pagination defaults and caps are documented per endpoint. `search` defaults to
  10 hits per page with a maximum of 100; `search-file` is capped at 200 hits in
  total; the activity feed defaults to 25; the trash defaults to 50.
- CORS is answered for the origins in `server.cors_allowed_origins` (the site
  URL by default) on the API and transfer routes. WebDAV deliberately does not
  get the CORS layer.

## Compatibility with Seafile

Each endpoint page ends with a **Seafile** note. The full comparison — every
operation in the official API reference, grouped the way that reference groups
them, plus the endpoints Seafile's server has but its reference does not
document — is in [Seafile API compatibility](/reference/api/compatibility).

- **Same** — the same method and path template; the response is documented on
  the endpoint page and may carry Nanofile-specific additions.
- **Differs** — the feature exists but the method, path, credential or response
  shape is different; the note says how.
- **Missing** — not implemented; the note gives the closest alternative.

## Pages

| Page | Covers |
| --- | --- |
| [Authentication](/reference/api/authentication) | Tokens, two-factor authentication, client login and SSO |
| [API keys and credentials](/reference/api/api-keys) | API keys, their capabilities, sessions and devices |
| [Accounts and administration](/reference/api/accounts) | Account info, avatars, user administration, server info |
| [Libraries](/reference/api/libraries) | Library creation and metadata, links, passwords |
| [Directories](/reference/api/directories) | Directory listings and directory operations |
| [Files and batch operations](/reference/api/files) | File metadata, file operations, batch copy/move/delete |
| [Uploads and downloads](/reference/api/transfers) | Transfer endpoints, resumable uploads, ZIP downloads |
| [History, revisions and trash](/reference/api/history) | File history, revisions and the trash |
| [Sharing](/reference/api/sharing) | Share links, upload links, stars, activity and groups |
| [Metadata and search](/reference/api/metadata) | Metadata, tags, search and indexing |
| [Sync protocol](/reference/api/sync) | `/seafhttp/` endpoints |
| [Notification WebSocket](/reference/api/notification) | The notification protocol |
| [WebDAV](/reference/api/webdav) | `/dav/` |
| [Seafile API compatibility](/reference/api/compatibility) | The endpoint-by-endpoint comparison |
