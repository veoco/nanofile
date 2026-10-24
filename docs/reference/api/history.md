# History, revisions and trash

A file's version history, downloading or restoring one revision, and the two
trash APIs (seahub's cursor-based `/trash/` and the paged `/trash2/`).

Source: `server/src/handler/history.rs`, `server/src/handler/trash.rs`,
`server/src/service/repo/history.rs`, `server/src/fs/core/trash.rs`;
tests: `server/tests/api_v21_test.rs`, `server/tests/repo_trash_test.rs`,
`server/tests/trash_restore_test.rs`.

## Changes in a commit {#get-api2-repo-history-changes-repo-id}

`GET /api2/repo_history_changes/{repo_id}/?commit_id=<id>`

The file changes one commit introduced, grouped the way seahub's library history
view expects.

**Response `200`**

```json
{
  "new_files": [{ "path": "/notes.txt", "size": 1204 }],
  "deleted_files": [],
  "modified_files": [],
  "renamed_files": [],
  "new_dirs": [{ "path": "/photos" }],
  "deleted_dirs": []
}
```

Requires `history.read`.

**Seafile** — this is seahub's internal history endpoint (`ajax/repo/...`), not
the official reference's `GET /api/v2.1/repos/{repo_id}/history/`, which Nanofile
does not implement.

## File history {#get-api-v2-1-repos-repo-id-file-history}

`GET /api/v2.1/repos/{repo_id}/file/history/?p=<path>&limit=<n>`

Versions of one file, newest first. `limit` defaults to 25 and is clamped to
100; the response is always a single page.

**Response `200`**

```json
{
  "data": [
    {
      "commit_id": "…",
      "path": "/notes.txt",
      "size": 1204,
      "mtime": 1760000000,
      "last_modified_by": "user@example.com",
      "rev_file_id": "…",
      "rev_file_name": "notes.txt",
      "rev_desc": "Edited notes.txt",
      "file_name": "notes.txt",
      "file_size": 1204,
      "file_mtime": 1760000000,
      "file_type": "file"
    }
  ],
  "page": 1,
  "page_next": false
}
```

Commits that did not change the file's content are skipped. Requires
`history.read`.

## Download a revision {#get-api-v2-1-repos-repo-id-file-revision}

`GET /api/v2.1/repos/{repo_id}/file/revision/?p=<path>&commit_id=<id>`

Streams the file's bytes as they were in that commit, with
`Content-Disposition: attachment`. Requires `history.read`.

**Seafile** — the official reference documents this operation under the legacy
`/api2/repos/{repo_id}/file/revision/`; Nanofile serves it under `/api/v2.1/`
(the path the web interface uses). The bytes and query parameters are the same.

## Restore a revision {#post-api-v2-1-repos-repo-id-file-revision-restore}

`POST /api/v2.1/repos/{repo_id}/file/revision/restore/?p=<path>&commit_id=<id>`

Points the file back at the revision's content and commits the change. History
is not rewritten.

**Response `200`** — `{"success": true}`. Requires `history.write`.

## List file trash {#get-api-v2-1-repos-repo-id-trash}

`GET /api/v2.1/repos/{repo_id}/trash/?cursor=<ts>&limit=<n>`

Cursor-paginated list of deleted entries, newest first. `limit` defaults to 50
and is clamped to 100.

**Response `200`**

```json
{
  "items": [
    {
      "parent_dir": "/",
      "obj_name": "old.txt",
      "deleted_time": "2026-10-24T09:00:00+00:00",
      "deleted_time_ts": 1760000000,
      "commit_id": "…",
      "is_dir": false,
      "size": 1204,
      "obj_id": "…",
      "repo_id": "…",
      "repo_name": ""
    }
  ],
  "has_more": false
}
```

Pass the last item's `deleted_time_ts` as `cursor` for the next page. Requires
`trash.read`.

## Clean file trash {#delete-api-v2-1-repos-repo-id-trash}

`DELETE /api/v2.1/repos/{repo_id}/trash/`

Empties the library's trash. An optional JSON body `{"keep_days": <n>}` keeps
entries deleted within the last `n` days.

**Response `200`** — `{"success": true}`. Requires `trash.write`.

## Revert deleted entries {#post-api-v2-1-repos-repo-id-trash-revert-dirents}

`POST /api/v2.1/repos/{repo_id}/trash/revert-dirents/`

The legacy revert endpoint. The body is a URL-encoded form with `commit_id` and
an optional colon-separated `file_names` list (empty means every entry from that
commit).

**Response `200`**

```json
{ "success": [{ "path": "/old.txt", "is_dir": false }], "failed": [] }
```

Requires `trash.write`. Seafile posts repeated `path` fields; Nanofile reads one
colon-separated `file_names` field.

## List file trash (paged) {#get-api-v2-1-repos-repo-id-trash2}

`GET /api/v2.1/repos/{repo_id}/trash2/?page=<n>&per_page=<n>`

The paged shape the web interface uses. `page` defaults to 1 and `per_page` to
50 (clamped to 100).

**Response `200`**

```json
{ "items": [], "total_count": 0, "can_search": true }
```

Items have the same fields as the cursor listing. Requires `trash.read`.

## Search the trash {#get-api-v2-1-repos-repo-id-trash2-search}

`GET /api/v2.1/repos/{repo_id}/trash2/search/?q=<text>&page=&per_page=&op_users=&time_from=&time_to=&suffixes=`

Filters deleted entries. `q` matches the name and path; `op_users` is a
comma-separated list of deleting users; `time_from`/`time_to` are Unix seconds;
`suffixes` filters by extension.

**Response `200`** — the same `{items, total_count, can_search}` envelope.
Requires `trash.read`.

## Restore from the trash {#post-api-v2-1-repos-repo-id-trash2-revert}

`POST /api/v2.1/repos/{repo_id}/trash2/revert/`

Restores the named entries. The body maps a commit id to full paths:

```json
{ "<commit_id>": ["/old.txt", "/photos"] }
```

**Response `200`** — `{"success": [{"path": "/old.txt", "is_dir": false}], "failed": []}`.
A failure carries `{"commit_id", "path", "error_msg"}`. Requires `trash.write`.

## List deleted libraries {#get-api-v2-1-deleted-repos}

`GET /api/v2.1/deleted-repos/`

The libraries in the caller's trash.

**Response `200`**

```json
[
  {
    "repo_id": "…",
    "repo_name": "Old library",
    "owner_email": "user@example.com",
    "owner_name": "User",
    "owner_contact_email": "user@example.com",
    "head_commit_id": "…",
    "size": 1204,
    "del_time": "2026-10-24T09:00:00+00:00",
    "org_id": -1,
    "encrypted": false
  }
]
```

Requires `library.read`.

## Restore a deleted library {#post-api-v2-1-deleted-repos}

`POST /api/v2.1/deleted-repos/`

Restores a library from the trash. The body is a URL-encoded form (`repo_id=…`)
or JSON `{"repo_id": "…"}`.

**Response `200`** — `{"success": true}`. Requires `library.delete`.

## Empty the library trash {#delete-api-v2-1-deleted-repos}

`DELETE /api/v2.1/deleted-repos/`

Permanently deletes every library in the caller's trash and reclaims its blocks.

**Response `200`** — `{"success": true}`. Requires `library.delete`.

## Purge one deleted library {#delete-api-v2-1-deleted-repos-repo-id}

`DELETE /api/v2.1/deleted-repos/{repo_id}/`

Permanently deletes one library from the trash.

**Response `200`** — `{"success": true}`. Requires `library.delete`.

**Seafile** — the official reference documents `GET /api/v2.1/repos/{repo_id}/trash/`
with seahub's path/`scan_stat` shape. Nanofile's version is cursor-paginated and
returns a different item set (no `scan_stat`, but `deleted_time_ts`, `repo_id`
and `repo_name`). The paged `/trash2/` API is a seahub endpoint the reference
omits. Per-library history (`/api/v2.1/repos/{repo_id}/history/`) and the commit
snapshot endpoints are not implemented.
