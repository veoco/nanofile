# Seafile compatibility

Nanofile implements the Seafile sync protocol and the APIs the official desktop
and mobile clients use. This page records the compatibility rules that the code
has to preserve. It documents behaviour asserted by code and tests; it does not
claim support for features that are not implemented. The endpoint-by-endpoint
comparison with Seafile's Web API is on [Seafile API compatibility](/reference/api/compatibility);
this page covers the protocol rules that page cannot.

## Commit ids

A commit id is computed exactly like Seafile's `compute_commit_id()`: SHA-1 over,
in order,

1. the root id as 40 hex characters plus a NUL,
2. the creator id as 40 hex characters plus a NUL,
3. the creator name plus a NUL, omitted entirely when the name is empty,
4. the description plus a NUL,
5. the commit time as eight big-endian bytes.

The NUL terminators are part of the hash: every string is fed as a C string
(`strlen + 1`), never as a bare byte slice. Omitting them yields ids no official
client or server would compute for the same logical commit, so a library written
through the web API and the same library written by a desktop client would
disagree on history.

Source: `server/src/domain/commit.rs`; covered by `server/tests/commit_test.rs`.

## Directory order and `fs_id`

A directory object's `fs_id` is the SHA-1 of its serialized form, so the order of
its `dirents` is part of the object's identity.

Official clients always write entries in **descending name order** (upstream
`compare_dirents()` is `strcmp(b, a)`), and they read a stored v1 object back
verbatim. A directory stored in any other order carries an id no client can
reproduce, and the next local write makes the two sides disagree about a directory
neither changed.

`nanofile verify-dir-order` walks every directory reachable from a library's HEAD
and reports objects whose stored order is not canonical. With `--repair` it
appends one ordinary commit per affected library whose root is the canonical form
of the current tree: history is not rewritten, old objects stay readable, and
clients pick the fix up as a normal update. Running it twice changes nothing the
second time.

Source: `server/src/fs/core/dir_order.rs`; covered by `server/tests/dir_order_test.rs`
and `server/tests/fs_id_compat.rs`.

## Received FS objects

A received FS object's id must equal `sha1(uncompressed JSON)` by default:
mismatches are rejected with 400. The official server performs no such check, but
every legitimate repo-v1 client sends a matching id, so strict verification is
safe and prevents a client from storing an object under an id whose content it
does not match.

The policy is `[sync] verify_fs_objects` (`NANOFILE_SYNC_VERIFY_FS_OBJECTS`):
`strict` (default), `log` or `off`. Tree walks are bounded by
`sync.max_tree_depth` and `sync.max_tree_visits` as a cycle and denial-of-service
guard.

## Credentials and capabilities

Every request is classified as one of three credentials:

- **Session** — a browser session or the account-level token a client obtained by
  logging in. It *is* the account, so it satisfies every capability, and it is the
  only credential accepted by the key-management surface.
- **API key** — carries exactly the capabilities it was granted, plus a per-library
  ceiling that can narrow a route's requirement. Granting `file.write` implies
  `file.read`; unknown routes are denied.
- **Sync token** — bound to one library by construction and never accepted by the
  API surfaces; it is refused rather than treated as an account credential.

The route-to-capability table lives in `server/src/domain/capability.rs`; a new
route that is not classified fails closed. `server/tests/capability_coverage_test.rs`
checks that the table covers every route.

## Verified against upstream

The rules on this page were checked against the upstream sources, not just
remembered. The revisions used:

| repository | revision | what it anchors |
| --- | --- | --- |
| `haiwen/seafile` | `af6539c` | commit-id computation, the client daemon's commit/fs writers |
| `haiwen/seafile-server` | `a652460` | server side of the sync API (C `http-server.c` and the Go `fileserver/`), repo operations |
| `haiwen/seahub` | `59e5b84` | `api2`/`api/v2.1` response shapes |
| `haiwen/seafile-client` | `7b5a95a` | desktop client request/parse paths |
| `haiwen/seafile-ios` | `2aedcc3` | iOS client request/parse paths |
| `haiwen/seadroid` | `9d6d490` | Android client request/parse paths |
| `haiwen/seafdav` | `11295ad` | WebDAV behaviour (wsgidav) |

Line numbers below are from those revisions; they drift, the commit ids do not.

| behaviour | upstream anchor |
| --- | --- |
| commit id input order, NUL terminators, empty name skipped | `seafile/common/commit-mgr.c:39-60`, `seafile-server/common/commit-mgr.c:41-60` |
| `creator_name` written only when set; lowercased when read | `commit-mgr.c:606-609`, `:791-794` |
| client computes the id before adding repo metadata to the commit | `seafile/daemon/repo-mgr.c:4567-4590` |
| commit upload: 400 on parse/repo mismatch, no hash check | `seafile-server/fileserver/sync_api.go:941-984` |
| `recv-fs`: 400 on a malformed pack, no content check | `sync_api.go:453-517` |
| file/dir object id is `sha1(json_dumps(obj, JSON_SORT_KEYS))`; `modifier`/`size` only on files; descending order | `seafile-server/common/fs-mgr.c:381-392`, `:1795-1820`, `:1906-1911` |
| same-repo move is one commit (add + delete, then commit) | `seafile-server/server/repo-op.c:3161-3245` |
| legacy listing: fields, `dir_perm` via `check_permission_by_path`, dirs then files sorted by `name.lower()` | `seahub/api2/views.py:2380-2462` |
| v2.1 listing: `{dirent_list, dir_id, user_perm}`, per-entry `permission`, `parent_dir`, `starred`, `modifier_email` | `seahub/api2/endpoints/dir.py:45-215`, `:343-344` |
| `/api/v2.1/search-file/`: `{"data": [{path, size, mtime(ISO), type: file/folder}]}`, unbounded, filename substring | `seahub/api2/endpoints/search_file.py` |
| `/api2/search/`: `{results, total, has_more}`, `per_page <= 100`, Pro only | `seahub/api2/views.py:452-700` |
| auth failure body is `{"detail": …}`; functional errors are `{"error_msg": …}` | `seahub/api2/authentication.py:28-33`, `seahub/api2/utils.py:49-51` |
| desktop client requires a 40-character `oid`, `dir_perm == "r"` means read-only, per-entry `permission == "r"` | `seafile-client/src/filebrowser/file-browser-requests.cpp:73-96`, `seaf-dirent.cpp:19-45` |
| desktop client renders the listing in server order | `seafile-client/src/filebrowser/file-table.cpp:926-937` |
| desktop `/api2/search/` fields: `repo_id, name, oid, last_modified|mtime, fullpath, size, is_dir` | `seafile-client/src/api/requests.cpp:939-980` |
| iOS v2.1 listing: `dirent_list`/`dir_id`/`user_perm`, per-entry `permission` decides sub-folder editability; sorts locally | `seafile-ios/Pod/Classes/SeafDir.m:264-355`, `:160-170`, `:386-409` |
| iOS search-file shape (`type == "folder"`, absolute `path`); `/api2/search/` only when `seafile-pro`+`file-search` | `seafile-ios/Pod/Classes/SeafConnection+Search.m:47-100`, `SeafConnection.m:1321-1338` |
| Android v2.1 listing model; per-entry `permission` builds the action menu; `modifier_email` | `seadroid/.../ui/repo/RepoService.java:34-38`, `.../framework/db/entities/DirentModel.java:50-78`, `.../ui/repo/sheetaction/BottomSheetMenuManager.java:190-228` |
| WebDAV ETag is the FS object id (`obj_id`), reported by GET and by the `getetag` property | `seafdav/wsgidav/seafile_dav_provider.py:153-185`, `:637-640`, `dav_provider.py:747-749` |
| conditionals are evaluated before ranges; single range only; 416 carries `Content-Range: bytes */size` | `seafdav/wsgidav/request_server.py:1477-1560`, `util.py:1485-1492` |
| `Depth: infinity` may be refused with 403 + `<D:propfind-finite-depth/>` (RFC 4918 §9.1) | `seafdav/wsgidav/request_server.py:282-296` |
| WebDAV does not serve encrypted libraries | `seafdav/wsgidav/seafile_dav_provider.py:871-874` |

## Listing responses

`/api2/repos/{id}/dir/` and `/api/v2.1/repos/{id}/dir/` report the caller's **real**
permission, because the official clients use it to decide what is editable:

- the legacy `dir_perm` header and every entry's `permission` are `"rw"` or `"r"`;
- v2.1 additionally carries `user_perm` (same value) and `dir_id`;
- nanofile has no per-folder shares, so every entry in a library shares that value.

The legacy listing is ordered like seahub (directories first, then files, each by
lowercased name) and honours `?t=f`/`?t=d`. The stored order of a directory object
is its descending-name identity order and is not a display contract.

A library with no head commit (nanofile creates those, seahub always writes an
initial `Created library` commit) answers `oid`/`dir_id` = `EMPTY_SHA1` and an empty
list. The desktop client rejects a listing whose `oid` is not 40 characters, so this
is required, not cosmetic.

Listing keys use seahub's spelling: `modifier_email` (not `modifier`), plus
`modifier_name`/`modifier_contact_email` for files.

## WebDAV validators

A file's `ETag` is the FS object id in quotes — the same value as GET and the
PROPFIND `getetag` property (seafdav reports `obj_id` in both). `If-None-Match` is
evaluated before `Range`, and its weak comparison accepts a `W/` prefix. A
`Depth: infinity`/`1` listing over `server.max_propfind_entries` is refused with
403 and the RFC 4918 `propfind-finite-depth` body, which tells the client to retry
shallower; collections carry no `getcontentlength`.

## Deliberate deviations

These differ from upstream on purpose:

- **`_Internal` is hidden from every listing** (plain, recursive, WebDAV,
  `reloaddir`). Upstream only skips it in the zip/index recursive walk (root level,
  alongside `images`), because seahub uses it for wiki/seadoc bookkeeping; nanofile
  has no such tree. A library migrated from seahub would show one fewer entry.
- **`/api/v2.1/search-file/` is capped** at the shared 200-hit budget; upstream walks
  the whole tree unbounded. Both mobile clients receive the truncated `data` list.
- **`/api2/search/` ignores the Pro-only filters** (`search_type`, `obj_type`,
  `time_from`, `size_from`, `search_path`, `with_permission`) and is served even
  when the search index is off (filename matches only).
- **Overwrite quota accounting uses `new - old`**; upstream charges the whole new
  `Content-Length` on the upload path and nothing at `update-branch`.
- **No `Created library` initial commit.** Repos start headless; `commit/HEAD`
  answers the `000…0` sentinel and `get_commit` synthesizes it (upstream always has a
  real commit).
- **`dir/detail/` reports `mtime` as a number**, seahub as an ISO string (Android's
  model declares `long` and never calls it).
- **`?oid=` revalidation** (seahub answers `"uptodate"`): not implemented; the
  official clients do not send it.
- **`If-Match`/`If-Unmodified-Since`/`If-Modified-Since` and `PROPPATCH`** are not
  implemented.
- **WebDAV answers 403 for an encrypted library**; seafdav simply omits it from the
  repo list (the request would be a 404).
- **`encoded_thumbnail_src` is always empty.** iOS and Android only use it as a
  capability signal and build the old `/api2/repos/{id}/thumbnail/` URL themselves
  for a server version below 14.0, which is what nanofile reports.

## Client quirks preserved

These behaviours are deliberate, because an official client depends on them:

- Activity feed avatar URLs are absolute, because the Android client loads that
  field directly with Glide. A missing author gets a placeholder, so the URL is
  still well formed.
- Repo events use `path="/"` and an empty `name`, matching seahub; nanofile's own
  repo-rename events have no seahub equivalent and keep the new repo name.
- Server-side merges with no conflict use the upstream description
  `Auto merge by system`.

## Tests

The compatibility surface is covered by `commit_test.rs`, `fs_test.rs`,
`fs_id_compat.rs`, `dir_order_test.rs`, `sync_test.rs`, `sync_merge_test.rs`,
`client_compat_test.rs`, `api_v21_test.rs`, `webdav_test.rs`, `auth_test.rs` and
`capability_coverage_test.rs` under `server/tests/`.
See [Testing](/dev/testing) for how the suite runs.
