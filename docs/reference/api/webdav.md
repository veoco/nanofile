# WebDAV

Mount a library as a network drive. The implementation is a minimal WebDAV
provider aimed at the clients Windows, macOS, davfs2/gvfs and mobile apps use;
it is not a general-purpose WebDAV server.

Source: `server/src/webdav/`; tests: `server/tests/webdav_test.rs`.

## Endpoint

```
/dav/{repo_id}/
/dav/{repo_id}/{path}
```

The library root is `/dav/{repo_id}/`; `{path}` is the path inside the library,
percent-encoded per segment. The official Seafile WebDAV extension uses
`/seafdav`; Nanofile uses `/dav` and the library id in the path, so every address
is self-contained.

## Authentication

HTTP Basic: the username is the account's **email** and the password is a
**WebDAV API key**, created under *Settings → API keys* with a WebDAV preset.
This differs from Seafile, where the password is the account password: Nanofile
keys are scoped (`webdav.read` for a read-only mount, `webdav.read` +
`webdav.write` for read-write) and can be revoked independently.

A failed request answers `401` with
`WWW-Authenticate: Basic realm="nanofile webdav"`; repeated failures from one
address are throttled (`429`). Requests
are refused with `403` when `server.webdav_enabled` is off or when the library is
encrypted — WebDAV has no way to carry the library password.

## Methods

| Method | Behaviour |
| --- | --- |
| `OPTIONS` | `200`, `DAV: 1, 2`, `Allow: OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, PROPFIND, PROPPATCH, MOVE, COPY, LOCK, UNLOCK`. |
| `PROPFIND` | `207 Multi-Status` listing the resource and, with a depth, its descendants. |
| `GET` | File bytes; `405` for a collection. |
| `HEAD` | The same headers as `GET`, no body. |
| `PUT` | Creates or replaces a file (`201` created, `204` replaced). |
| `MKCOL` | Creates a directory (`201`); `405` if it exists, `409` if the parent is missing. |
| `DELETE` | Removes a file or a directory tree (`204`). |
| `MOVE` | Moves inside the library (`201` created, `204` replaced). |
| `COPY` | Copies inside the library (`201` created, `204` replaced). |
| `PROPPATCH` | Accepted and answered `207` with an empty result; no property is stored. |
| `LOCK` | Accepted, answers `200` with a generated `opaquelocktoken`; locks are never enforced. |
| `UNLOCK` | `204`. |

Write methods require the key's `webdav.write` capability and `rw` permission on
the library; they answer `403` otherwise.

## PROPFIND

The request body may be empty, `<D:propfind><D:allprop/></D:propfind>`,
`<D:propname/>`, or a `<D:prop>` selection. Requested and returned properties:

| Property | On | Value |
| --- | --- | --- |
| `D:resourcetype` | every resource | `<D:collection/>` for directories, empty for files |
| `D:displayname` | every resource | The library name for the root, otherwise the entry name |
| `D:getcontentlength` | files | Size in bytes; **absent for collections** |
| `D:getlastmodified` | every resource | RFC 1123 date |
| `D:getetag` | files | The FS object id, quoted — the same value `GET` returns in `ETag` |

`Depth` accepts `0`, `1` and `infinity` (the default). `Depth: infinity` on a
file is a `400`. A listing whose result would exceed
`server.max_propfind_entries` (default 100,000) is refused rather than truncated:
`403` with the RFC 4918 precondition body
`<D:error xmlns:D="DAV:"><D:propfind-finite-depth/></D:error>`, which tells the
client to retry shallower. This matches seafdav when infinite depth is disabled.

`href` values are absolute paths (`/dav/{repo_id}/…`), percent-encoded, with a
trailing slash on collections.

## GET and HEAD

The response carries a strong `ETag: "<fs_id>"`, the MIME type inferred from the
name, `Cache-Control: private, max-age=604800`, and the file bytes. A single
`Range` is honoured (the reply is `206` with `Content-Range`); an unsatisfiable
range on a non-empty file answers `416` with `Content-Range: bytes */<size>`, while
a zero-length file stays `200`. `If-None-Match` is evaluated before `Range`, so a
matching validator answers `304` even when a range was requested. `HEAD` returns
the same headers without a body.

## PUT

The body is streamed into blocks as it arrives. The target's parent must exist
(`409` otherwise); an existing file is replaced, and `201` vs `204` says whether
the resource existed. There is no `If-Match`/`If-Unmodified-Since`
implementation: a concurrent write is last-writer-wins, but a locked file is
reported with `403` and the plain-text `File <path> is locked` body that the
clients parse.

## MOVE and COPY

`Destination` names the target (`/dav/{repo_id}/{path}`; an absolute URL is
accepted). The destination must be inside the same library (`403` otherwise) and
its parent must exist (`409`). `Overwrite: F` on an existing destination answers
`412`; the default `T` replaces it. Moving a directory into its own subtree is a
`403`. Same-library moves are one commit.

The copied tree's blocks are duplicated into the destination library because
blocks are stored per library.

## Known limitations

No `PROPPATCH` storage, no enforced locks, no `If-Match`/`If-Modified-Since`, no
`REPORT`, and no quotas or ACL properties. Encrypted libraries are not served at
all. These are listed on the
[compatibility page](/reference/api/compatibility#webdav).
