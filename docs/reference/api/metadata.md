# Metadata and search

Seafile's metadata service (as Nanofile implements it), the tag endpoints, and
the two search APIs plus the indexing controls. Metadata is a Seafile Pro
feature upstream; here it is always available and stores only tags and
per-file key/value records.

Source: `server/src/handler/metadata.rs`, `server/src/handler/search.rs`,
`server/src/handler/reindex.rs`, `server/src/service/fs/metadata.rs`,
`server/src/service/fs/search.rs`; tests: `server/tests/metadata_test.rs`,
`server/tests/index_test.rs`, `server/tests/api_v21_test.rs`.

## Get metadata config {#get-api-v2-1-repos-repo-id-metadata}

`GET /api/v2.1/repos/{repo_id}/metadata/`

**Response `200`**

```json
{
  "enabled": true,
  "tags_enabled": true,
  "details_settings": "{}",
  "global_hidden_columns": [],
  "face_recognition_enabled": false,
  "summary_enabled": false,
  "show_view": true
}
```

Face recognition, summaries and views are reported off and cannot be enabled.
Requires `metadata.read`.

## Enable or disable metadata {#put-api-v2-1-repos-repo-id-metadata}

`PUT /api/v2.1/repos/{repo_id}/metadata/`

**Body** (JSON) — `{"enabled": true}` (defaults to `true` when absent).

**Response `200`** — `{"success": true}`. Requires `metadata.write`.

The official reference also has `DELETE` to disable metadata; Nanofile uses the
same `PUT` with `enabled: false`.

## List tags {#get-api-v2-1-repos-repo-id-metadata-tags}

`GET /api/v2.1/repos/{repo_id}/metadata/tags/?start=<n>&limit=<n>`

Library-wide tags. `limit` defaults to 1000 and is capped at 1000; `start` is an
offset capped at 10,000,000. `limit=0` returns no rows.

**Response `200`**

```json
{
  "results": [
    {
      "_id": "1",
      "_tag_name": "Important",
      "_tag_color": "#ff0000",
      "_tag_file_links": [],
      "_creator": "",
      "_last_modifier": "",
      "_ctime": "2026-10-24T09:00:00+00:00",
      "_mtime": "2026-10-24T09:00:00+00:00"
    }
  ],
  "metadata": []
}
```

Requires `tag.read`.

## Create tags {#post-api-v2-1-repos-repo-id-metadata-tags}

`POST /api/v2.1/repos/{repo_id}/metadata/tags/`

**Body** (JSON) — `{"tags_data": [{"_tag_name": "Important", "_tag_color": "#ff0000"}]}`.
Names are de-duplicated; an empty name is skipped; colours are validated.

**Response `200`** — `{"tags": [<tag>]}` with the created tags. Requires
`tag.write`.

## Update tags {#put-api-v2-1-repos-repo-id-metadata-tags}

`PUT /api/v2.1/repos/{repo_id}/metadata/tags/`

**Body** (JSON) — `{"tags_data": [{"tag_id": "1", "tag": {"_tag_name": "…", "_tag_color": "#00ff00"}}]}`.

**Response `200`** — `{"success": true}`. Requires `tag.write`.

## Delete tags {#delete-api-v2-1-repos-repo-id-metadata-tags}

`DELETE /api/v2.1/repos/{repo_id}/metadata/tags/`

**Body** (JSON) — `{"tag_ids": ["1", "2"]}`.

**Response `200`** — `{"success": true}`. Requires `tag.write`.

## Get the tags feature status {#get-api-v2-1-repos-repo-id-metadata-tags-status}

`GET /api/v2.1/repos/{repo_id}/metadata/tags-status/`

**Response `200`** — `{"enabled": true, "lang": "en"}`. Requires `tag.read`.

This read is a Nanofile addition; the official reference only has the write
operations.

## Enable tags {#put-api-v2-1-repos-repo-id-metadata-tags-status}

`PUT /api/v2.1/repos/{repo_id}/metadata/tags-status/`

Turns the tag feature on. **Response `200`** — `{"success": true}`. Requires
`tag.write`.

## Disable tags {#delete-api-v2-1-repos-repo-id-metadata-tags-status}

`DELETE /api/v2.1/repos/{repo_id}/metadata/tags-status/`

Turns the tag feature off. **Response `200`** — `{"success": true}`. Requires
`tag.write`.

## List files carrying a tag {#get-api-v2-1-repos-repo-id-metadata-tag-files-tag-id}

`GET /api/v2.1/repos/{repo_id}/metadata/tag-files/{tag_id}/?start=<n>&limit=<n>`

Files and folders tagged with `tag_id`, in the metadata record shape.

**Response `200`**

```json
{
  "results": [
    {
      "_id": "…",
      "_name": "notes.txt",
      "_parent_dir": "/",
      "_size": 1204,
      "_file_mtime": "2026-10-24T09:00:00+00:00",
      "_file_modifier": "user@example.com",
      "_tags": [{ "row_id": "1", "display_value": "Important" }]
    }
  ],
  "metadata": []
}
```

The offset and limit follow the tag list's caps. Requires `tag.read`.

## Set a file's tags {#put-api-v2-1-repos-repo-id-metadata-file-tags}

`PUT /api/v2.1/repos/{repo_id}/metadata/file-tags/`

**Body** (JSON) — `{"file_tags_data": [{"record_id": "…", "tags": ["1", "2"]}]}`.
The record id is the one returned by the record endpoint.

**Response `200`** — `{"success": true}`. Requires `tag.write`.

## Get a file's metadata record {#get-api-v2-1-repos-repo-id-metadata-record}

`GET /api/v2.1/repos/{repo_id}/metadata/record/?parent_dir=<dir>&file_name=<name>`

Resolves a file's metadata record, which is how the mobile clients find the
record id they need for tags.

**Response `200`**

```json
{
  "_id": "…",
  "_name": "notes.txt",
  "_parent_dir": "/",
  "_size": 1204,
  "_file_mtime": "2026-10-24T09:00:00+00:00",
  "_file_modifier": "user@example.com",
  "_tags": [{ "row_id": "1", "display_value": "Important" }]
}
```

Stored custom fields are merged into the object. `file_name` may be sent as
`name`. Requires `metadata.read`.

## Update a file's metadata record {#put-api-v2-1-repos-repo-id-metadata-record}

`PUT /api/v2.1/repos/{repo_id}/metadata/record/`

**Body** (JSON) — `{"record_id": "…", "data": {"field": "value"}}`. The `data`
object replaces the stored custom fields.

**Response `200`** — `{"success": true}`. Requires `metadata.write`.

## Related users {#get-api-v2-1-repos-repo-id-related-users}

`GET /api/v2.1/repos/{repo_id}/related-users/`

The library's members, for the "modified by" filter.

**Response `200`**

```json
{
  "user_list": [
    { "email": "user@example.com", "name": "User", "contact_email": "user@example.com", "avatar_url": "https://files.example.com/avatars/user/user@example.com/resized/72/" }
  ]
}
```

Requires `library.read`.

## Search files in libraries {#get-api2-search}

`GET /api2/search/?q=<text>&per_page=<n>&page=<n>&search_repo=<id|all>&search_filename_only=<bool>`

The legacy, library-spanning search the desktop client uses.

| Query | Meaning |
| --- | --- |
| `q` | Search text. Matches file names, and indexed content unless `search_filename_only` is true. |
| `per_page` | Hits per page, default 10, maximum 100. |
| `page` | Page number, default 1. |
| `search_repo` | A library id, or a seahub scope keyword (`all`, `mine`, `shared`); empty searches everything visible. |
| `search_filename_only` | Skip the full-text index. |

**Response `200`**

```json
{
  "results": [
    {
      "repo_id": "…",
      "repo_name": "Docs",
      "name": "notes.txt",
      "oid": "…",
      "last_modified": 1760000000,
      "mtime": 1760000000,
      "fullpath": "/notes.txt",
      "size": 1204,
      "is_dir": false,
      "dir_url": "/libraries/<repo_id>/files/",
      "content_highlight": ""
    }
  ],
  "total": 1,
  "has_more": false
}
```

Both `last_modified` and `mtime` are emitted because the desktop client reads the
former and iOS the latter. Requires `search.read`. Rate limited per user per
minute (`auth.search_max_per_minute`).

**Seafile** — same route and envelope. The Pro-only filters (`search_type`,
`obj_type`, `time_from`, `size_from`, `search_path`, `with_permission`) are
ignored, and the endpoint works with the full-text index disabled (filename
matches only).

## Search files by name in a library {#get-api-v2-1-search-file}

`GET /api/v2.1/search-file/?repo_id=<id>&q=<text>`

`GET /api/v2.1/search-file`

Filename-only search inside one library, in the shape the mobile clients parse.

**Response `200`**

```json
{ "data": [{ "path": "/notes.txt", "size": 1204, "mtime": "2026-10-24T09:00:00+00:00", "type": "file" }] }
```

Folders come first, then files, each by modification time descending. `type` is
`file` or `folder`. Requires `search.read` and the library must be inside the
credential's scope.

The path is registered with and without a trailing slash, because iOS sends the
former and Android the latter.

**Seafile** — same route and shape, but Nanofile caps the result at the shared
200-hit budget; upstream walks the whole tree.

## Update a file's indexed text {#post-api2-index-file-text}

`POST /api2/index-file-text/`

Sets the full-text index content for one file, bypassing extraction. For
conversion services.

**Body** (JSON) — `{"repo_id": "…", "path": "/a.pdf", "text": "…"}`.

**Response `200`** — `{"status": "ok"}`. Library owner or administrator only;
requires `search.write` and full-text indexing enabled.

## Read a file's indexed text {#get-api2-repos-repo-id-file-index-text}

`GET /api2/repos/{repo_id}/file/index-text/?p=<path>`

**Response `200`** — `{"content": "…"}`, or `null` for `content` when the file is
not indexed. Requires `file.read` and full-text indexing enabled.

## Reindex a file {#post-api2-repos-repo-id-file-reindex}

`POST /api2/repos/{repo_id}/file/reindex/`

Reindexes one file synchronously.

**Body** (JSON) — `{"p": "/notes.txt"}`.

**Response `200`** — `{"status": "ok", "indexed": true}`. Library owner or
administrator only; requires `search.reindex`.

## Rebuild a library's index {#post-api2-reindex}

`POST /api2/reindex/`

Starts a background full-text rebuild.

**Body** (JSON) — `{"repo_id": "…"}`.

**Response `200`** — `{"status": "ok", "task_id": "…"}`. Owner or administrator
only; requires `search.reindex`. One rebuild per library at a time; the request
is rate limited per user per hour (`auth.reindex_max_per_hour`).

## Reindex progress {#get-api2-reindex-progress}

`GET /api2/reindex-progress/?task_id=<id>`

**Response `200`**

```json
{
  "state": "running",
  "repo_id": "…",
  "done_count": 4,
  "total": 10,
  "indexed": 0,
  "skipped": 0,
  "error": null,
  "finished_at": null,
  "creator_id": 7
}
```

`state` is `running`, `completed` or `failed`. Only the task's creator or an
administrator can read it; others get `404`. Requires `search.reindex`.

**Seafile** — metadata config, tags, tag files, file tags, records and
related-users are the documented upstream routes. The metadata *views*, columns,
tag links, tag merge, OCR and face-recognition endpoints are not implemented. The
search endpoints match the reference; the indexing controls are seahub
endpoints the reference does not document.
