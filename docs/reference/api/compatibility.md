# Seafile API compatibility

This page compares every operation in Seafile's official API reference with what Nanofile serves. It answers two questions: which official endpoints exist here, and which endpoints exist here that the official reference never mentions.

The baseline is the upstream reference **v13.0** (the version the site serves by default), fetched on **2026-10-24**: 304 operations across five sections.

## Legend {#legend}

- **Same** — the same method and path template (parameter names may differ). The response is documented on the linked endpoint page; Nanofile-specific additions are allowed.
- **Differs** — the job can be done, but the route, the request/response shape or a documented parameter differs. The note says how.
- **Missing** — no equivalent; the note gives the closest alternative when there is one.
- **N/A** — the operation belongs to a parallel surface this server does not expose at all.

A `Same` row is a route contract, not a byte-for-byte promise: check the linked page for the exact response.

## Summary {#summary}

| Section | Official operations | Same | Differs | Missing | N/A |
| --- | --- | --- | --- | --- | --- |
| authentication | 2 | 1 | 1 | 0 | 0 |
| Via Repo-Token | 40 | 0 | 3 | 0 | 37 |
| Account Operations - User | 165 | 25 | 40 | 100 | 0 |
| Account Operations - System Admin | 57 | 0 | 4 | 53 | 0 |
| Metadata Operations - User | 40 | 8 | 5 | 27 | 0 |
| **Total** | **304** | **34** | **53** | **180** | **37** |

## authentication

### Account-Token

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Obtain account token](https://seafile-api.readme.io/reference/post_api2-auth-token)<br>`POST /api2/auth-token/` | Same | [our endpoint](/reference/api/authentication#post-api2-auth-token) |  |

### Repo-Token

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Generate repo api token](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-repo-api-tokens)<br>`POST /api/v2.1/repos/{repo_id}/repo-api-tokens/` | Differs | [our endpoint](/reference/api/libraries#get-api2-repo-tokens) | The legacy `/api2/repo-tokens/` mints repository sync tokens; there is no HTTP repo-token API. Use `/api2/api-keys/` for scoped access. |

## Via Repo-Token

The `/api/v2.1/via-repo-token/` surface is a mirror of the normal API for Repo-Token holders. Nanofile does not route that prefix: a library-scoped API key authenticates the normal `/api2/` and `/api/v2.1/` routes directly, so the mirror operations are marked `N/A` here; the three token-management operations in this section are judged against Nanofile's API keys.

### API tokens

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List repo api tokens](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-repo-api-tokens)<br>`GET /api/v2.1/repos/{repo_id}/repo-api-tokens/` | Differs | [our endpoint](/reference/api/api-keys#get-api2-api-keys) | Not implemented as a route. API keys are the equivalent scoped credential. |
| [Update repo api token](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-repo-api-tokens-app-name)<br>`PUT /api/v2.1/repos/{repo_id}/repo-api-tokens/{app_name}/` | Differs | [our endpoint](/reference/api/api-keys#put-api2-api-keys-id) | Not implemented. An API key can be renamed/re-scoped through `/api2/api-keys/{id}/`. |
| [Delete repo api token](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-repo-api-tokens-app-name)<br>`DELETE /api/v2.1/repos/{repo_id}/repo-api-tokens/{app_name}/` | Differs | [our endpoint](/reference/api/api-keys#delete-api2-api-keys-id) | Not implemented. Revoke the equivalent API key instead. |

### Dir / Files

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List items in directory](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-dir)<br>`GET /api/v2.1/via-repo-token/dir/` | N/A | — |  |
| [Operation directory](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-dir)<br>`POST /api/v2.1/via-repo-token/dir/` | N/A | — |  |
| [Delete dir](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-dir)<br>`DELETE /api/v2.1/via-repo-token/dir/` | N/A | — |  |
| [Get file info](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-file)<br>`GET /api/v2.1/via-repo-token/file/` | N/A | — |  |
| [Operation file](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-file)<br>`POST /api/v2.1/via-repo-token/file/` | N/A | — |  |
| [Lock/Unlock file](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-file)<br>`PUT /api/v2.1/via-repo-token/file/` | N/A | — |  |
| [Delete file](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-file)<br>`DELETE /api/v2.1/via-repo-token/file/` | N/A | — |  |
| [Move directory with items merged](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-move-dir)<br>`POST /api/v2.1/via-repo-token/move-dir/` | N/A | — |  |
| [Get upload link](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-upload-link)<br>`GET /api/v2.1/via-repo-token/upload-link/` | N/A | — |  |
| [Get download link](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-download-link)<br>`GET /api/v2.1/via-repo-token/download-link/` | N/A | — |  |
| [Get repo info](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-repo-info)<br>`GET /api/v2.1/via-repo-token/repo-info/` | N/A | — |  |
| [Sync batch move item](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-sync-batch-move-item)<br>`POST /api/v2.1/via-repo-token/sync-batch-move-item/` | N/A | — |  |
| [Sync batch copy item](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-sync-batch-copy-item)<br>`POST /api/v2.1/via-repo-token/sync-batch-copy-item/` | N/A | — |  |
| [Batch delete items](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-batch-delete-item)<br>`DELETE /api/v2.1/via-repo-token/batch-delete-item/` | N/A | — |  |

### Share links

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Create share link](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-share-links)<br>`POST /api/v2.1/via-repo-token/share-links/` | N/A | — |  |

### Metadata records

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List metadata records](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-records)<br>`GET /api/v2.1/via-repo-token/metadata/records/` | N/A | — |  |
| [Update metadata records](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-records)<br>`PUT /api/v2.1/via-repo-token/metadata/records/` | N/A | — |  |

### Metadata views

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List views](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-views)<br>`GET /api/v2.1/via-repo-token/metadata/views/` | N/A | — |  |
| [Add view](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-views)<br>`POST /api/v2.1/via-repo-token/metadata/views/` | N/A | — |  |
| [Update view](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-views)<br>`PUT /api/v2.1/via-repo-token/metadata/views/` | N/A | — |  |
| [Delete view](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-views)<br>`DELETE /api/v2.1/via-repo-token/metadata/views/` | N/A | — |  |
| [Get a view](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-views-view-id)<br>`GET /api/v2.1/via-repo-token/metadata/views/{view_id}/` | N/A | — |  |
| [Move view](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-move-views)<br>`POST /api/v2.1/via-repo-token/metadata/move-views/` | N/A | — |  |
| [Duplicate view](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-duplicate-view)<br>`POST /api/v2.1/via-repo-token/metadata/duplicate-view/` | N/A | — |  |

### Tags

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Update tags links](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-tags-links)<br>`PUT /api/v2.1/via-repo-token/metadata/tags-links/` | N/A | — |  |
| [Delete tags links](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-tags-links)<br>`DELETE /api/v2.1/via-repo-token/metadata/tags-links/` | N/A | — |  |
| [Turn on tags feature](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-tags-status)<br>`PUT /api/v2.1/via-repo-token/metadata/tags-status/` | N/A | — |  |
| [Turn off tags feature](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-tags-status)<br>`DELETE /api/v2.1/via-repo-token/metadata/tags-status/` | N/A | — |  |
| [List tags](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-tags)<br>`GET /api/v2.1/via-repo-token/metadata/tags/` | N/A | — |  |
| [Add tags](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-tags)<br>`POST /api/v2.1/via-repo-token/metadata/tags/` | N/A | — |  |
| [Update tags](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-tags)<br>`PUT /api/v2.1/via-repo-token/metadata/tags/` | N/A | — |  |
| [Delete tags](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-tags)<br>`DELETE /api/v2.1/via-repo-token/metadata/tags/` | N/A | — |  |
| [Update file tags](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-file-tags)<br>`PUT /api/v2.1/via-repo-token/metadata/file-tags` | N/A | — |  |
| [List tag files](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-tag-files-tag-id)<br>`GET /api/v2.1/via-repo-token/metadata/tag-files/{tag_id}/` | N/A | — |  |
| [List tags files](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-tags-files)<br>`POST /api/v2.1/via-repo-token/metadata/tags-files/` | N/A | — |  |
| [Add tags links](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-tags-links)<br>`POST /api/v2.1/via-repo-token/metadata/tags-links/` | N/A | — |  |
| [Merge tags](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-merge-tags)<br>`POST /api/v2.1/via-repo-token/metadata/merge-tags/` | N/A | — |  |

## Account Operations - User

### Account

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get account info](https://seafile-api.readme.io/reference/get_api2-account-info)<br>`GET /api2/account/info/` | Differs | [our endpoint](/reference/api/accounts#get-api2-account-info) | Response omits seahub's `login_id`, `is_staff`, `email_notification_interval`, `institution` and `department`. |
| [Get client token](https://seafile-api.readme.io/reference/post_api2-client-login)<br>`POST /api2/client-login/` | Same | [our endpoint](/reference/api/authentication#post-api2-client-login) |  |

### Activities

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get file activities](https://seafile-api.readme.io/reference/get_api-v2-1-activities)<br>`GET /api/v2.1/activities/` | Same | [our endpoint](/reference/api/sharing#get-api-v2-1-activities) |  |

### Avatars

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Upload/Update user avatar](https://seafile-api.readme.io/reference/post_api-v2-1-user-avatar)<br>`POST /api/v2.1/user-avatar/` | Same | [our endpoint](/reference/api/accounts#post-api-v2-1-user-avatar) |  |
| [Get user avatar](https://seafile-api.readme.io/reference/get_api2-avatars-user-user-resized-size)<br>`GET /api2/avatars/user/{user}/resized/{size}/` | Same | [our endpoint](/reference/api/accounts#get-api2-avatars-user-email-resized-size) |  |

### Department libraries

No groups or group-owned libraries.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Add group owned library](https://seafile-api.readme.io/reference/post_api-v2-1-groups-group-id-group-owned-libraries)<br>`POST /api/v2.1/groups/{group_id}/group-owned-libraries/` | Missing | — |  |
| [Rename a group owned library](https://seafile-api.readme.io/reference/put_api-v2-1-groups-group-id-group-owned-libraries-repo-id)<br>`PUT /api/v2.1/groups/{group_id}/group-owned-libraries/{repo_id}/` | Missing | — |  |
| [Delete group owned library](https://seafile-api.readme.io/reference/delete_api-v2-1-groups-group-id-libraries-repo-id)<br>`DELETE /api/v2.1/groups/{group_id}/libraries/{repo_id}/` | Missing | — |  |
| [Get group owned library user share info](https://seafile-api.readme.io/reference/get_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`GET /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | Missing | — |  |
| [Share group owned library to user](https://seafile-api.readme.io/reference/post_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`POST /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | Missing | — |  |
| [Modify group owned library user share permission](https://seafile-api.readme.io/reference/put_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`PUT /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | Missing | — |  |
| [Delete group owned library user share](https://seafile-api.readme.io/reference/delete_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`DELETE /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | Missing | — |  |
| [Get group owned library group share info](https://seafile-api.readme.io/reference/get_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`GET /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | Missing | — |  |
| [Share group owned library to group](https://seafile-api.readme.io/reference/post_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`POST /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | Missing | — |  |
| [Modify group owned library group share permission](https://seafile-api.readme.io/reference/put_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`PUT /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | Missing | — |  |
| [Delete group owned library group share](https://seafile-api.readme.io/reference/delete_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`DELETE /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | Missing | — |  |

### Devices

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List devices](https://seafile-api.readme.io/reference/get_api2-devices)<br>`GET /api2/devices/` | Same | [our endpoint](/reference/api/api-keys#get-api2-devices) |  |
| [Unlink device](https://seafile-api.readme.io/reference/delete_api2-devices)<br>`DELETE /api2/devices/` | Differs | [our endpoint](/reference/api/api-keys#delete-api2-devices) | Parses a URL-encoded form (not JSON) and ignores `remote_wipe`; also accepts `kind`+`id` addressing. |

### Directories

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List items in directory](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-dir)<br>`GET /api2/repos/{repo_id}/dir/` | Differs | [our endpoint](/reference/api/directories#get-api2-repos-repo-id-dir) | No `?oid=` revalidation; `_Internal` is hidden; entries use display order and every entry carries `modifier_email`. |
| [Create new or rename directory](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-dir)<br>`POST /api2/repos/{repo_id}/dir/` | Same | [our endpoint](/reference/api/directories#post-api2-repos-repo-id-dir) |  |
| [Delete directory](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-dir)<br>`DELETE /api2/repos/{repo_id}/dir/` | Same | [our endpoint](/reference/api/directories#delete-api2-repos-repo-id-dir) |  |
| [Get directory detail](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-dir-detail)<br>`GET /api/v2.1/repos/{repo_id}/dir/detail/` | Differs | [our endpoint](/reference/api/directories#get-api-v2-1-repos-repo-id-dir-detail) | `mtime` is a JSON number; seahub returns an ISO 8601 string. |
| [Revert directory to a history status](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-dir-revert)<br>`PUT /api2/repos/{repo_id}/dir/revert/` | Missing | — | Use the trash or a file revision restore instead. |
| [Get task directory token](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-zip-task)<br>`GET /api/v2.1/repos/{repo_id}/zip-task/` | Differs | [our endpoint](/reference/api/transfers#post-api-v2-1-repos-repo-id-zip-task) | Nanofile mints the archive token with `POST` (the web interface's shape). |
| [Query task progress](https://seafile-api.readme.io/reference/get_api-v2-1-query-zip-progress)<br>`GET /api/v2.1/query-zip-progress/` | Missing | — | `zip-task` is synchronous; poll the task list in the web interface instead. |
| [Move directory with items merged](https://seafile-api.readme.io/reference/post_api-v2-1-move-folder-merge)<br>`POST /api/v2.1/move-folder-merge/` | Missing | — | Use `dir/move/`; a directory is moved as a whole, without merging. |

### File comments

Nanofile has no comments.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List file comments](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-comments)<br>`GET /api2/repos/{repo_id}/file/comments/` | Missing | — | No comments; `file/detail/` always reports `comment_total: 0`. |
| [Submit a file comment](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-file-comments)<br>`POST /api2/repos/{repo_id}/file/comments/` | Missing | — |  |
| [Get a file comment](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-comments-pk)<br>`GET /api2/repos/{repo_id}/file/comments/{pk}/` | Missing | — |  |
| [Update a file comment](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-file-comments-pk)<br>`PUT /api2/repos/{repo_id}/file/comments/{pk}/` | Missing | — |  |
| [Delete a file comment](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-file-comments-pk)<br>`DELETE /api2/repos/{repo_id}/file/comments/{pk}/` | Missing | — |  |
| [Get number of comments](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-comments-counts)<br>`GET /api2/repos/{repo_id}/file/comments/counts/` | Missing | — |  |

### File history

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get file history](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-file-history)<br>`GET /api/v2.1/repos/{repo_id}/file/history/` | Differs | [our endpoint](/reference/api/history#get-api-v2-1-repos-repo-id-file-history) | Envelope is `{data, page, page_next}`; `next_start_commit` is absent and only one page is returned. |
| [Download file from a revision](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-revision)<br>`GET /api2/repos/{repo_id}/file/revision/` | Differs | [our endpoint](/reference/api/history#get-api-v2-1-repos-repo-id-file-revision) | Served under `/api/v2.1/repos/{repo_id}/file/revision/`. |

### Files

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Create/Rename/Move/Copy/Revert a file](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-file)<br>`POST /api/v2.1/repos/{repo_id}/file/` | Differs | [our endpoint](/reference/api/files#post-api-v2-1-repos-repo-id-file) | Only `create` and `rename`; a move uses the legacy `/api2/.../file/` form, a copy uses `fileops`, and a revert uses revision restore. |
| [Lock/Unlock file](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-file)<br>`PUT /api/v2.1/repos/{repo_id}/file/` | Differs | [our endpoint](/reference/api/files#put-api2-repos-repo-id-file) | Lock/unlock lives on the legacy `/api2/repos/{repo_id}/file/` PUT with `operation=lock\|unlock`. |
| [Delete file](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-file)<br>`DELETE /api/v2.1/repos/{repo_id}/file/` | Same | [our endpoint](/reference/api/files#delete-api-v2-1-repos-repo-id-file) |  |
| [Download file](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file)<br>`GET /api2/repos/{repo_id}/file/` | Same | [our endpoint](/reference/api/files#get-api2-repos-repo-id-file) |  |
| [Get file detail](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-detail)<br>`GET /api2/repos/{repo_id}/file/detail/` | Same | [our endpoint](/reference/api/files#get-api2-repos-repo-id-file-detail) |  |
| [Get smart link for a file](https://seafile-api.readme.io/reference/get_api-v2-1-smart-link)<br>`GET /api/v2.1/smart-link/` | Differs | [our endpoint](/reference/api/sharing#get-api-v2-1-smart-link) | Returns `{url}` pointing at the file endpoint; the official `smart_link`/`smart_link_token`/`name` shape is not produced. |

### File search

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Search files in libraries](https://seafile-api.readme.io/reference/get_api2-search)<br>`GET /api2/search/` | Differs | [our endpoint](/reference/api/metadata#get-api2-search) | Pro-only filters are ignored and the endpoint works without a full-text index (filename matches only). |
| [Search files by name in library](https://seafile-api.readme.io/reference/get_api-v2-1-search-file)<br>`GET /api/v2.1/search-file/` | Differs | [our endpoint](/reference/api/metadata#get-api-v2-1-search-file) | Results are capped at 200 hits; upstream walks the whole tree. |

### File tags

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List all tags of a library](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-repo-tags)<br>`GET /api/v2.1/repos/{repo_id}/repo-tags/` | Differs | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tags) | Tags are served under the metadata API. |
| [Add a tag to a library](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-repo-tags)<br>`POST /api/v2.1/repos/{repo_id}/repo-tags/` | Differs | [our endpoint](/reference/api/metadata#post-api-v2-1-repos-repo-id-metadata-tags) | Send `tags_data` to the metadata tags endpoint. |
| [Update a tag of a library](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-repo-tags-repo-tag-id)<br>`PUT /api/v2.1/repos/{repo_id}/repo-tags/{repo_tag_id}/` | Differs | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-tags) | The id travels inside `tags_data`. |
| [Delete a tag of a library](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-repo-tags-repo-tag-id)<br>`DELETE /api/v2.1/repos/{repo_id}/repo-tags/{repo_tag_id}/` | Differs | [our endpoint](/reference/api/metadata#delete-api-v2-1-repos-repo-id-metadata-tags) | The id travels inside `tag_ids`. |
| [List all tags of a file](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-file-tags)<br>`GET /api/v2.1/repos/{repo_id}/file-tags/` | Differs | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-record) | A file's tags are in the record's `_tags`. |
| [Add a tag for a file](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-file-tags)<br>`POST /api/v2.1/repos/{repo_id}/file-tags/` | Differs | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-file-tags) | Set the whole tag list through the metadata endpoint. |
| [Delete a tag from a file](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-file-tags-file-tag-id)<br>`DELETE /api/v2.1/repos/{repo_id}/file-tags/{file_tag_id}/` | Differs | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-file-tags) | Set the remaining tag list through the metadata endpoint. |
| [List tagged files by tag id](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-tagged-files-repo-tag-id)<br>`GET /api/v2.1/repos/{repo_id}/tagged-files/{repo_tag_id}/` | Differs | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tag-files-tag-id) | Served under the metadata API. |

### File upload

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get upload link](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-upload-link)<br>`GET /api2/repos/{repo_id}/upload-link/` | Same | [our endpoint](/reference/api/libraries#get-api2-repos-repo-id-upload-link) |  |
| [Upload file](https://seafile-api.readme.io/reference/post_seafhttp-upload-api-upload-token-ret-json-1)<br>`POST /seafhttp/upload-api/{upload_token}?ret-json=1` | Differs | [our endpoint](/reference/api/transfers#post-upload-api-token) | Upload links point at `/upload-api/<token>`; the `/seafhttp/` prefix is not used. |
| [Get update link](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-update-link)<br>`GET /api2/repos/{repo_id}/update-link/` | Same | [our endpoint](/reference/api/libraries#get-api2-repos-repo-id-update-link) |  |
| [Update file](https://seafile-api.readme.io/reference/post_seafhttp-update-api-upload-token)<br>`POST /seafhttp/update-api/{upload_token}/` | Differs | [our endpoint](/reference/api/transfers#post-update-api-token) | Update links point at `/update-api/<token>`; the `/seafhttp/` prefix is not used. |

### Files/Directories batch operation

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Batch copy items synchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-sync-batch-copy-item)<br>`POST /api/v2.1/repos/sync-batch-copy-item/` | Same | [our endpoint](/reference/api/files#post-api-v2-1-repos-sync-batch-copy-item) |  |
| [Batch move items synchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-sync-batch-move-item)<br>`POST /api/v2.1/repos/sync-batch-move-item/` | Same | [our endpoint](/reference/api/files#post-api-v2-1-repos-sync-batch-move-item) |  |
| [Batch delete items](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-batch-delete-item)<br>`DELETE /api/v2.1/repos/batch-delete-item/` | Differs | [our endpoint](/reference/api/files#post-api-v2-1-repos-batch-delete-item) | Returns `{success}` without `commit_id`. |
| [Batch copy items asynchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-async-batch-copy-item)<br>`POST /api/v2.1/repos/async-batch-copy-item/` | Same | [our endpoint](/reference/api/files#post-api-v2-1-repos-async-batch-copy-item) |  |
| [Batch move items asynchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-async-batch-move-item)<br>`POST /api/v2.1/repos/async-batch-move-item/` | Same | [our endpoint](/reference/api/files#post-api-v2-1-repos-async-batch-move-item) |  |
| [Query async operation progress](https://seafile-api.readme.io/reference/get_api-v2-1-query-copy-move-progress)<br>`GET /api/v2.1/query-copy-move-progress/` | Differs | — | Progress is `state`/`done_count`/`total`/`cancelable`; `canceled` is absent and `done` is a boolean. |
| [Cancel async operation](https://seafile-api.readme.io/reference/delete_api-v2-1-copy-move-task)<br>`DELETE /api/v2.1/copy-move-task/` | Missing | — | Tasks cannot be cancelled: progress reports `cancelable: false`. |

### Groups

Only the empty list stubs; no group management.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List groups](https://seafile-api.readme.io/reference/get_api2-groups)<br>`GET /api2/groups/` | Differs | [our endpoint](/reference/api/sharing#get-api2-groups) | Returns a bare array; the official shape is `{replynum, groups}`. Always empty (no groups). |
| [Add a group](https://seafile-api.readme.io/reference/post_api-v2-1-groups)<br>`POST /api/v2.1/groups/` | Missing | — |  |
| [Get info of a group](https://seafile-api.readme.io/reference/get_api-v2-1-groups-group-id)<br>`GET /api/v2.1/groups/{group_id}/` | Missing | — |  |
| [Rename/Transfer a group](https://seafile-api.readme.io/reference/put_api-v2-1-groups-group-id)<br>`PUT /api/v2.1/groups/{group_id}/` | Missing | — |  |
| [Delete a group](https://seafile-api.readme.io/reference/delete_api-v2-1-groups-group-id)<br>`DELETE /api/v2.1/groups/{group_id}/` | Missing | — |  |
| [List all group members](https://seafile-api.readme.io/reference/get_api-v2-1-groups-group-id-members)<br>`GET /api/v2.1/groups/{group_id}/members/` | Missing | — |  |
| [Add a group member](https://seafile-api.readme.io/reference/post_api-v2-1-groups-group-id-members)<br>`POST /api/v2.1/groups/{group_id}/members/` | Missing | — |  |
| [Bulk add group members](https://seafile-api.readme.io/reference/post_api-v2-1-groups-group-id-members-bulk)<br>`POST /api/v2.1/groups/{group_id}/members/bulk/` | Missing | — |  |
| [Get info of a group member](https://seafile-api.readme.io/reference/get_api-v2-1-groups-group-id-members-user-id)<br>`GET /api/v2.1/groups/{group_id}/members/{user_id}/` | Missing | — |  |
| [Set/Unset a group admin](https://seafile-api.readme.io/reference/put_api-v2-1-groups-group-id-members-user-id)<br>`PUT /api/v2.1/groups/{group_id}/members/{user_id}/` | Missing | — |  |
| [Leave group or group owner/admin delete a group member](https://seafile-api.readme.io/reference/delete_api-v2-1-groups-group-id-members-user-id)<br>`DELETE /api/v2.1/groups/{group_id}/members/{user_id}/` | Missing | — |  |

### Invitations

Nanofile has no invitations; registration is a web flow or admin-only.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List invitations](https://seafile-api.readme.io/reference/get_api-v2-1-invitations)<br>`GET /api/v2.1/invitations/` | Missing | — |  |
| [Add invitation](https://seafile-api.readme.io/reference/post_api-v2-1-invitations)<br>`POST /api/v2.1/invitations/` | Missing | — |  |
| [Batch add invitations](https://seafile-api.readme.io/reference/post_api-v2-1-invitations-batch)<br>`POST /api/v2.1/invitations/batch/` | Missing | — |  |
| [Get invitation](https://seafile-api.readme.io/reference/get_api-v2-1-invitations-invitation-token)<br>`GET /api/v2.1/invitations/{invitation_token}/` | Missing | — |  |
| [Delete invitation](https://seafile-api.readme.io/reference/delete_api-v2-1-invitations-invitation-token)<br>`DELETE /api/v2.1/invitations/{invitation_token}/` | Missing | — |  |
| [Revoke invitation](https://seafile-api.readme.io/reference/delete_api-v2-1-invitations-invitation-token-revoke)<br>`DELETE /api/v2.1/invitations/{invitation_token}/revoke/` | Missing | — |  |

### Libraries

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get default library](https://seafile-api.readme.io/reference/get_api2-default-repo)<br>`GET /api2/default-repo/` | Same | [our endpoint](/reference/api/libraries#get-api2-default-repo) |  |
| [Create default library](https://seafile-api.readme.io/reference/post_api2-default-repo)<br>`POST /api2/default-repo/` | Differs | [our endpoint](/reference/api/libraries#post-api2-default-repo) | Returns only `{repo_id}`; `exists` is not echoed. |
| [List libraries/Search library by name](https://seafile-api.readme.io/reference/get_api2-repos)<br>`GET /api2/repos/` | Differs | [our endpoint](/reference/api/libraries#get-api2-repos) | No `?name=` search filter, and items lack `repo_size`/`mtime_relative`/relay fields. |
| [Create (Encrypted) library](https://seafile-api.readme.io/reference/post_api2-repos)<br>`POST /api2/repos/` | Differs | [our endpoint](/reference/api/libraries#post-api2-repos) | Response lacks `repo_size`, `repo_size_formatted`, `mtime_relative` and the relay fields. |
| [Get library info](https://seafile-api.readme.io/reference/get_api2-repos-repo-id)<br>`GET /api2/repos/{repo_id}/` | Differs | [our endpoint](/reference/api/libraries#get-api2-repos-repo-id) | Response has no `file_count` and no repo-level `modifier_*` fields. |
| [Rename/Decrypt library](https://seafile-api.readme.io/reference/post_api2-repos-repo-id)<br>`POST /api2/repos/{repo_id}/` | Differs | [our endpoint](/reference/api/libraries#post-api2-repos-repo-id) | Supports `rename`, `update`, `setpassword` and `checkpassword`; decrypt is not implemented. |
| [Delete library](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id)<br>`DELETE /api2/repos/{repo_id}/` | Same | [our endpoint](/reference/api/libraries#delete-api2-repos-repo-id) |  |
| [Get library owner](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-owner)<br>`GET /api2/repos/{repo_id}/owner/` | Missing | — | No ownership-transfer API. |
| [Transfer library](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-owner)<br>`PUT /api2/repos/{repo_id}/owner/` | Missing | — |  |
| [Get library history](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-history)<br>`GET /api/v2.1/repos/{repo_id}/history/` | Missing | — | Only the per-commit changes view exists (`/api2/repo_history_changes/`). |
| [Get library history limit days](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-history-limit)<br>`GET /api2/repos/{repo_id}/history-limit/` | Missing | — |  |
| [Set library history limit days](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-history-limit)<br>`PUT /api2/repos/{repo_id}/history-limit/` | Missing | — |  |
| [Get library trash](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-trash)<br>`GET /api/v2.1/repos/{repo_id}/trash/` | Differs | [our endpoint](/reference/api/history#get-api-v2-1-repos-repo-id-trash) | Cursor shape `{items, has_more}`, not seahub's `{data, more, scan_stat}`. |
| [Clean library trash](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-trash)<br>`DELETE /api/v2.1/repos/{repo_id}/trash/` | Same | [our endpoint](/reference/api/history#delete-api-v2-1-repos-repo-id-trash) |  |
| [Get library commit info](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-commits-commit-id)<br>`GET /api/v2.1/repos/{repo_id}/commits/{commit_id}/` | Missing | — |  |

### List share

No user-to-user shares exist.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List shared users/groups of a library](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-dir-shared-items)<br>`GET /api2/repos/{repo_id}/dir/shared_items/` | Differs | [our endpoint](/reference/api/directories#get-api2-repos-repo-id-dir-shared-items) | Lists download share links; user/group shares do not exist here. |
| [List libraries shared to me](https://seafile-api.readme.io/reference/get_api2-beshared-repos)<br>`GET /api2/beshared-repos/` | Missing | — |  |

### Share to user/group

Nanofile has no user-to-user or group shares; only link sharing.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Share a library to user/group](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-dir-shared-items)<br>`PUT /api2/repos/{repo_id}/dir/shared_items/` | Missing | — |  |
| [Unshare a library from user/group](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-dir-shared-items)<br>`DELETE /api2/repos/{repo_id}/dir/shared_items/` | Missing | — |  |
| [Update user share permission of a library](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-dir-shared-items)<br>`POST /api2/repos/{repo_id}/dir/shared_items/` | Missing | — |  |
| [Delete a library shared to me (leave the share)](https://seafile-api.readme.io/reference/delete_api2-beshared-repos-repo-id)<br>`DELETE /api2/beshared-repos/{repo_id}/` | Missing | — |  |
| [Batch share libraries to user/group](https://seafile-api.readme.io/reference/post_api-v2-1-repos-batch)<br>`POST /api/v2.1/repos/batch/` | Missing | — |  |
| [Share a folder](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-dir-shared-items-p-path)<br>`PUT /api2/repos/{repo_id}/dir/shared_items/p={path}` | Missing | — |  |
| [List shared folders](https://seafile-api.readme.io/reference/get_api-v2-1-shared-folders)<br>`GET /api/v2.1/shared-folders/` | Missing | — |  |
| [Update folder share permission](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-dir-shared-items-p-path)<br>`POST /api2/repos/{repo_id}/dir/shared_items/?p={path}` | Missing | — |  |
| [Remove the specified shared folder](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-dir-shared-items-p-path)<br>`DELETE /api2/repos/{repo_id}/dir/shared_items/?p={path}` | Missing | — |  |

### Server info

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get server information](https://seafile-api.readme.io/reference/get_api2-server-info)<br>`GET /api2/server-info/` | Same | [our endpoint](/reference/api/accounts#get-api2-server-info) |  |

### Share links

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List all share links](https://seafile-api.readme.io/reference/get_api-v2-1-share-links)<br>`GET /api/v2.1/share-links/` | Differs | [our endpoint](/reference/api/sharing#get-api-v2-1-share-links) | Items use the legacy fields (`created_at`, `expire_at`, `s_type`); `repo_name`/`username` are absent. |
| [Create share link](https://seafile-api.readme.io/reference/post_api-v2-1-share-links)<br>`POST /api/v2.1/share-links/` | Differs | [our endpoint](/reference/api/sharing#post-api-v2-1-share-links) | `permissions` is ignored; `repo_name`/`username` are null and permissions are fixed to download. |
| [Create multi share links](https://seafile-api.readme.io/reference/post_api-v2-1-multi-share-links)<br>`POST /api/v2.1/multi-share-links/` | Differs | [our endpoint](/reference/api/sharing#post-api-v2-1-multi-share-links) | Creates one link; `emails`/`user_scope` batch creation is not supported. |
| [Batch create shared links](https://seafile-api.readme.io/reference/post_api-v2-1-multi-share-links-batch)<br>`POST /api/v2.1/multi-share-links/batch/` | Missing | — |  |
| [List share links of a library](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-repo-id-repo-id)<br>`GET /api/v2.1/share-links/?repo_id={repo_id}/` | Differs | [our endpoint](/reference/api/sharing#get-api-v2-1-share-links) | The documented path is malformed; pass `?repo_id=` (and `?path=`) to the list endpoint. |
| [List share link of a folder (file)](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-repo-id-repo-id-path-path)<br>`GET /api/v2.1/share-links/?repo_id={repo_id}&path={path}` | Differs | [our endpoint](/reference/api/sharing#get-api-v2-1-share-links) | The documented path is malformed; pass both as query parameters. |
| [Delete share link](https://seafile-api.readme.io/reference/delete_api-v2-1-share-links-token)<br>`DELETE /api/v2.1/share-links/{token}/` | Same | [our endpoint](/reference/api/sharing#delete-api-v2-1-share-links-token) |  |
| [Send share link email](https://seafile-api.readme.io/reference/post_api2-send-share-link)<br>`POST /api2/send-share-link/` | Missing | — | No email sending for links; share the link yourself. |
| [List direntry in dir download link](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-token-dirents)<br>`GET /api/v2.1/share-links/{token}/dirents/` | Missing | — |  |
| [List user authorizations](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-link-token-user-auth)<br>`GET /api/v2.1/share-links/{link_token}/user-auth/` | Missing | — |  |
| [Add user authorization](https://seafile-api.readme.io/reference/post_api-v2-1-share-links-link-token-user-auth)<br>`POST /api/v2.1/share-links/{link_token}/user-auth/` | Missing | — |  |
| [Delete user authorization](https://seafile-api.readme.io/reference/delete_api-v2-1-share-links-link-token-user-auth)<br>`DELETE /api/v2.1/share-links/{link_token}/user-auth/` | Missing | — |  |
| [List email authorizations](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-link-token-email-auth)<br>`GET /api/v2.1/share-links/{link_token}/email-auth/` | Missing | — |  |
| [Add email authorization](https://seafile-api.readme.io/reference/post_api-v2-1-share-links-link-token-email-auth)<br>`POST /api/v2.1/share-links/{link_token}/email-auth/` | Missing | — |  |
| [Delete email authorization](https://seafile-api.readme.io/reference/delete_api-v2-1-share-links-link-token-email-auth)<br>`DELETE /api/v2.1/share-links/{link_token}/email-auth/` | Missing | — |  |

### Snapshot

No snapshot browsing or revert API.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List items in directory of a snapshot](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-commits-commit-id-dir)<br>`GET /api/v2.1/repos/{repo_id}/commits/{commit_id}/dir/` | Missing | — |  |
| [Revert library to a snapshot](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-commits-commit-id-revert)<br>`POST /api/v2.1/repos/{repo_id}/commits/{commit_id}/revert/` | Missing | — |  |

### Snapshot label

No snapshot labels.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get snapshot label](https://seafile-api.readme.io/reference/get_api-v2-1-revision-tags-tag-names)<br>`GET /api/v2.1/revision-tags/tag-names/` | Missing | — |  |
| [Create snapshot label](https://seafile-api.readme.io/reference/post_api-v2-1-revision-tags-tagged-items)<br>`POST /api/v2.1/revision-tags/tagged-items/` | Missing | — |  |
| [Update snapshot label](https://seafile-api.readme.io/reference/put_api-v2-1-revision-tags-tagged-items)<br>`PUT /api/v2.1/revision-tags/tagged-items/` | Missing | — |  |
| [Delete snapshot label](https://seafile-api.readme.io/reference/delete_api-v2-1-revision-tags-tagged-items)<br>`DELETE /api/v2.1/revision-tags/tagged-items/` | Missing | — |  |

### Starred items

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List starred items](https://seafile-api.readme.io/reference/get_api-v2-1-starred-items)<br>`GET /api/v2.1/starred-items/` | Same | [our endpoint](/reference/api/sharing#get-api-v2-1-starred-items) |  |
| [Star a library/folder/file](https://seafile-api.readme.io/reference/post_api-v2-1-starred-items)<br>`POST /api/v2.1/starred-items/` | Same | [our endpoint](/reference/api/sharing#post-api-v2-1-starred-items) |  |
| [Unstar a library/folder/file](https://seafile-api.readme.io/reference/delete_api-v2-1-starred-items)<br>`DELETE /api/v2.1/starred-items/` | Same | [our endpoint](/reference/api/sharing#delete-api-v2-1-starred-items) |  |

### Sub folder permissions

No per-folder permissions; every member has the library permission.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get user folder permission](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-user-folder-perm)<br>`GET /api2/repos/{repo_id}/user-folder-perm/` | Missing | — |  |
| [Set user folder permission](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-user-folder-perm)<br>`POST /api2/repos/{repo_id}/user-folder-perm/` | Missing | — |  |
| [Modify user folder permission](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-user-folder-perm)<br>`PUT /api2/repos/{repo_id}/user-folder-perm/` | Missing | — |  |
| [Delete user folder permission](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-user-folder-perm)<br>`DELETE /api2/repos/{repo_id}/user-folder-perm/` | Missing | — |  |
| [Get group folder permission](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-group-folder-perm)<br>`GET /api2/repos/{repo_id}/group-folder-perm/` | Missing | — |  |
| [Set group folder permission](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-group-folder-perm)<br>`POST /api2/repos/{repo_id}/group-folder-perm/` | Missing | — |  |
| [Modify group folder permission](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-group-folder-perm)<br>`PUT /api2/repos/{repo_id}/group-folder-perm/` | Missing | — |  |
| [Delete group folder permission](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-group-folder-perm)<br>`DELETE /api2/repos/{repo_id}/group-folder-perm/` | Missing | — |  |

### Upload links

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List upload links](https://seafile-api.readme.io/reference/get_api-v2-1-upload-links)<br>`GET /api/v2.1/upload-links/` | Differs | [our endpoint](/reference/api/sharing#get-api-v2-1-upload-links) | Items lack `repo_name`, `obj_name`, `ctime` and `is_expired`; `expire_at`/`has_password` are used instead. |
| [Create upload link](https://seafile-api.readme.io/reference/post_api-v2-1-upload-links)<br>`POST /api/v2.1/upload-links/` | Differs | [our endpoint](/reference/api/sharing#post-api-v2-1-upload-links) | `expire_date` and `description` are always null even when an expiry was applied. |
| [Delete upload links](https://seafile-api.readme.io/reference/delete_api-v2-1-upload-links-token)<br>`DELETE /api/v2.1/upload-links/{token}/` | Differs | [our endpoint](/reference/api/sharing#delete-api-v2-1-upload-links-token) | Returns bare `true` (the Android client's shape), not `{success}`. |
| [Send upload link email](https://seafile-api.readme.io/reference/post_api2-send-upload-link)<br>`POST /api2/send-upload-link/` | Missing | — | No email sending for links; share the link yourself. |
| [Get shared upload link](https://seafile-api.readme.io/reference/get_api-v2-1-upload-links-token-upload)<br>`GET /api/v2.1/upload-links/{token}/upload/` | Same | [our endpoint](/reference/api/sharing#get-api-v2-1-upload-links-token-upload) |  |

### User profile

Only `/api2/account/info/`; there is no v2.1 profile API.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get user profile](https://seafile-api.readme.io/reference/get_api-v2-1-user)<br>`GET /api/v2.1/user/` | Missing | — |  |
| [Update user profile](https://seafile-api.readme.io/reference/put_api-v2-1-user)<br>`PUT /api/v2.1/user/` | Missing | — |  |

### User search

No user directory search.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Search user](https://seafile-api.readme.io/reference/get_api2-search-user)<br>`GET /api2/search-user/` | Missing | — |  |

### Wiki

Nanofile has no wiki.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List wikis](https://seafile-api.readme.io/reference/get_api-v2-1-wikis2)<br>`GET /api/v2.1/wikis2/` | Missing | — |  |
| [Add wiki](https://seafile-api.readme.io/reference/post_api-v2-1-wikis2)<br>`POST /api/v2.1/wikis2/` | Missing | — |  |
| [Delete wiki](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/` | Missing | — |  |
| [Update wiki](https://seafile-api.readme.io/reference/put_api-v2-1-wiki2-wiki-id)<br>`PUT /api/v2.1/wiki2/{wiki_id}/` | Missing | — |  |
| [Get wiki config](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-config)<br>`GET /api/v2.1/wiki2/{wiki_id}/config/` | Missing | — |  |
| [Update wiki config](https://seafile-api.readme.io/reference/put_api-v2-1-wiki2-wiki-id-config)<br>`PUT /api/v2.1/wiki2/{wiki_id}/config/` | Missing | — |  |
| [Create wiki page](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-wiki-id-pages)<br>`POST /api/v2.1/wiki2/{wiki_id}/pages/` | Missing | — |  |
| [Delete wiki page](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id-page-page-id)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/page/{page_id}/` | Missing | — |  |
| [Get wiki page](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-page-page-id)<br>`GET /api/v2.1/wiki2/{wiki_id}/page/{page_id}/` | Missing | — |  |
| [Duplicate wiki page](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-8-duplicate-page)<br>`POST /api/v2.1/wiki2/8/duplicate-page/` | Missing | — |  |
| [Get publish wiki link](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-publish)<br>`GET /api/v2.1/wiki2/{wiki_id}/publish/` | Missing | — |  |
| [Publish wiki](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-wiki-id-publish)<br>`POST /api/v2.1/wiki2/{wiki_id}/publish/` | Missing | — |  |
| [Delete published wiki link](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id-publish)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/publish/` | Missing | — |  |
| [Get publish wiki config](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-publish-config)<br>`GET /api/v2.1/wiki2/{wiki_id}/publish/config/` | Missing | — |  |
| [Get publish wiki page](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-publish-page-page-id)<br>`GET /api/v2.1/wiki2/{wiki_id}/publish/page/{page_id}/` | Missing | — |  |
| [Search wiki](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-search)<br>`POST /api/v2.1/wiki2/search/` | Missing | — |  |
| [Convert wiki](https://seafile-api.readme.io/reference/post_api-v2-1-convert-wiki)<br>`POST /api/v2.1/convert-wiki/` | Missing | — |  |
| [Get wiki page trash](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-trash)<br>`GET /api/v2.1/wiki2/{wiki_id}/trash/` | Missing | — |  |
| [Revert page](https://seafile-api.readme.io/reference/put_api-v2-1-wiki2-wiki-id-trash)<br>`PUT /api/v2.1/wiki2/{wiki_id}/trash/` | Missing | — |  |
| [Clean wiki page trash](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id-trash)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/trash/` | Missing | — |  |

## Account Operations - System Admin

### Accounts

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List all users' info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users)<br>`GET /api/v2.1/admin/users/` | Differs | [our endpoint](/reference/api/accounts#get-api2-admin-users) | Nanofile's own administration API (`/api2/admin/users/`) returns a different shape. |
| [Add user](https://seafile-api.readme.io/reference/post_api-v2-1-admin-users)<br>`POST /api/v2.1/admin/users/` | Differs | [our endpoint](/reference/api/accounts#post-api2-admin-users) | Use `/api2/admin/users/` (JSON `email`/`password`/`is_admin`/`is_active`/`storage_quota`). |
| [Search user](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-user)<br>`GET /api/v2.1/admin/search-user/` | Missing | — |  |
| [Get a user's info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users-user-id)<br>`GET /api/v2.1/admin/users/{user_id}/` | Missing | — | No single-user GET; list `/api2/admin/users/` and filter. |
| [Update user info](https://seafile-api.readme.io/reference/put_api-v2-1-admin-users-user-id)<br>`PUT /api/v2.1/admin/users/{user_id}/` | Differs | [our endpoint](/reference/api/accounts#put-api2-admin-users-user-id) | Use `/api2/admin/users/{id}/`; the id is numeric. |
| [Delete user](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-users-user-id)<br>`DELETE /api/v2.1/admin/users/{user_id}/` | Differs | [our endpoint](/reference/api/accounts#delete-api2-admin-users-user-id) | Use `/api2/admin/users/{id}/`; the id is numeric. |

### Departments

No departments/address book.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List top level departments](https://seafile-api.readme.io/reference/get_api-v2-1-admin-address-book-groups)<br>`GET /api/v2.1/admin/address-book/groups/` | Missing | — |  |
| [Add a department](https://seafile-api.readme.io/reference/post_api-v2-1-admin-address-book-groups)<br>`POST /api/v2.1/admin/address-book/groups/` | Missing | — |  |
| [List groups and members in a department](https://seafile-api.readme.io/reference/get_api-v2-1-admin-address-book-groups-group-id)<br>`GET /api/v2.1/admin/address-book/groups/{group_id}/` | Missing | — |  |
| [Delete a department](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-address-book-groups-group-id)<br>`DELETE /api/v2.1/admin/address-book/groups/{group_id}/` | Missing | — |  |

### Department libraries

No groups or group-owned libraries.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Add group owned library](https://seafile-api.readme.io/reference/post_api-v2-1-admin-groups-group-id-group-owned-libraries)<br>`POST /api/v2.1/admin/groups/{group_id}/group-owned-libraries/` | Missing | — |  |
| [Delete group owned library](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id-group-owned-libraries-repo-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/group-owned-libraries/{repo_id}/` | Missing | — |  |

### Groups

No groups.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get all groups](https://seafile-api.readme.io/reference/get_api-v2-1-admin-groups)<br>`GET /api/v2.1/admin/groups/` | Missing | — |  |
| [Create a new group](https://seafile-api.readme.io/reference/post_api-v2-1-admin-groups)<br>`POST /api/v2.1/admin/groups/` | Missing | — |  |
| [Delete a group](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/` | Missing | — |  |
| [Transfer a group/Set department quota](https://seafile-api.readme.io/reference/put_api-v2-1-admin-groups-group-id)<br>`PUT /api/v2.1/admin/groups/{group_id}/` | Missing | — |  |
| [Search group](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-group)<br>`GET /api/v2.1/admin/search-group/` | Missing | — |  |
| [List group members](https://seafile-api.readme.io/reference/get_api-v2-1-admin-groups-group-id-members)<br>`GET /api/v2.1/admin/groups/{group_id}/members/` | Missing | — |  |
| [Add group member](https://seafile-api.readme.io/reference/post_api-v2-1-admin-groups-group-id-members)<br>`POST /api/v2.1/admin/groups/{group_id}/members/` | Missing | — |  |
| [Delete group member](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id-members-user-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/members/{user_id}/` | Missing | — |  |
| [Set(Unset) a group member as admin](https://seafile-api.readme.io/reference/put_api-v2-1-admin-groups-group-id-members-user-id)<br>`PUT /api/v2.1/admin/groups/{group_id}/members/{user_id}/` | Missing | — |  |
| [List libraries shared to a group](https://seafile-api.readme.io/reference/get_api-v2-1-admin-groups-group-id-libraries)<br>`GET /api/v2.1/admin/groups/{group_id}/libraries/` | Missing | — |  |
| [Remove a library from group](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id-libraries-repo-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/libraries/{repo_id}/` | Missing | — |  |

### Libraries

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get all libraries/Search library by owner](https://seafile-api.readme.io/reference/get_api-v2-1-admin-libraries)<br>`GET /api/v2.1/admin/libraries/` | Missing | — |  |
| [Create library](https://seafile-api.readme.io/reference/post_api-v2-1-admin-libraries)<br>`POST /api/v2.1/admin/libraries/` | Missing | — |  |
| [Search library by name](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-library)<br>`GET /api/v2.1/admin/search-library/` | Missing | — |  |
| [Get a library info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-libraries-repo-id)<br>`GET /api/v2.1/admin/libraries/{repo_id}/` | Missing | — |  |
| [Delete a library](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-libraries-repo-id)<br>`DELETE /api/v2.1/admin/libraries/{repo_id}/` | Missing | — |  |
| [Update a library status/Transfer a library](https://seafile-api.readme.io/reference/put_api-v2-1-admin-libraries-repo-id)<br>`PUT /api/v2.1/admin/libraries/{repo_id}/` | Missing | — |  |
| [Get all deleted libraries/Get deleted libraries by owner](https://seafile-api.readme.io/reference/get_api-v2-1-admin-trash-libraries)<br>`GET /api/v2.1/admin/trash-libraries/` | Missing | — |  |
| [Clean all deleted libraries](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-trash-libraries)<br>`DELETE /api/v2.1/admin/trash-libraries/` | Missing | — |  |
| [Clean deleted library](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-trash-libraries-repo-id)<br>`DELETE /api/v2.1/admin/trash-libraries/{repo_id}/` | Missing | — |  |
| [Restore deleted library](https://seafile-api.readme.io/reference/put_api-v2-1-admin-trash-libraries-repo-id)<br>`PUT /api/v2.1/admin/trash-libraries/{repo_id}/` | Missing | — |  |
| [Get library history setting](https://seafile-api.readme.io/reference/get_api-v2-1-admin-libraries-repo-id-history-limit)<br>`GET /api/v2.1/admin/libraries/{repo_id}/history-limit/` | Missing | — |  |
| [Update library history setting](https://seafile-api.readme.io/reference/put_api-v2-1-admin-libraries-repo-id-history-limit)<br>`PUT /api/v2.1/admin/libraries/{repo_id}/history-limit/` | Missing | — |  |
| [Create library folder](https://seafile-api.readme.io/reference/post_api-v2-1-admin-libraries-repo-id-dirents)<br>`POST /api/v2.1/admin/libraries/{repo_id}/dirents/` | Missing | — |  |

### Logs

No admin log API; logs are files on disk.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get login log](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-login)<br>`GET /api/v2.1/admin/logs/login/` | Missing | — |  |
| [Get file access log](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-file-audit)<br>`GET /api/v2.1/admin/logs/file-audit/` | Missing | — |  |
| [Get file update logs](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-file-update)<br>`GET /api/v2.1/admin/logs/file-update/` | Missing | — |  |
| [Get permission audit logs](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-perm-audit)<br>`GET /api/v2.1/admin/logs/perm-audit/` | Missing | — |  |

### Notifications

No admin notification feed.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get notifications](https://seafile-api.readme.io/reference/get_api-v2-1-admin-notifications)<br>`GET /api/v2.1/admin/notifications/` | Missing | — |  |

### Organizations

No organizations.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get organizations](https://seafile-api.readme.io/reference/get_api-v2-1-admin-organizations)<br>`GET /api/v2.1/admin/organizations/` | Missing | — |  |
| [Add organizations](https://seafile-api.readme.io/reference/post_api-v2-1-admin-organizations)<br>`POST /api/v2.1/admin/organizations/` | Missing | — |  |
| [Search organizations](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-organization)<br>`GET /api/v2.1/admin/search-organization/` | Missing | — |  |
| [Get organization info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-organizations-org-id)<br>`GET /api/v2.1/admin/organizations/{org_id}/` | Missing | — |  |
| [Update organization info](https://seafile-api.readme.io/reference/put_api-v2-1-admin-organizations-org-id)<br>`PUT /api/v2.1/admin/organizations/{org_id}/` | Missing | — |  |
| [Delete organization](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-organizations-org-id)<br>`DELETE /api/v2.1/admin/organizations/{org_id}/` | Missing | — |  |

### Share

No user/group shares.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get repo user/group shares](https://seafile-api.readme.io/reference/get_api-v2-1-admin-shares)<br>`GET /api/v2.1/admin/shares/` | Missing | — |  |
| [Share repo to user/group](https://seafile-api.readme.io/reference/post_api-v2-1-admin-shares)<br>`POST /api/v2.1/admin/shares/` | Missing | — |  |
| [Modify repo user/group share permission](https://seafile-api.readme.io/reference/put_api-v2-1-admin-shares)<br>`PUT /api/v2.1/admin/shares/` | Missing | — |  |
| [Delete repo user/group share](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-shares)<br>`DELETE /api/v2.1/admin/shares/` | Missing | — |  |
| [List shared folders](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users-user-id-shared-folders)<br>`GET /api/v2.1/admin/users/{user_id}/shared-folders/` | Missing | — |  |
| [List shared repos](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users-user-id-shared-repos)<br>`GET /api/v2.1/admin/users/{user_id}/shared-repos/` | Missing | — |  |

### Share links

No admin-wide share-link API.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get all share links](https://seafile-api.readme.io/reference/get_api-v2-1-admin-share-links)<br>`GET /api/v2.1/admin/share-links/` | Missing | — |  |
| [Get shared file/dir info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-share-links-token)<br>`GET /api/v2.1/admin/share-links/{token}/` | Missing | — |  |
| [Delete share link](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-share-links-token)<br>`DELETE /api/v2.1/admin/share-links/{token}/` | Missing | — |  |
| [List items in folder shared links](https://seafile-api.readme.io/reference/get_api-v2-1-admin-share-links-token-dirents)<br>`GET /api/v2.1/admin/share-links/{token}/dirents/` | Missing | — |  |

## Metadata Operations - User

### Metadata config

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get metadata enabled status](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata)<br>`GET /api/v2.1/repos/{repo_id}/metadata/` | Differs | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata) | Reports `face_recognition_enabled`/`summary_enabled`; `ocr_enabled` and `tags_lang` are absent. |
| [Enabled metadata](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/` | Same | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata) |  |
| [Disable metadata](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/` | Differs | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata) | Disable metadata with `PUT {"enabled": false}`. |
| [Details settings](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-details-settings)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/details-settings/` | Missing | — | `details_settings` is always `"{}"`. |
| [Extract file details](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-extract-file-details)<br>`POST /api/v2.1/repos/{repo_id}/metadata/extract-file-details/` | Missing | — |  |

### Metadata records

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List metadata records](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-records)<br>`GET /api/v2.1/repos/{repo_id}/metadata/records/` | Missing | — | Only a single record is available (`/metadata/record/`). |
| [Update metadata records](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-records)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/records/` | Missing | — | Update one record at a time through `/metadata/record/`. |
| [Get metadata record](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-record)<br>`GET /api/v2.1/repos/{repo_id}/metadata/record/` | Differs | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-record) | Returns the record itself; the official `{metadata, results}` column envelope is not produced. |
| [Update metadata record](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-record)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/record/` | Differs | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-record) | Addresses the file by `record_id`; the official `parent_dir`/`file_name` body is not read. |
| [Add column](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-columns)<br>`POST /api/v2.1/repos/{repo_id}/metadata/columns/` | Missing | — | Columns are not modelled; records store free-form JSON. |

### Metadata views

Views are not implemented.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [List views](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-views)<br>`GET /api/v2.1/repos/{repo_id}/metadata/views` | Missing | — |  |
| [Add view](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-views)<br>`POST /api/v2.1/repos/{repo_id}/metadata/views` | Missing | — |  |
| [Update view](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-views)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/views` | Missing | — |  |
| [Delete view](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-views)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/views` | Missing | — |  |
| [Get a view](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-views-view-id)<br>`GET /api/v2.1/repos/{repo_id}/metadata/views/{view_id}/` | Missing | — |  |
| [Move view](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-move-views)<br>`POST /api/v2.1/repos/{repo_id}/metadata/move-views/` | Missing | — |  |
| [Duplicate view](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-duplicate-view)<br>`POST /api/v2.1/repos/{repo_id}/metadata/duplicate-view/` | Missing | — |  |

### Tags

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Turn on tags feature](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-tags-status)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/tags-status/` | Same | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-tags-status) |  |
| [Turn off tags feature](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-tags-status)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/tags-status/` | Same | [our endpoint](/reference/api/metadata#delete-api-v2-1-repos-repo-id-metadata-tags-status) |  |
| [List tags](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-tags)<br>`GET /api/v2.1/repos/{repo_id}/metadata/tags/` | Same | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Add tags](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-tags)<br>`POST /api/v2.1/repos/{repo_id}/metadata/tags/` | Same | [our endpoint](/reference/api/metadata#post-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Update tags](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-tags)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/tags/` | Same | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Delete tags](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-tags)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/tags/` | Same | [our endpoint](/reference/api/metadata#delete-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Update file tags](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-file-tags)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/file-tags` | Differs | [our endpoint](/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-file-tags) | Returns `{success: true}`; the official `{success, fail}` id lists are not produced. |
| [List tag files](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-tag-files-tag-id)<br>`GET /api/v2.1/repos/{repo_id}/metadata/tag-files/{tag_id}/` | Same | [our endpoint](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tag-files-tag-id) |  |
| [List tags files](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-tags-files)<br>`POST /api/v2.1/repos/{repo_id}/metadata/tags-files/` | Missing | — |  |
| [Add tags links](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-tags-links)<br>`POST /api/v2.1/repos/{repo_id}/metadata/tags-links/` | Missing | — |  |
| [Update tags links](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-tags-links)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/tags-links/` | Missing | — |  |
| [Delete tags links](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-tags-links)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/tags-links/` | Missing | — |  |
| [Merge tags](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-merge-tags)<br>`POST /api/v2.1/repos/{repo_id}/metadata/merge-tags/` | Missing | — |  |

### OCR

OCR is not implemented; the feature reports off.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Turn on OCR feature](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-ocr)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/ocr/` | Missing | — |  |
| [Turn off OCR feature](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-ocr)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/ocr/` | Missing | — |  |

### Face recognition

Face recognition is not implemented; the feature reports off.

| Official operation | Status | Nanofile | Notes |
| --- | --- | --- | --- |
| [Get face recognition enabled status](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-face-recognition)<br>`GET /api/v2.1/repos/{repo_id}/metadata/face-recognition/` | Missing | — |  |
| [Open face recognition](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-face-recognition)<br>`POST /api/v2.1/repos/{repo_id}/metadata/face-recognition/` | Missing | — |  |
| [List face records](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-face-records)<br>`GET /api/v2.1/repos/{repo_id}/metadata/face-records/` | Missing | — |  |
| [List people photos](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-people-photos-people-id)<br>`GET /api/v2.1/repos/{repo_id}/metadata/people-photos/{people_id}/` | Missing | — |  |
| [Remove people photos](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-people-photos-people-id)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/people-photos/{people_id}/` | Missing | — |  |
| [Add people photos](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-people-photos-people-id)<br>`POST /api/v2.1/repos/{repo_id}/metadata/people-photos/{people_id}/` | Missing | — |  |
| [Update face name](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-face-record)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/face-record/` | Missing | — |  |
| [Update people cover photo](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-people-cover-photo-people-id)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/people-cover-photo/{people_id}/` | Missing | — |  |

## Served here, absent from the official reference {#undocumented}

These routes are served but are not an operation in the official reference. Most are seahub endpoints the reference omits (the v2.1 variants, legacy transfer helpers, trash2, related-users); the ones marked \* are Nanofile-only. The `/seafhttp/`, `/dav/` and `/notification` surfaces, and the browser transfer endpoints (`/upload-aj/`, `/download-api/`, `/zip/`, the progress shims), are documented on their own pages and are not repeated here.

| Endpoint | Documented at |
| --- | --- |
| `GET /api2/auth/ping/` | [docs](/reference/api/authentication#get-api2-auth-ping) |
| `POST /api2/logout-device/` | [docs](/reference/api/authentication#post-api2-logout-device) |
| `GET /api2/ping/` | [docs](/reference/api/accounts#get-api2-ping) |
| `POST /api2/device-wiped/` | [docs](/reference/api/authentication#post-api2-device-wiped) |
| `POST /api2/client-sso-link/` | [docs](/reference/api/authentication#post-api2-client-sso-link) |
| `GET /api2/client-sso-link/{token}/` | [docs](/reference/api/authentication#get-api2-client-sso-link-token) |
| `PUT /api2/account/info/` | [docs](/reference/api/accounts#put-api2-account-info) |
| `POST /api2/account/` | [docs](/reference/api/accounts#post-api2-account) |
| `POST /api2/accounts/` | [docs](/reference/api/accounts#post-api2-accounts) |
| `GET /api2/credentials/` \* | [docs](/reference/api/api-keys#get-api2-credentials) |
| `DELETE /api2/credentials/{kind}/{id}/` \* | [docs](/reference/api/api-keys#delete-api2-credentials-kind-id) |
| `GET /api2/unseen_messages/` | [docs](/reference/api/accounts#get-api2-unseen-messages) |
| `GET /api/v2.1/repos/` | [docs](/reference/api/libraries#get-api-v2-1-repos) |
| `GET /api/v2.1/repos/{repo_id}/` | [docs](/reference/api/libraries#get-api-v2-1-repos-repo-id) |
| `DELETE /api/v2.1/repos/{repo_id}/` | [docs](/reference/api/libraries#delete-api-v2-1-repos-repo-id) |
| `GET /api2/repos/{repo_id}/dir/sub_repo/` | [docs](/reference/api/directories#get-api2-repos-repo-id-dir-sub-repo) |
| `POST /api/v2.1/repos/{repo_id}/set-password/` | [docs](/reference/api/libraries#post-api-v2-1-repos-repo-id-set-password) |
| `PUT /api/v2.1/repos/{repo_id}/set-password/` | [docs](/reference/api/libraries#put-api-v2-1-repos-repo-id-set-password) |
| `GET /api/v2.1/deleted-repos/` | [docs](/reference/api/history#get-api-v2-1-deleted-repos) |
| `POST /api/v2.1/deleted-repos/` | [docs](/reference/api/history#post-api-v2-1-deleted-repos) |
| `DELETE /api/v2.1/deleted-repos/` | [docs](/reference/api/history#delete-api-v2-1-deleted-repos) |
| `DELETE /api/v2.1/deleted-repos/{repo_id}/` | [docs](/reference/api/history#delete-api-v2-1-deleted-repos-repo-id) |
| `GET /api/v2.1/repos/{repo_id}/dir/` | [docs](/reference/api/directories#get-api-v2-1-repos-repo-id-dir) |
| `POST /api/v2.1/repos/{repo_id}/dir/` | [docs](/reference/api/directories#post-api-v2-1-repos-repo-id-dir) |
| `DELETE /api/v2.1/repos/{repo_id}/dir/` | [docs](/reference/api/directories#delete-api-v2-1-repos-repo-id-dir) |
| `POST /api2/repos/{repo_id}/dir/move/` \* | [docs](/reference/api/directories#post-api2-repos-repo-id-dir-move) |
| `POST /api2/repos/{repo_id}/dir/rename/` \* | [docs](/reference/api/directories#post-api2-repos-repo-id-dir-rename) |
| `POST /api2/repos/{repo_id}/file/` | [docs](/reference/api/files#post-api2-repos-repo-id-file) |
| `DELETE /api2/repos/{repo_id}/file/` | [docs](/reference/api/files#delete-api2-repos-repo-id-file) |
| `POST /api2/repos/{repo_id}/file/rename/` \* | [docs](/reference/api/files#post-api2-repos-repo-id-file-rename) |
| `POST /api2/repos/{repo_id}/file/move/` \* | [docs](/reference/api/files#post-api2-repos-repo-id-file-move) |
| `GET /api2/repos/{repo_id}/file/exif/` \* | [docs](/reference/api/files#get-api2-repos-repo-id-file-exif) |
| `GET /api2/repos/{repo_id}/file/index-text/` | [docs](/reference/api/metadata#get-api2-repos-repo-id-file-index-text) |
| `POST /api2/repos/{repo_id}/file/reindex/` | [docs](/reference/api/metadata#post-api2-repos-repo-id-file-reindex) |
| `GET /api2/repos/{repo_id}/thumbnail/` | [docs](/reference/api/files#get-api2-repos-repo-id-thumbnail) |
| `GET /api2/repos/{repo_id}/file-uploaded-bytes/` | [docs](/reference/api/transfers#get-api2-repos-repo-id-file-uploaded-bytes) |
| `GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/` | [docs](/reference/api/transfers#get-api-v2-1-repos-repo-id-file-uploaded-bytes) |
| `GET /repos/{repo_id}/files/{*path}` | [docs](/reference/api/transfers#get-repos-repo-id-files-path) |
| `GET /api2/repos/{repo_id}/upload-blks-link/` | [docs](/reference/api/libraries#get-api2-repos-repo-id-upload-blks-link) |
| `GET /api2/repos/{repo_id}/update-blks-link/` | [docs](/reference/api/libraries#get-api2-repos-repo-id-update-blks-link) |
| `GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/` | [docs](/reference/api/libraries#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link) |
| `POST /api2/repos/{repo_id}/fileops/delete/` | [docs](/reference/api/files#post-api2-repos-repo-id-fileops-delete) |
| `POST /api2/repos/{repo_id}/fileops/copy/` | [docs](/reference/api/files#post-api2-repos-repo-id-fileops-copy) |
| `POST /api2/repos/{repo_id}/fileops/move/` | [docs](/reference/api/files#post-api2-repos-repo-id-fileops-move) |
| `POST /api/v2.1/copy-move-task/` | [docs](/reference/api/files#post-api-v2-1-copy-move-task) |
| `POST /api/v2.1/repos/{repo_id}/trash/revert-dirents/` | [docs](/reference/api/history#post-api-v2-1-repos-repo-id-trash-revert-dirents) |
| `GET /api/v2.1/repos/{repo_id}/trash2/` | [docs](/reference/api/history#get-api-v2-1-repos-repo-id-trash2) |
| `GET /api/v2.1/repos/{repo_id}/trash2/search/` | [docs](/reference/api/history#get-api-v2-1-repos-repo-id-trash2-search) |
| `POST /api/v2.1/repos/{repo_id}/trash2/revert/` | [docs](/reference/api/history#post-api-v2-1-repos-repo-id-trash2-revert) |
| `GET /api2/repo_history_changes/{repo_id}/` | [docs](/reference/api/history#get-api2-repo-history-changes-repo-id) |
| `POST /api/v2.1/repos/{repo_id}/file/revision/restore/` \* | [docs](/reference/api/history#post-api-v2-1-repos-repo-id-file-revision-restore) |
| `GET /api2/shared-links/` | [docs](/reference/api/sharing#get-api2-shared-links) |
| `POST /api2/shared-links/` | [docs](/reference/api/sharing#post-api2-shared-links) |
| `DELETE /api2/shared-links/{token}` | [docs](/reference/api/sharing#delete-api2-shared-links-token) |
| `GET /api/v2.1/share-links/{token}/` | [docs](/reference/api/sharing#get-api-v2-1-share-links-token) |
| `PUT /api/v2.1/share-links/{token}/` | [docs](/reference/api/sharing#put-api-v2-1-share-links-token) |
| `GET /api2/upload-links/` | [docs](/reference/api/sharing#get-api2-upload-links) |
| `POST /api2/upload-links/` | [docs](/reference/api/sharing#post-api2-upload-links) |
| `DELETE /api2/upload-links/{token}` | [docs](/reference/api/sharing#delete-api2-upload-links-token) |
| `DELETE /api/v2.1/upload-links/clean-invalid/` | [docs](/reference/api/sharing#delete-api-v2-1-upload-links-clean-invalid) |
| `GET /api/v2.1/upload-links/{token}/` | [docs](/reference/api/sharing#get-api-v2-1-upload-links-token) |
| `PUT /api/v2.1/upload-links/{token}/` | [docs](/reference/api/sharing#put-api-v2-1-upload-links-token) |
| `GET /api/v2.1/repos/{repo_id}/upload-links/` | [docs](/reference/api/sharing#get-api-v2-1-repos-repo-id-upload-links) |
| `GET /api/v2.1/groups/` | [docs](/reference/api/sharing#get-api-v2-1-groups) |
| `GET /api/v2.1/repos/{repo_id}/related-users/` | [docs](/reference/api/metadata#get-api-v2-1-repos-repo-id-related-users) |
| `GET /api/v2.1/repos/{repo_id}/metadata/tags-status/` | [docs](/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tags-status) |
| `GET /api2/starredfiles/` | [docs](/reference/api/sharing#get-api2-starredfiles) |
| `POST /api2/reindex/` | [docs](/reference/api/metadata#post-api2-reindex) |
| `GET /api2/reindex-progress/` | [docs](/reference/api/metadata#get-api2-reindex-progress) |
| `POST /api2/index-file-text/` | [docs](/reference/api/metadata#post-api2-index-file-text) |
| `GET /api2/repos/{repo_id}/download-info/` | [docs](/reference/api/libraries#get-api2-repos-repo-id-download-info) |
| `GET /api2/api-keys/catalog/` \* | [docs](/reference/api/api-keys#get-api2-api-keys-catalog) |
| `POST /api2/api-keys/` \* | [docs](/reference/api/api-keys#post-api2-api-keys) |
| `GET /api2/api-keys/{id}/` \* | [docs](/reference/api/api-keys#get-api2-api-keys-id) |

## Behaviour differences {#behaviour}

Route-level rows cannot capture everything. These differences apply across the surface; the endpoint pages and [Seafile compatibility](/dev/protocol#deliberate-deviations) give the protocol-level detail.

- **Credentials.** Besides the account token, Nanofile accepts API keys (capability-scoped, library-scoped) and, on `/seafhttp/`, repository sync tokens. Repo-Token callers use a library-scoped API key; the `/via-repo-token/` mirror is not routed.
- **Authorization header.** Both `Bearer` and the legacy `Token` prefix are accepted on every API route, so clients written for pre-11 and post-11 Seafile both work.
- **Errors.** Authentication failures carry both `detail` and `error_msg`; other errors carry `error_msg`. Login failures use DRF's `non_field_errors`; a locked file is plain text. Seafile's 440–446 and 520 status codes are used where the wire protocol needs them.
- **Version string.** `/api2/server-info/` reports `server.version` (`8.0.0` by default). Clients gate features on it; this page describes the routes that actually exist, independent of that string.
- **Timestamps.** Most are Unix seconds. `file/detail/` and `search-file` use RFC 3339; `dir/detail/` returns a number where seahub returns a string.
- **Listings.** The legacy directory listing hides `_Internal`, sorts directories first by lowercased name, and reports the real `dir_perm`/`permission`; `?oid=` revalidation is not implemented.
- **Search.** `/api/v2.1/search-file/` caps results at 200; `/api2/search/` ignores the Pro-only filters.
- **Transfers.** Upload/update links point at `/upload-api/` and `/update-api/` (no `/seafhttp/` prefix); clients use the returned URL verbatim. `zip-task` mints with `POST`.
- **WebDAV.** It lives at `/dav/{repo_id}/` (not `/seafdav`), takes an email plus a WebDAV API key as Basic credentials, refuses encrypted libraries with `403`, and does not store properties or enforce locks.
- **Notification.** `/notification` speaks the Seafile notification protocol with per-subscription JWTs; it is not part of the Web API reference.
- **No user-to-user sharing.** There are no shares to users or groups, no per-folder permissions, no comments, wikis, invitations, snapshot labels, OCR or face recognition. Those official groups are `Missing` above.

## WebDAV {#webdav}

WebDAV is not part of Seafile's Web API reference. Nanofile's surface is documented in [WebDAV](/reference/api/webdav); the deliberate differences are authentication (WebDAV key, not the account password), the `/dav/{repo_id}/` path, the refusal to serve encrypted libraries, and the no-op `PROPPATCH`/`LOCK`.
