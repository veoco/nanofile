# 与 Seafile API 的兼容性

本页把 Seafile 官方 API 参考中的每个操作与 Nanofile 实际提供的接口逐项对照，回答两个问题：官方接口哪些这里有，以及这里有哪些官方参考从未提到的接口。

基线是上游参考的 **v13.0**（站点默认版本），抓取于 **2026-10-24**：五个分节共 304 个操作。

## 图例 {#legend}

- **一致** —— method 与路径模板相同（参数名可以不同）。响应以链接到的接口页为准，允许 Nanofile 特有的附加字段。
- **有差异** —— 功能能做，但路由、请求/响应形状或某个文档化参数不同；说明里写清楚。
- **未实现** —— 没有对应实现；有替代方式时写在说明里。
- **不适用** —— 属于本服务器完全不暴露的平行接口面。

`一致` 是路由层面的约定，不是逐字节承诺：具体响应看链接到的页面。

## 汇总 {#summary}

| 分节 | 官方操作数 | 一致 | 有差异 | 未实现 | 不适用 |
| --- | --- | --- | --- | --- | --- |
| authentication | 2 | 1 | 1 | 0 | 0 |
| Via Repo-Token | 40 | 0 | 3 | 0 | 37 |
| Account Operations - User | 165 | 25 | 40 | 100 | 0 |
| Account Operations - System Admin | 57 | 0 | 4 | 53 | 0 |
| Metadata Operations - User | 40 | 8 | 5 | 27 | 0 |
| **Total** | **304** | **34** | **53** | **180** | **37** |

## authentication

### Account-Token

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Obtain account token](https://seafile-api.readme.io/reference/post_api2-auth-token)<br>`POST /api2/auth-token/` | 一致 | [本地接口](/zh/reference/api/authentication#post-api2-auth-token) |  |

### Repo-Token

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Generate repo api token](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-repo-api-tokens)<br>`POST /api/v2.1/repos/{repo_id}/repo-api-tokens/` | 有差异 | [本地接口](/zh/reference/api/libraries#get-api2-repo-tokens) | 旧版 `/api2/repo-tokens/` 签发同步令牌，没有 HTTP repo-token 接口。需要受限访问请用 `/api2/api-keys/`。 |

## Via Repo-Token

`/api/v2.1/via-repo-token/` 是给 Repo-Token 持有者用的镜像接口面。Nanofile 不路由这个前缀：绑定资料库的 API 密钥直接认证普通的 `/api2/` 与 `/api/v2.1/` 路由，因此镜像操作在这里标为 `不适用`；本节的三个令牌管理操作按 Nanofile 的 API 密钥判定。

### API tokens

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List repo api tokens](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-repo-api-tokens)<br>`GET /api/v2.1/repos/{repo_id}/repo-api-tokens/` | 有差异 | [本地接口](/zh/reference/api/api-keys#get-api2-api-keys) | 没有对应的路由；API 密钥是等效的受限凭据。 |
| [Update repo api token](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-repo-api-tokens-app-name)<br>`PUT /api/v2.1/repos/{repo_id}/repo-api-tokens/{app_name}/` | 有差异 | [本地接口](/zh/reference/api/api-keys#put-api2-api-keys-id) | 未实现。可通过 `/api2/api-keys/{id}/` 重命名或调整 API 密钥。 |
| [Delete repo api token](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-repo-api-tokens-app-name)<br>`DELETE /api/v2.1/repos/{repo_id}/repo-api-tokens/{app_name}/` | 有差异 | [本地接口](/zh/reference/api/api-keys#delete-api2-api-keys-id) | 未实现。请吊销等效的 API 密钥。 |

### Dir / Files

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List items in directory](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-dir)<br>`GET /api/v2.1/via-repo-token/dir/` | 不适用 | — |  |
| [Operation directory](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-dir)<br>`POST /api/v2.1/via-repo-token/dir/` | 不适用 | — |  |
| [Delete dir](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-dir)<br>`DELETE /api/v2.1/via-repo-token/dir/` | 不适用 | — |  |
| [Get file info](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-file)<br>`GET /api/v2.1/via-repo-token/file/` | 不适用 | — |  |
| [Operation file](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-file)<br>`POST /api/v2.1/via-repo-token/file/` | 不适用 | — |  |
| [Lock/Unlock file](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-file)<br>`PUT /api/v2.1/via-repo-token/file/` | 不适用 | — |  |
| [Delete file](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-file)<br>`DELETE /api/v2.1/via-repo-token/file/` | 不适用 | — |  |
| [Move directory with items merged](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-move-dir)<br>`POST /api/v2.1/via-repo-token/move-dir/` | 不适用 | — |  |
| [Get upload link](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-upload-link)<br>`GET /api/v2.1/via-repo-token/upload-link/` | 不适用 | — |  |
| [Get download link](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-download-link)<br>`GET /api/v2.1/via-repo-token/download-link/` | 不适用 | — |  |
| [Get repo info](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-repo-info)<br>`GET /api/v2.1/via-repo-token/repo-info/` | 不适用 | — |  |
| [Sync batch move item](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-sync-batch-move-item)<br>`POST /api/v2.1/via-repo-token/sync-batch-move-item/` | 不适用 | — |  |
| [Sync batch copy item](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-sync-batch-copy-item)<br>`POST /api/v2.1/via-repo-token/sync-batch-copy-item/` | 不适用 | — |  |
| [Batch delete items](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-batch-delete-item)<br>`DELETE /api/v2.1/via-repo-token/batch-delete-item/` | 不适用 | — |  |

### Share links

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Create share link](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-share-links)<br>`POST /api/v2.1/via-repo-token/share-links/` | 不适用 | — |  |

### Metadata records

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List metadata records](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-records)<br>`GET /api/v2.1/via-repo-token/metadata/records/` | 不适用 | — |  |
| [Update metadata records](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-records)<br>`PUT /api/v2.1/via-repo-token/metadata/records/` | 不适用 | — |  |

### Metadata views

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List views](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-views)<br>`GET /api/v2.1/via-repo-token/metadata/views/` | 不适用 | — |  |
| [Add view](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-views)<br>`POST /api/v2.1/via-repo-token/metadata/views/` | 不适用 | — |  |
| [Update view](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-views)<br>`PUT /api/v2.1/via-repo-token/metadata/views/` | 不适用 | — |  |
| [Delete view](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-views)<br>`DELETE /api/v2.1/via-repo-token/metadata/views/` | 不适用 | — |  |
| [Get a view](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-views-view-id)<br>`GET /api/v2.1/via-repo-token/metadata/views/{view_id}/` | 不适用 | — |  |
| [Move view](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-move-views)<br>`POST /api/v2.1/via-repo-token/metadata/move-views/` | 不适用 | — |  |
| [Duplicate view](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-duplicate-view)<br>`POST /api/v2.1/via-repo-token/metadata/duplicate-view/` | 不适用 | — |  |

### Tags

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Update tags links](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-tags-links)<br>`PUT /api/v2.1/via-repo-token/metadata/tags-links/` | 不适用 | — |  |
| [Delete tags links](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-tags-links)<br>`DELETE /api/v2.1/via-repo-token/metadata/tags-links/` | 不适用 | — |  |
| [Turn on tags feature](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-tags-status)<br>`PUT /api/v2.1/via-repo-token/metadata/tags-status/` | 不适用 | — |  |
| [Turn off tags feature](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-tags-status)<br>`DELETE /api/v2.1/via-repo-token/metadata/tags-status/` | 不适用 | — |  |
| [List tags](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-tags)<br>`GET /api/v2.1/via-repo-token/metadata/tags/` | 不适用 | — |  |
| [Add tags](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-tags)<br>`POST /api/v2.1/via-repo-token/metadata/tags/` | 不适用 | — |  |
| [Update tags](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-tags)<br>`PUT /api/v2.1/via-repo-token/metadata/tags/` | 不适用 | — |  |
| [Delete tags](https://seafile-api.readme.io/reference/delete_api-v2-1-via-repo-token-metadata-tags)<br>`DELETE /api/v2.1/via-repo-token/metadata/tags/` | 不适用 | — |  |
| [Update file tags](https://seafile-api.readme.io/reference/put_api-v2-1-via-repo-token-metadata-file-tags)<br>`PUT /api/v2.1/via-repo-token/metadata/file-tags` | 不适用 | — |  |
| [List tag files](https://seafile-api.readme.io/reference/get_api-v2-1-via-repo-token-metadata-tag-files-tag-id)<br>`GET /api/v2.1/via-repo-token/metadata/tag-files/{tag_id}/` | 不适用 | — |  |
| [List tags files](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-tags-files)<br>`POST /api/v2.1/via-repo-token/metadata/tags-files/` | 不适用 | — |  |
| [Add tags links](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-tags-links)<br>`POST /api/v2.1/via-repo-token/metadata/tags-links/` | 不适用 | — |  |
| [Merge tags](https://seafile-api.readme.io/reference/post_api-v2-1-via-repo-token-metadata-merge-tags)<br>`POST /api/v2.1/via-repo-token/metadata/merge-tags/` | 不适用 | — |  |

## Account Operations - User

### Account

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get account info](https://seafile-api.readme.io/reference/get_api2-account-info)<br>`GET /api2/account/info/` | 有差异 | [本地接口](/zh/reference/api/accounts#get-api2-account-info) | 响应不包含 seahub 的 `login_id`、`is_staff`、`email_notification_interval`、`institution`、`department`。 |
| [Get client token](https://seafile-api.readme.io/reference/post_api2-client-login)<br>`POST /api2/client-login/` | 一致 | [本地接口](/zh/reference/api/authentication#post-api2-client-login) |  |

### Activities

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get file activities](https://seafile-api.readme.io/reference/get_api-v2-1-activities)<br>`GET /api/v2.1/activities/` | 一致 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-activities) |  |

### Avatars

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Upload/Update user avatar](https://seafile-api.readme.io/reference/post_api-v2-1-user-avatar)<br>`POST /api/v2.1/user-avatar/` | 一致 | [本地接口](/zh/reference/api/accounts#post-api-v2-1-user-avatar) |  |
| [Get user avatar](https://seafile-api.readme.io/reference/get_api2-avatars-user-user-resized-size)<br>`GET /api2/avatars/user/{user}/resized/{size}/` | 一致 | [本地接口](/zh/reference/api/accounts#get-api2-avatars-user-email-resized-size) |  |

### Department libraries

没有群组或群组-owned 资料库。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Add group owned library](https://seafile-api.readme.io/reference/post_api-v2-1-groups-group-id-group-owned-libraries)<br>`POST /api/v2.1/groups/{group_id}/group-owned-libraries/` | 未实现 | — |  |
| [Rename a group owned library](https://seafile-api.readme.io/reference/put_api-v2-1-groups-group-id-group-owned-libraries-repo-id)<br>`PUT /api/v2.1/groups/{group_id}/group-owned-libraries/{repo_id}/` | 未实现 | — |  |
| [Delete group owned library](https://seafile-api.readme.io/reference/delete_api-v2-1-groups-group-id-libraries-repo-id)<br>`DELETE /api/v2.1/groups/{group_id}/libraries/{repo_id}/` | 未实现 | — |  |
| [Get group owned library user share info](https://seafile-api.readme.io/reference/get_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`GET /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | 未实现 | — |  |
| [Share group owned library to user](https://seafile-api.readme.io/reference/post_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`POST /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | 未实现 | — |  |
| [Modify group owned library user share permission](https://seafile-api.readme.io/reference/put_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`PUT /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | 未实现 | — |  |
| [Delete group owned library user share](https://seafile-api.readme.io/reference/delete_api-v2-1-group-owned-libraries-repo-id-user-share)<br>`DELETE /api/v2.1/group-owned-libraries/{repo_id}/user-share/` | 未实现 | — |  |
| [Get group owned library group share info](https://seafile-api.readme.io/reference/get_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`GET /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | 未实现 | — |  |
| [Share group owned library to group](https://seafile-api.readme.io/reference/post_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`POST /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | 未实现 | — |  |
| [Modify group owned library group share permission](https://seafile-api.readme.io/reference/put_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`PUT /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | 未实现 | — |  |
| [Delete group owned library group share](https://seafile-api.readme.io/reference/delete_api-v2-1-group-owned-libraries-repo-id-group-share)<br>`DELETE /api/v2.1/group-owned-libraries/{repo_id}/group-share/` | 未实现 | — |  |

### Devices

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List devices](https://seafile-api.readme.io/reference/get_api2-devices)<br>`GET /api2/devices/` | 一致 | [本地接口](/zh/reference/api/api-keys#get-api2-devices) |  |
| [Unlink device](https://seafile-api.readme.io/reference/delete_api2-devices)<br>`DELETE /api2/devices/` | 有差异 | [本地接口](/zh/reference/api/api-keys#delete-api2-devices) | 解析 URL 编码表单（不是 JSON），忽略 `remote_wipe`；另支持 `kind`+`id` 寻址。 |

### Directories

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List items in directory](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-dir)<br>`GET /api2/repos/{repo_id}/dir/` | 有差异 | [本地接口](/zh/reference/api/directories#get-api2-repos-repo-id-dir) | 不支持 `?oid=` 校验；`_Internal` 被隐藏；条目按展示顺序排列，每个条目都带 `modifier_email`。 |
| [Create new or rename directory](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-dir)<br>`POST /api2/repos/{repo_id}/dir/` | 一致 | [本地接口](/zh/reference/api/directories#post-api2-repos-repo-id-dir) |  |
| [Delete directory](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-dir)<br>`DELETE /api2/repos/{repo_id}/dir/` | 一致 | [本地接口](/zh/reference/api/directories#delete-api2-repos-repo-id-dir) |  |
| [Get directory detail](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-dir-detail)<br>`GET /api/v2.1/repos/{repo_id}/dir/detail/` | 有差异 | [本地接口](/zh/reference/api/directories#get-api-v2-1-repos-repo-id-dir-detail) | `mtime` 是 JSON 数字；seahub 返回 ISO 8601 字符串。 |
| [Revert directory to a history status](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-dir-revert)<br>`PUT /api2/repos/{repo_id}/dir/revert/` | 未实现 | — | 请改用回收站或文件版本恢复。 |
| [Get task directory token](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-zip-task)<br>`GET /api/v2.1/repos/{repo_id}/zip-task/` | 有差异 | [本地接口](/zh/reference/api/transfers#post-api-v2-1-repos-repo-id-zip-task) | Nanofile 用 `POST` 生成压缩包令牌（网页界面的形状）。 |
| [Query task progress](https://seafile-api.readme.io/reference/get_api-v2-1-query-zip-progress)<br>`GET /api/v2.1/query-zip-progress/` | 未实现 | — | `zip-task` 是同步的；网页界面通过任务列表查看进度。 |
| [Move directory with items merged](https://seafile-api.readme.io/reference/post_api-v2-1-move-folder-merge)<br>`POST /api/v2.1/move-folder-merge/` | 未实现 | — | 请用 `dir/move/`；目录整体移动，不做合并。 |

### File comments

Nanofile 没有评论功能。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List file comments](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-comments)<br>`GET /api2/repos/{repo_id}/file/comments/` | 未实现 | — | 没有评论功能；`file/detail/` 始终报告 `comment_total: 0`。 |
| [Submit a file comment](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-file-comments)<br>`POST /api2/repos/{repo_id}/file/comments/` | 未实现 | — |  |
| [Get a file comment](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-comments-pk)<br>`GET /api2/repos/{repo_id}/file/comments/{pk}/` | 未实现 | — |  |
| [Update a file comment](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-file-comments-pk)<br>`PUT /api2/repos/{repo_id}/file/comments/{pk}/` | 未实现 | — |  |
| [Delete a file comment](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-file-comments-pk)<br>`DELETE /api2/repos/{repo_id}/file/comments/{pk}/` | 未实现 | — |  |
| [Get number of comments](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-comments-counts)<br>`GET /api2/repos/{repo_id}/file/comments/counts/` | 未实现 | — |  |

### File history

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get file history](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-file-history)<br>`GET /api/v2.1/repos/{repo_id}/file/history/` | 有差异 | [本地接口](/zh/reference/api/history#get-api-v2-1-repos-repo-id-file-history) | 外层结构是 `{data, page, page_next}`；没有 `next_start_commit`，且只返回一页。 |
| [Download file from a revision](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-revision)<br>`GET /api2/repos/{repo_id}/file/revision/` | 有差异 | [本地接口](/zh/reference/api/history#get-api-v2-1-repos-repo-id-file-revision) | 由 `/api/v2.1/repos/{repo_id}/file/revision/` 提供。 |

### Files

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Create/Rename/Move/Copy/Revert a file](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-file)<br>`POST /api/v2.1/repos/{repo_id}/file/` | 有差异 | [本地接口](/zh/reference/api/files#post-api-v2-1-repos-repo-id-file) | 只支持 `create` 与 `rename`；移动走旧版 `/api2/.../file/` 表单，复制用 `fileops`，回退用版本恢复。 |
| [Lock/Unlock file](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-file)<br>`PUT /api/v2.1/repos/{repo_id}/file/` | 有差异 | [本地接口](/zh/reference/api/files#put-api2-repos-repo-id-file) | 加锁/解锁在旧版 `/api2/repos/{repo_id}/file/` 的 PUT，`operation=lock\|unlock`。 |
| [Delete file](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-file)<br>`DELETE /api/v2.1/repos/{repo_id}/file/` | 一致 | [本地接口](/zh/reference/api/files#delete-api-v2-1-repos-repo-id-file) |  |
| [Download file](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file)<br>`GET /api2/repos/{repo_id}/file/` | 一致 | [本地接口](/zh/reference/api/files#get-api2-repos-repo-id-file) |  |
| [Get file detail](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-file-detail)<br>`GET /api2/repos/{repo_id}/file/detail/` | 一致 | [本地接口](/zh/reference/api/files#get-api2-repos-repo-id-file-detail) |  |
| [Get smart link for a file](https://seafile-api.readme.io/reference/get_api-v2-1-smart-link)<br>`GET /api/v2.1/smart-link/` | 有差异 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-smart-link) | 返回指向文件接口的 `{url}`；不产出官方的 `smart_link`/`smart_link_token`/`name` 结构。 |

### File search

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Search files in libraries](https://seafile-api.readme.io/reference/get_api2-search)<br>`GET /api2/search/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api2-search) | 忽略 Pro 专属过滤参数；全文索引关闭时也可用（只匹配文件名）。 |
| [Search files by name in library](https://seafile-api.readme.io/reference/get_api-v2-1-search-file)<br>`GET /api/v2.1/search-file/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-search-file) | 结果最多 200 条；上游会遍历整棵树。 |

### File tags

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List all tags of a library](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-repo-tags)<br>`GET /api/v2.1/repos/{repo_id}/repo-tags/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tags) | 标签由元数据接口提供。 |
| [Add a tag to a library](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-repo-tags)<br>`POST /api/v2.1/repos/{repo_id}/repo-tags/` | 有差异 | [本地接口](/zh/reference/api/metadata#post-api-v2-1-repos-repo-id-metadata-tags) | 向元数据标签接口发送 `tags_data`。 |
| [Update a tag of a library](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-repo-tags-repo-tag-id)<br>`PUT /api/v2.1/repos/{repo_id}/repo-tags/{repo_tag_id}/` | 有差异 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-tags) | id 放在 `tags_data` 里。 |
| [Delete a tag of a library](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-repo-tags-repo-tag-id)<br>`DELETE /api/v2.1/repos/{repo_id}/repo-tags/{repo_tag_id}/` | 有差异 | [本地接口](/zh/reference/api/metadata#delete-api-v2-1-repos-repo-id-metadata-tags) | id 放在 `tag_ids` 里。 |
| [List all tags of a file](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-file-tags)<br>`GET /api/v2.1/repos/{repo_id}/file-tags/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-record) | 文件的标签在记录的 `_tags` 里。 |
| [Add a tag for a file](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-file-tags)<br>`POST /api/v2.1/repos/{repo_id}/file-tags/` | 有差异 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-file-tags) | 通过元数据接口整体设置标签列表。 |
| [Delete a tag from a file](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-file-tags-file-tag-id)<br>`DELETE /api/v2.1/repos/{repo_id}/file-tags/{file_tag_id}/` | 有差异 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-file-tags) | 通过元数据接口设置剩余标签列表。 |
| [List tagged files by tag id](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-tagged-files-repo-tag-id)<br>`GET /api/v2.1/repos/{repo_id}/tagged-files/{repo_tag_id}/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tag-files-tag-id) | 由元数据接口提供。 |

### File upload

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get upload link](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-upload-link)<br>`GET /api2/repos/{repo_id}/upload-link/` | 一致 | [本地接口](/zh/reference/api/libraries#get-api2-repos-repo-id-upload-link) |  |
| [Upload file](https://seafile-api.readme.io/reference/post_seafhttp-upload-api-upload-token-ret-json-1)<br>`POST /seafhttp/upload-api/{upload_token}?ret-json=1` | 有差异 | [本地接口](/zh/reference/api/transfers#post-upload-api-token) | 上传链接指向 `/upload-api/<token>`，不使用 `/seafhttp/` 前缀。 |
| [Get update link](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-update-link)<br>`GET /api2/repos/{repo_id}/update-link/` | 一致 | [本地接口](/zh/reference/api/libraries#get-api2-repos-repo-id-update-link) |  |
| [Update file](https://seafile-api.readme.io/reference/post_seafhttp-update-api-upload-token)<br>`POST /seafhttp/update-api/{upload_token}/` | 有差异 | [本地接口](/zh/reference/api/transfers#post-update-api-token) | 更新链接指向 `/update-api/<token>`，不使用 `/seafhttp/` 前缀。 |

### Files/Directories batch operation

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Batch copy items synchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-sync-batch-copy-item)<br>`POST /api/v2.1/repos/sync-batch-copy-item/` | 一致 | [本地接口](/zh/reference/api/files#post-api-v2-1-repos-sync-batch-copy-item) |  |
| [Batch move items synchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-sync-batch-move-item)<br>`POST /api/v2.1/repos/sync-batch-move-item/` | 一致 | [本地接口](/zh/reference/api/files#post-api-v2-1-repos-sync-batch-move-item) |  |
| [Batch delete items](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-batch-delete-item)<br>`DELETE /api/v2.1/repos/batch-delete-item/` | 有差异 | [本地接口](/zh/reference/api/files#post-api-v2-1-repos-batch-delete-item) | 只返回 `{success}`，没有 `commit_id`。 |
| [Batch copy items asynchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-async-batch-copy-item)<br>`POST /api/v2.1/repos/async-batch-copy-item/` | 一致 | [本地接口](/zh/reference/api/files#post-api-v2-1-repos-async-batch-copy-item) |  |
| [Batch move items asynchronously](https://seafile-api.readme.io/reference/post_api-v2-1-repos-async-batch-move-item)<br>`POST /api/v2.1/repos/async-batch-move-item/` | 一致 | [本地接口](/zh/reference/api/files#post-api-v2-1-repos-async-batch-move-item) |  |
| [Query async operation progress](https://seafile-api.readme.io/reference/get_api-v2-1-query-copy-move-progress)<br>`GET /api/v2.1/query-copy-move-progress/` | 有差异 | — | 进度字段是 `state`/`done_count`/`total`/`cancelable`；没有 `canceled`，`done` 是布尔值。 |
| [Cancel async operation](https://seafile-api.readme.io/reference/delete_api-v2-1-copy-move-task)<br>`DELETE /api/v2.1/copy-move-task/` | 未实现 | — | 任务不能取消：进度里 `cancelable: false`。 |

### Groups

只有空列表桩；没有群组管理。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List groups](https://seafile-api.readme.io/reference/get_api2-groups)<br>`GET /api2/groups/` | 有差异 | [本地接口](/zh/reference/api/sharing#get-api2-groups) | 返回裸数组；官方结构是 `{replynum, groups}`。永远是空数组（没有群组）。 |
| [Add a group](https://seafile-api.readme.io/reference/post_api-v2-1-groups)<br>`POST /api/v2.1/groups/` | 未实现 | — |  |
| [Get info of a group](https://seafile-api.readme.io/reference/get_api-v2-1-groups-group-id)<br>`GET /api/v2.1/groups/{group_id}/` | 未实现 | — |  |
| [Rename/Transfer a group](https://seafile-api.readme.io/reference/put_api-v2-1-groups-group-id)<br>`PUT /api/v2.1/groups/{group_id}/` | 未实现 | — |  |
| [Delete a group](https://seafile-api.readme.io/reference/delete_api-v2-1-groups-group-id)<br>`DELETE /api/v2.1/groups/{group_id}/` | 未实现 | — |  |
| [List all group members](https://seafile-api.readme.io/reference/get_api-v2-1-groups-group-id-members)<br>`GET /api/v2.1/groups/{group_id}/members/` | 未实现 | — |  |
| [Add a group member](https://seafile-api.readme.io/reference/post_api-v2-1-groups-group-id-members)<br>`POST /api/v2.1/groups/{group_id}/members/` | 未实现 | — |  |
| [Bulk add group members](https://seafile-api.readme.io/reference/post_api-v2-1-groups-group-id-members-bulk)<br>`POST /api/v2.1/groups/{group_id}/members/bulk/` | 未实现 | — |  |
| [Get info of a group member](https://seafile-api.readme.io/reference/get_api-v2-1-groups-group-id-members-user-id)<br>`GET /api/v2.1/groups/{group_id}/members/{user_id}/` | 未实现 | — |  |
| [Set/Unset a group admin](https://seafile-api.readme.io/reference/put_api-v2-1-groups-group-id-members-user-id)<br>`PUT /api/v2.1/groups/{group_id}/members/{user_id}/` | 未实现 | — |  |
| [Leave group or group owner/admin delete a group member](https://seafile-api.readme.io/reference/delete_api-v2-1-groups-group-id-members-user-id)<br>`DELETE /api/v2.1/groups/{group_id}/members/{user_id}/` | 未实现 | — |  |

### Invitations

Nanofile 没有邀请功能；注册走网页流程或由管理员创建。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List invitations](https://seafile-api.readme.io/reference/get_api-v2-1-invitations)<br>`GET /api/v2.1/invitations/` | 未实现 | — |  |
| [Add invitation](https://seafile-api.readme.io/reference/post_api-v2-1-invitations)<br>`POST /api/v2.1/invitations/` | 未实现 | — |  |
| [Batch add invitations](https://seafile-api.readme.io/reference/post_api-v2-1-invitations-batch)<br>`POST /api/v2.1/invitations/batch/` | 未实现 | — |  |
| [Get invitation](https://seafile-api.readme.io/reference/get_api-v2-1-invitations-invitation-token)<br>`GET /api/v2.1/invitations/{invitation_token}/` | 未实现 | — |  |
| [Delete invitation](https://seafile-api.readme.io/reference/delete_api-v2-1-invitations-invitation-token)<br>`DELETE /api/v2.1/invitations/{invitation_token}/` | 未实现 | — |  |
| [Revoke invitation](https://seafile-api.readme.io/reference/delete_api-v2-1-invitations-invitation-token-revoke)<br>`DELETE /api/v2.1/invitations/{invitation_token}/revoke/` | 未实现 | — |  |

### Libraries

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get default library](https://seafile-api.readme.io/reference/get_api2-default-repo)<br>`GET /api2/default-repo/` | 一致 | [本地接口](/zh/reference/api/libraries#get-api2-default-repo) |  |
| [Create default library](https://seafile-api.readme.io/reference/post_api2-default-repo)<br>`POST /api2/default-repo/` | 有差异 | [本地接口](/zh/reference/api/libraries#post-api2-default-repo) | 只返回 `{repo_id}`，不回显 `exists`。 |
| [List libraries/Search library by name](https://seafile-api.readme.io/reference/get_api2-repos)<br>`GET /api2/repos/` | 有差异 | [本地接口](/zh/reference/api/libraries#get-api2-repos) | 没有 `?name=` 搜索过滤；条目不包含 `repo_size`/`mtime_relative`/relay 字段。 |
| [Create (Encrypted) library](https://seafile-api.readme.io/reference/post_api2-repos)<br>`POST /api2/repos/` | 有差异 | [本地接口](/zh/reference/api/libraries#post-api2-repos) | 响应缺少 `repo_size`、`repo_size_formatted`、`mtime_relative` 与 relay 字段。 |
| [Get library info](https://seafile-api.readme.io/reference/get_api2-repos-repo-id)<br>`GET /api2/repos/{repo_id}/` | 有差异 | [本地接口](/zh/reference/api/libraries#get-api2-repos-repo-id) | 响应没有 `file_count`，也没有库级 `modifier_*` 字段。 |
| [Rename/Decrypt library](https://seafile-api.readme.io/reference/post_api2-repos-repo-id)<br>`POST /api2/repos/{repo_id}/` | 有差异 | [本地接口](/zh/reference/api/libraries#post-api2-repos-repo-id) | 支持 `rename`、`update`、`setpassword`、`checkpassword`；没有解密操作。 |
| [Delete library](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id)<br>`DELETE /api2/repos/{repo_id}/` | 一致 | [本地接口](/zh/reference/api/libraries#delete-api2-repos-repo-id) |  |
| [Get library owner](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-owner)<br>`GET /api2/repos/{repo_id}/owner/` | 未实现 | — | 没有转让所有权的接口。 |
| [Transfer library](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-owner)<br>`PUT /api2/repos/{repo_id}/owner/` | 未实现 | — |  |
| [Get library history](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-history)<br>`GET /api/v2.1/repos/{repo_id}/history/` | 未实现 | — | 只有按提交查看变更的接口（`/api2/repo_history_changes/`）。 |
| [Get library history limit days](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-history-limit)<br>`GET /api2/repos/{repo_id}/history-limit/` | 未实现 | — |  |
| [Set library history limit days](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-history-limit)<br>`PUT /api2/repos/{repo_id}/history-limit/` | 未实现 | — |  |
| [Get library trash](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-trash)<br>`GET /api/v2.1/repos/{repo_id}/trash/` | 有差异 | [本地接口](/zh/reference/api/history#get-api-v2-1-repos-repo-id-trash) | 游标结构 `{items, has_more}`，不是 seahub 的 `{data, more, scan_stat}`。 |
| [Clean library trash](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-trash)<br>`DELETE /api/v2.1/repos/{repo_id}/trash/` | 一致 | [本地接口](/zh/reference/api/history#delete-api-v2-1-repos-repo-id-trash) |  |
| [Get library commit info](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-commits-commit-id)<br>`GET /api/v2.1/repos/{repo_id}/commits/{commit_id}/` | 未实现 | — |  |

### List share

不存在用户间分享。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List shared users/groups of a library](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-dir-shared-items)<br>`GET /api2/repos/{repo_id}/dir/shared_items/` | 有差异 | [本地接口](/zh/reference/api/directories#get-api2-repos-repo-id-dir-shared-items) | 列出下载分享链接；这里没有用户/群组分享。 |
| [List libraries shared to me](https://seafile-api.readme.io/reference/get_api2-beshared-repos)<br>`GET /api2/beshared-repos/` | 未实现 | — |  |

### Share to user/group

Nanofile 没有用户间或群组分享，只有链接分享。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Share a library to user/group](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-dir-shared-items)<br>`PUT /api2/repos/{repo_id}/dir/shared_items/` | 未实现 | — |  |
| [Unshare a library from user/group](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-dir-shared-items)<br>`DELETE /api2/repos/{repo_id}/dir/shared_items/` | 未实现 | — |  |
| [Update user share permission of a library](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-dir-shared-items)<br>`POST /api2/repos/{repo_id}/dir/shared_items/` | 未实现 | — |  |
| [Delete a library shared to me (leave the share)](https://seafile-api.readme.io/reference/delete_api2-beshared-repos-repo-id)<br>`DELETE /api2/beshared-repos/{repo_id}/` | 未实现 | — |  |
| [Batch share libraries to user/group](https://seafile-api.readme.io/reference/post_api-v2-1-repos-batch)<br>`POST /api/v2.1/repos/batch/` | 未实现 | — |  |
| [Share a folder](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-dir-shared-items-p-path)<br>`PUT /api2/repos/{repo_id}/dir/shared_items/p={path}` | 未实现 | — |  |
| [List shared folders](https://seafile-api.readme.io/reference/get_api-v2-1-shared-folders)<br>`GET /api/v2.1/shared-folders/` | 未实现 | — |  |
| [Update folder share permission](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-dir-shared-items-p-path)<br>`POST /api2/repos/{repo_id}/dir/shared_items/?p={path}` | 未实现 | — |  |
| [Remove the specified shared folder](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-dir-shared-items-p-path)<br>`DELETE /api2/repos/{repo_id}/dir/shared_items/?p={path}` | 未实现 | — |  |

### Server info

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get server information](https://seafile-api.readme.io/reference/get_api2-server-info)<br>`GET /api2/server-info/` | 一致 | [本地接口](/zh/reference/api/accounts#get-api2-server-info) |  |

### Share links

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List all share links](https://seafile-api.readme.io/reference/get_api-v2-1-share-links)<br>`GET /api/v2.1/share-links/` | 有差异 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-share-links) | 条目使用旧字段（`created_at`、`expire_at`、`s_type`）；没有 `repo_name`/`username`。 |
| [Create share link](https://seafile-api.readme.io/reference/post_api-v2-1-share-links)<br>`POST /api/v2.1/share-links/` | 有差异 | [本地接口](/zh/reference/api/sharing#post-api-v2-1-share-links) | 忽略 `permissions`；`repo_name`/`username` 为 null，权限固定为下载。 |
| [Create multi share links](https://seafile-api.readme.io/reference/post_api-v2-1-multi-share-links)<br>`POST /api/v2.1/multi-share-links/` | 有差异 | [本地接口](/zh/reference/api/sharing#post-api-v2-1-multi-share-links) | 只创建一条链接；不支持 `emails`/`user_scope` 批量创建。 |
| [Batch create shared links](https://seafile-api.readme.io/reference/post_api-v2-1-multi-share-links-batch)<br>`POST /api/v2.1/multi-share-links/batch/` | 未实现 | — |  |
| [List share links of a library](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-repo-id-repo-id)<br>`GET /api/v2.1/share-links/?repo_id={repo_id}/` | 有差异 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-share-links) | 文档里的路径有误；在列表接口上传 `?repo_id=`（和 `?path=`）。 |
| [List share link of a folder (file)](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-repo-id-repo-id-path-path)<br>`GET /api/v2.1/share-links/?repo_id={repo_id}&path={path}` | 有差异 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-share-links) | 文档里的路径有误；把两者作为查询参数传入。 |
| [Delete share link](https://seafile-api.readme.io/reference/delete_api-v2-1-share-links-token)<br>`DELETE /api/v2.1/share-links/{token}/` | 一致 | [本地接口](/zh/reference/api/sharing#delete-api-v2-1-share-links-token) |  |
| [Send share link email](https://seafile-api.readme.io/reference/post_api2-send-share-link)<br>`POST /api2/send-share-link/` | 未实现 | — | 不会发送链接邮件；请自行分享链接。 |
| [List direntry in dir download link](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-token-dirents)<br>`GET /api/v2.1/share-links/{token}/dirents/` | 未实现 | — |  |
| [List user authorizations](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-link-token-user-auth)<br>`GET /api/v2.1/share-links/{link_token}/user-auth/` | 未实现 | — |  |
| [Add user authorization](https://seafile-api.readme.io/reference/post_api-v2-1-share-links-link-token-user-auth)<br>`POST /api/v2.1/share-links/{link_token}/user-auth/` | 未实现 | — |  |
| [Delete user authorization](https://seafile-api.readme.io/reference/delete_api-v2-1-share-links-link-token-user-auth)<br>`DELETE /api/v2.1/share-links/{link_token}/user-auth/` | 未实现 | — |  |
| [List email authorizations](https://seafile-api.readme.io/reference/get_api-v2-1-share-links-link-token-email-auth)<br>`GET /api/v2.1/share-links/{link_token}/email-auth/` | 未实现 | — |  |
| [Add email authorization](https://seafile-api.readme.io/reference/post_api-v2-1-share-links-link-token-email-auth)<br>`POST /api/v2.1/share-links/{link_token}/email-auth/` | 未实现 | — |  |
| [Delete email authorization](https://seafile-api.readme.io/reference/delete_api-v2-1-share-links-link-token-email-auth)<br>`DELETE /api/v2.1/share-links/{link_token}/email-auth/` | 未实现 | — |  |

### Snapshot

没有快照浏览或回退接口。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List items in directory of a snapshot](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-commits-commit-id-dir)<br>`GET /api/v2.1/repos/{repo_id}/commits/{commit_id}/dir/` | 未实现 | — |  |
| [Revert library to a snapshot](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-commits-commit-id-revert)<br>`POST /api/v2.1/repos/{repo_id}/commits/{commit_id}/revert/` | 未实现 | — |  |

### Snapshot label

没有快照标签。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get snapshot label](https://seafile-api.readme.io/reference/get_api-v2-1-revision-tags-tag-names)<br>`GET /api/v2.1/revision-tags/tag-names/` | 未实现 | — |  |
| [Create snapshot label](https://seafile-api.readme.io/reference/post_api-v2-1-revision-tags-tagged-items)<br>`POST /api/v2.1/revision-tags/tagged-items/` | 未实现 | — |  |
| [Update snapshot label](https://seafile-api.readme.io/reference/put_api-v2-1-revision-tags-tagged-items)<br>`PUT /api/v2.1/revision-tags/tagged-items/` | 未实现 | — |  |
| [Delete snapshot label](https://seafile-api.readme.io/reference/delete_api-v2-1-revision-tags-tagged-items)<br>`DELETE /api/v2.1/revision-tags/tagged-items/` | 未实现 | — |  |

### Starred items

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List starred items](https://seafile-api.readme.io/reference/get_api-v2-1-starred-items)<br>`GET /api/v2.1/starred-items/` | 一致 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-starred-items) |  |
| [Star a library/folder/file](https://seafile-api.readme.io/reference/post_api-v2-1-starred-items)<br>`POST /api/v2.1/starred-items/` | 一致 | [本地接口](/zh/reference/api/sharing#post-api-v2-1-starred-items) |  |
| [Unstar a library/folder/file](https://seafile-api.readme.io/reference/delete_api-v2-1-starred-items)<br>`DELETE /api/v2.1/starred-items/` | 一致 | [本地接口](/zh/reference/api/sharing#delete-api-v2-1-starred-items) |  |

### Sub folder permissions

没有目录级权限；每个成员使用资料库权限。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get user folder permission](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-user-folder-perm)<br>`GET /api2/repos/{repo_id}/user-folder-perm/` | 未实现 | — |  |
| [Set user folder permission](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-user-folder-perm)<br>`POST /api2/repos/{repo_id}/user-folder-perm/` | 未实现 | — |  |
| [Modify user folder permission](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-user-folder-perm)<br>`PUT /api2/repos/{repo_id}/user-folder-perm/` | 未实现 | — |  |
| [Delete user folder permission](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-user-folder-perm)<br>`DELETE /api2/repos/{repo_id}/user-folder-perm/` | 未实现 | — |  |
| [Get group folder permission](https://seafile-api.readme.io/reference/get_api2-repos-repo-id-group-folder-perm)<br>`GET /api2/repos/{repo_id}/group-folder-perm/` | 未实现 | — |  |
| [Set group folder permission](https://seafile-api.readme.io/reference/post_api2-repos-repo-id-group-folder-perm)<br>`POST /api2/repos/{repo_id}/group-folder-perm/` | 未实现 | — |  |
| [Modify group folder permission](https://seafile-api.readme.io/reference/put_api2-repos-repo-id-group-folder-perm)<br>`PUT /api2/repos/{repo_id}/group-folder-perm/` | 未实现 | — |  |
| [Delete group folder permission](https://seafile-api.readme.io/reference/delete_api2-repos-repo-id-group-folder-perm)<br>`DELETE /api2/repos/{repo_id}/group-folder-perm/` | 未实现 | — |  |

### Upload links

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List upload links](https://seafile-api.readme.io/reference/get_api-v2-1-upload-links)<br>`GET /api/v2.1/upload-links/` | 有差异 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-upload-links) | 条目缺少 `repo_name`、`obj_name`、`ctime`、`is_expired`；改用 `expire_at`/`has_password`。 |
| [Create upload link](https://seafile-api.readme.io/reference/post_api-v2-1-upload-links)<br>`POST /api/v2.1/upload-links/` | 有差异 | [本地接口](/zh/reference/api/sharing#post-api-v2-1-upload-links) | 即使设置了过期时间，`expire_date` 与 `description` 也总是 null。 |
| [Delete upload links](https://seafile-api.readme.io/reference/delete_api-v2-1-upload-links-token)<br>`DELETE /api/v2.1/upload-links/{token}/` | 有差异 | [本地接口](/zh/reference/api/sharing#delete-api-v2-1-upload-links-token) | 返回裸值 `true`（Android 客户端的形状），不是 `{success}`。 |
| [Send upload link email](https://seafile-api.readme.io/reference/post_api2-send-upload-link)<br>`POST /api2/send-upload-link/` | 未实现 | — | 不会发送链接邮件；请自行分享链接。 |
| [Get shared upload link](https://seafile-api.readme.io/reference/get_api-v2-1-upload-links-token-upload)<br>`GET /api/v2.1/upload-links/{token}/upload/` | 一致 | [本地接口](/zh/reference/api/sharing#get-api-v2-1-upload-links-token-upload) |  |

### User profile

只有 `/api2/account/info/`，没有 v2.1 资料接口。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get user profile](https://seafile-api.readme.io/reference/get_api-v2-1-user)<br>`GET /api/v2.1/user/` | 未实现 | — |  |
| [Update user profile](https://seafile-api.readme.io/reference/put_api-v2-1-user)<br>`PUT /api/v2.1/user/` | 未实现 | — |  |

### User search

没有用户目录搜索。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Search user](https://seafile-api.readme.io/reference/get_api2-search-user)<br>`GET /api2/search-user/` | 未实现 | — |  |

### Wiki

Nanofile 没有 Wiki。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List wikis](https://seafile-api.readme.io/reference/get_api-v2-1-wikis2)<br>`GET /api/v2.1/wikis2/` | 未实现 | — |  |
| [Add wiki](https://seafile-api.readme.io/reference/post_api-v2-1-wikis2)<br>`POST /api/v2.1/wikis2/` | 未实现 | — |  |
| [Delete wiki](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/` | 未实现 | — |  |
| [Update wiki](https://seafile-api.readme.io/reference/put_api-v2-1-wiki2-wiki-id)<br>`PUT /api/v2.1/wiki2/{wiki_id}/` | 未实现 | — |  |
| [Get wiki config](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-config)<br>`GET /api/v2.1/wiki2/{wiki_id}/config/` | 未实现 | — |  |
| [Update wiki config](https://seafile-api.readme.io/reference/put_api-v2-1-wiki2-wiki-id-config)<br>`PUT /api/v2.1/wiki2/{wiki_id}/config/` | 未实现 | — |  |
| [Create wiki page](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-wiki-id-pages)<br>`POST /api/v2.1/wiki2/{wiki_id}/pages/` | 未实现 | — |  |
| [Delete wiki page](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id-page-page-id)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/page/{page_id}/` | 未实现 | — |  |
| [Get wiki page](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-page-page-id)<br>`GET /api/v2.1/wiki2/{wiki_id}/page/{page_id}/` | 未实现 | — |  |
| [Duplicate wiki page](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-8-duplicate-page)<br>`POST /api/v2.1/wiki2/8/duplicate-page/` | 未实现 | — |  |
| [Get publish wiki link](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-publish)<br>`GET /api/v2.1/wiki2/{wiki_id}/publish/` | 未实现 | — |  |
| [Publish wiki](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-wiki-id-publish)<br>`POST /api/v2.1/wiki2/{wiki_id}/publish/` | 未实现 | — |  |
| [Delete published wiki link](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id-publish)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/publish/` | 未实现 | — |  |
| [Get publish wiki config](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-publish-config)<br>`GET /api/v2.1/wiki2/{wiki_id}/publish/config/` | 未实现 | — |  |
| [Get publish wiki page](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-publish-page-page-id)<br>`GET /api/v2.1/wiki2/{wiki_id}/publish/page/{page_id}/` | 未实现 | — |  |
| [Search wiki](https://seafile-api.readme.io/reference/post_api-v2-1-wiki2-search)<br>`POST /api/v2.1/wiki2/search/` | 未实现 | — |  |
| [Convert wiki](https://seafile-api.readme.io/reference/post_api-v2-1-convert-wiki)<br>`POST /api/v2.1/convert-wiki/` | 未实现 | — |  |
| [Get wiki page trash](https://seafile-api.readme.io/reference/get_api-v2-1-wiki2-wiki-id-trash)<br>`GET /api/v2.1/wiki2/{wiki_id}/trash/` | 未实现 | — |  |
| [Revert page](https://seafile-api.readme.io/reference/put_api-v2-1-wiki2-wiki-id-trash)<br>`PUT /api/v2.1/wiki2/{wiki_id}/trash/` | 未实现 | — |  |
| [Clean wiki page trash](https://seafile-api.readme.io/reference/delete_api-v2-1-wiki2-wiki-id-trash)<br>`DELETE /api/v2.1/wiki2/{wiki_id}/trash/` | 未实现 | — |  |

## Account Operations - System Admin

### Accounts

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List all users' info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users)<br>`GET /api/v2.1/admin/users/` | 有差异 | [本地接口](/zh/reference/api/accounts#get-api2-admin-users) | Nanofile 自己的管理接口（`/api2/admin/users/`）返回不同的结构。 |
| [Add user](https://seafile-api.readme.io/reference/post_api-v2-1-admin-users)<br>`POST /api/v2.1/admin/users/` | 有差异 | [本地接口](/zh/reference/api/accounts#post-api2-admin-users) | 使用 `/api2/admin/users/`（JSON `email`/`password`/`is_admin`/`is_active`/`storage_quota`）。 |
| [Search user](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-user)<br>`GET /api/v2.1/admin/search-user/` | 未实现 | — |  |
| [Get a user's info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users-user-id)<br>`GET /api/v2.1/admin/users/{user_id}/` | 未实现 | — | 没有单用户 GET；列出 `/api2/admin/users/` 后过滤。 |
| [Update user info](https://seafile-api.readme.io/reference/put_api-v2-1-admin-users-user-id)<br>`PUT /api/v2.1/admin/users/{user_id}/` | 有差异 | [本地接口](/zh/reference/api/accounts#put-api2-admin-users-user-id) | 使用 `/api2/admin/users/{id}/`；id 是数字。 |
| [Delete user](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-users-user-id)<br>`DELETE /api/v2.1/admin/users/{user_id}/` | 有差异 | [本地接口](/zh/reference/api/accounts#delete-api2-admin-users-user-id) | 使用 `/api2/admin/users/{id}/`；id 是数字。 |

### Departments

没有部门/通讯录。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List top level departments](https://seafile-api.readme.io/reference/get_api-v2-1-admin-address-book-groups)<br>`GET /api/v2.1/admin/address-book/groups/` | 未实现 | — |  |
| [Add a department](https://seafile-api.readme.io/reference/post_api-v2-1-admin-address-book-groups)<br>`POST /api/v2.1/admin/address-book/groups/` | 未实现 | — |  |
| [List groups and members in a department](https://seafile-api.readme.io/reference/get_api-v2-1-admin-address-book-groups-group-id)<br>`GET /api/v2.1/admin/address-book/groups/{group_id}/` | 未实现 | — |  |
| [Delete a department](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-address-book-groups-group-id)<br>`DELETE /api/v2.1/admin/address-book/groups/{group_id}/` | 未实现 | — |  |

### Department libraries

没有群组或群组-owned 资料库。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Add group owned library](https://seafile-api.readme.io/reference/post_api-v2-1-admin-groups-group-id-group-owned-libraries)<br>`POST /api/v2.1/admin/groups/{group_id}/group-owned-libraries/` | 未实现 | — |  |
| [Delete group owned library](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id-group-owned-libraries-repo-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/group-owned-libraries/{repo_id}/` | 未实现 | — |  |

### Groups

没有群组。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get all groups](https://seafile-api.readme.io/reference/get_api-v2-1-admin-groups)<br>`GET /api/v2.1/admin/groups/` | 未实现 | — |  |
| [Create a new group](https://seafile-api.readme.io/reference/post_api-v2-1-admin-groups)<br>`POST /api/v2.1/admin/groups/` | 未实现 | — |  |
| [Delete a group](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/` | 未实现 | — |  |
| [Transfer a group/Set department quota](https://seafile-api.readme.io/reference/put_api-v2-1-admin-groups-group-id)<br>`PUT /api/v2.1/admin/groups/{group_id}/` | 未实现 | — |  |
| [Search group](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-group)<br>`GET /api/v2.1/admin/search-group/` | 未实现 | — |  |
| [List group members](https://seafile-api.readme.io/reference/get_api-v2-1-admin-groups-group-id-members)<br>`GET /api/v2.1/admin/groups/{group_id}/members/` | 未实现 | — |  |
| [Add group member](https://seafile-api.readme.io/reference/post_api-v2-1-admin-groups-group-id-members)<br>`POST /api/v2.1/admin/groups/{group_id}/members/` | 未实现 | — |  |
| [Delete group member](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id-members-user-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/members/{user_id}/` | 未实现 | — |  |
| [Set(Unset) a group member as admin](https://seafile-api.readme.io/reference/put_api-v2-1-admin-groups-group-id-members-user-id)<br>`PUT /api/v2.1/admin/groups/{group_id}/members/{user_id}/` | 未实现 | — |  |
| [List libraries shared to a group](https://seafile-api.readme.io/reference/get_api-v2-1-admin-groups-group-id-libraries)<br>`GET /api/v2.1/admin/groups/{group_id}/libraries/` | 未实现 | — |  |
| [Remove a library from group](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-groups-group-id-libraries-repo-id)<br>`DELETE /api/v2.1/admin/groups/{group_id}/libraries/{repo_id}/` | 未实现 | — |  |

### Libraries

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get all libraries/Search library by owner](https://seafile-api.readme.io/reference/get_api-v2-1-admin-libraries)<br>`GET /api/v2.1/admin/libraries/` | 未实现 | — |  |
| [Create library](https://seafile-api.readme.io/reference/post_api-v2-1-admin-libraries)<br>`POST /api/v2.1/admin/libraries/` | 未实现 | — |  |
| [Search library by name](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-library)<br>`GET /api/v2.1/admin/search-library/` | 未实现 | — |  |
| [Get a library info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-libraries-repo-id)<br>`GET /api/v2.1/admin/libraries/{repo_id}/` | 未实现 | — |  |
| [Delete a library](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-libraries-repo-id)<br>`DELETE /api/v2.1/admin/libraries/{repo_id}/` | 未实现 | — |  |
| [Update a library status/Transfer a library](https://seafile-api.readme.io/reference/put_api-v2-1-admin-libraries-repo-id)<br>`PUT /api/v2.1/admin/libraries/{repo_id}/` | 未实现 | — |  |
| [Get all deleted libraries/Get deleted libraries by owner](https://seafile-api.readme.io/reference/get_api-v2-1-admin-trash-libraries)<br>`GET /api/v2.1/admin/trash-libraries/` | 未实现 | — |  |
| [Clean all deleted libraries](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-trash-libraries)<br>`DELETE /api/v2.1/admin/trash-libraries/` | 未实现 | — |  |
| [Clean deleted library](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-trash-libraries-repo-id)<br>`DELETE /api/v2.1/admin/trash-libraries/{repo_id}/` | 未实现 | — |  |
| [Restore deleted library](https://seafile-api.readme.io/reference/put_api-v2-1-admin-trash-libraries-repo-id)<br>`PUT /api/v2.1/admin/trash-libraries/{repo_id}/` | 未实现 | — |  |
| [Get library history setting](https://seafile-api.readme.io/reference/get_api-v2-1-admin-libraries-repo-id-history-limit)<br>`GET /api/v2.1/admin/libraries/{repo_id}/history-limit/` | 未实现 | — |  |
| [Update library history setting](https://seafile-api.readme.io/reference/put_api-v2-1-admin-libraries-repo-id-history-limit)<br>`PUT /api/v2.1/admin/libraries/{repo_id}/history-limit/` | 未实现 | — |  |
| [Create library folder](https://seafile-api.readme.io/reference/post_api-v2-1-admin-libraries-repo-id-dirents)<br>`POST /api/v2.1/admin/libraries/{repo_id}/dirents/` | 未实现 | — |  |

### Logs

没有管理日志接口；日志是磁盘文件。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get login log](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-login)<br>`GET /api/v2.1/admin/logs/login/` | 未实现 | — |  |
| [Get file access log](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-file-audit)<br>`GET /api/v2.1/admin/logs/file-audit/` | 未实现 | — |  |
| [Get file update logs](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-file-update)<br>`GET /api/v2.1/admin/logs/file-update/` | 未实现 | — |  |
| [Get permission audit logs](https://seafile-api.readme.io/reference/get_api-v2-1-admin-logs-perm-audit)<br>`GET /api/v2.1/admin/logs/perm-audit/` | 未实现 | — |  |

### Notifications

没有管理端通知流。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get notifications](https://seafile-api.readme.io/reference/get_api-v2-1-admin-notifications)<br>`GET /api/v2.1/admin/notifications/` | 未实现 | — |  |

### Organizations

没有组织。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get organizations](https://seafile-api.readme.io/reference/get_api-v2-1-admin-organizations)<br>`GET /api/v2.1/admin/organizations/` | 未实现 | — |  |
| [Add organizations](https://seafile-api.readme.io/reference/post_api-v2-1-admin-organizations)<br>`POST /api/v2.1/admin/organizations/` | 未实现 | — |  |
| [Search organizations](https://seafile-api.readme.io/reference/get_api-v2-1-admin-search-organization)<br>`GET /api/v2.1/admin/search-organization/` | 未实现 | — |  |
| [Get organization info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-organizations-org-id)<br>`GET /api/v2.1/admin/organizations/{org_id}/` | 未实现 | — |  |
| [Update organization info](https://seafile-api.readme.io/reference/put_api-v2-1-admin-organizations-org-id)<br>`PUT /api/v2.1/admin/organizations/{org_id}/` | 未实现 | — |  |
| [Delete organization](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-organizations-org-id)<br>`DELETE /api/v2.1/admin/organizations/{org_id}/` | 未实现 | — |  |

### Share

没有用户/群组分享。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get repo user/group shares](https://seafile-api.readme.io/reference/get_api-v2-1-admin-shares)<br>`GET /api/v2.1/admin/shares/` | 未实现 | — |  |
| [Share repo to user/group](https://seafile-api.readme.io/reference/post_api-v2-1-admin-shares)<br>`POST /api/v2.1/admin/shares/` | 未实现 | — |  |
| [Modify repo user/group share permission](https://seafile-api.readme.io/reference/put_api-v2-1-admin-shares)<br>`PUT /api/v2.1/admin/shares/` | 未实现 | — |  |
| [Delete repo user/group share](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-shares)<br>`DELETE /api/v2.1/admin/shares/` | 未实现 | — |  |
| [List shared folders](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users-user-id-shared-folders)<br>`GET /api/v2.1/admin/users/{user_id}/shared-folders/` | 未实现 | — |  |
| [List shared repos](https://seafile-api.readme.io/reference/get_api-v2-1-admin-users-user-id-shared-repos)<br>`GET /api/v2.1/admin/users/{user_id}/shared-repos/` | 未实现 | — |  |

### Share links

没有全站分享链接管理接口。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get all share links](https://seafile-api.readme.io/reference/get_api-v2-1-admin-share-links)<br>`GET /api/v2.1/admin/share-links/` | 未实现 | — |  |
| [Get shared file/dir info](https://seafile-api.readme.io/reference/get_api-v2-1-admin-share-links-token)<br>`GET /api/v2.1/admin/share-links/{token}/` | 未实现 | — |  |
| [Delete share link](https://seafile-api.readme.io/reference/delete_api-v2-1-admin-share-links-token)<br>`DELETE /api/v2.1/admin/share-links/{token}/` | 未实现 | — |  |
| [List items in folder shared links](https://seafile-api.readme.io/reference/get_api-v2-1-admin-share-links-token-dirents)<br>`GET /api/v2.1/admin/share-links/{token}/dirents/` | 未实现 | — |  |

## Metadata Operations - User

### Metadata config

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get metadata enabled status](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata)<br>`GET /api/v2.1/repos/{repo_id}/metadata/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata) | 返回 `face_recognition_enabled`/`summary_enabled`；没有 `ocr_enabled` 与 `tags_lang`。 |
| [Enabled metadata](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/` | 一致 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata) |  |
| [Disable metadata](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/` | 有差异 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata) | 用 `PUT {"enabled": false}` 关闭元数据。 |
| [Details settings](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-details-settings)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/details-settings/` | 未实现 | — | `details_settings` 永远是 `"{}"`。 |
| [Extract file details](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-extract-file-details)<br>`POST /api/v2.1/repos/{repo_id}/metadata/extract-file-details/` | 未实现 | — |  |

### Metadata records

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List metadata records](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-records)<br>`GET /api/v2.1/repos/{repo_id}/metadata/records/` | 未实现 | — | 只能读取单条记录（`/metadata/record/`）。 |
| [Update metadata records](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-records)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/records/` | 未实现 | — | 通过 `/metadata/record/` 逐条更新。 |
| [Get metadata record](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-record)<br>`GET /api/v2.1/repos/{repo_id}/metadata/record/` | 有差异 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-record) | 直接返回记录；不产出官方的 `{metadata, results}` 列结构。 |
| [Update metadata record](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-record)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/record/` | 有差异 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-record) | 用 `record_id` 定位文件；不读取官方的 `parent_dir`/`file_name` 请求体。 |
| [Add column](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-columns)<br>`POST /api/v2.1/repos/{repo_id}/metadata/columns/` | 未实现 | — | 没有列模型；记录保存自由格式 JSON。 |

### Metadata views

没有实现视图。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [List views](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-views)<br>`GET /api/v2.1/repos/{repo_id}/metadata/views` | 未实现 | — |  |
| [Add view](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-views)<br>`POST /api/v2.1/repos/{repo_id}/metadata/views` | 未实现 | — |  |
| [Update view](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-views)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/views` | 未实现 | — |  |
| [Delete view](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-views)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/views` | 未实现 | — |  |
| [Get a view](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-views-view-id)<br>`GET /api/v2.1/repos/{repo_id}/metadata/views/{view_id}/` | 未实现 | — |  |
| [Move view](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-move-views)<br>`POST /api/v2.1/repos/{repo_id}/metadata/move-views/` | 未实现 | — |  |
| [Duplicate view](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-duplicate-view)<br>`POST /api/v2.1/repos/{repo_id}/metadata/duplicate-view/` | 未实现 | — |  |

### Tags

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Turn on tags feature](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-tags-status)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/tags-status/` | 一致 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-tags-status) |  |
| [Turn off tags feature](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-tags-status)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/tags-status/` | 一致 | [本地接口](/zh/reference/api/metadata#delete-api-v2-1-repos-repo-id-metadata-tags-status) |  |
| [List tags](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-tags)<br>`GET /api/v2.1/repos/{repo_id}/metadata/tags/` | 一致 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Add tags](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-tags)<br>`POST /api/v2.1/repos/{repo_id}/metadata/tags/` | 一致 | [本地接口](/zh/reference/api/metadata#post-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Update tags](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-tags)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/tags/` | 一致 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Delete tags](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-tags)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/tags/` | 一致 | [本地接口](/zh/reference/api/metadata#delete-api-v2-1-repos-repo-id-metadata-tags) |  |
| [Update file tags](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-file-tags)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/file-tags` | 有差异 | [本地接口](/zh/reference/api/metadata#put-api-v2-1-repos-repo-id-metadata-file-tags) | 返回 `{success: true}`；不产出官方的 `{success, fail}` id 列表。 |
| [List tag files](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-tag-files-tag-id)<br>`GET /api/v2.1/repos/{repo_id}/metadata/tag-files/{tag_id}/` | 一致 | [本地接口](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tag-files-tag-id) |  |
| [List tags files](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-tags-files)<br>`POST /api/v2.1/repos/{repo_id}/metadata/tags-files/` | 未实现 | — |  |
| [Add tags links](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-tags-links)<br>`POST /api/v2.1/repos/{repo_id}/metadata/tags-links/` | 未实现 | — |  |
| [Update tags links](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-tags-links)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/tags-links/` | 未实现 | — |  |
| [Delete tags links](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-tags-links)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/tags-links/` | 未实现 | — |  |
| [Merge tags](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-merge-tags)<br>`POST /api/v2.1/repos/{repo_id}/metadata/merge-tags/` | 未实现 | — |  |

### OCR

没有实现 OCR；该功能报告为关闭。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Turn on OCR feature](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-ocr)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/ocr/` | 未实现 | — |  |
| [Turn off OCR feature](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-ocr)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/ocr/` | 未实现 | — |  |

### Face recognition

没有人脸识别；该功能报告为关闭。

| 官方操作 | 状态 | Nanofile | 说明 |
| --- | --- | --- | --- |
| [Get face recognition enabled status](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-face-recognition)<br>`GET /api/v2.1/repos/{repo_id}/metadata/face-recognition/` | 未实现 | — |  |
| [Open face recognition](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-face-recognition)<br>`POST /api/v2.1/repos/{repo_id}/metadata/face-recognition/` | 未实现 | — |  |
| [List face records](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-face-records)<br>`GET /api/v2.1/repos/{repo_id}/metadata/face-records/` | 未实现 | — |  |
| [List people photos](https://seafile-api.readme.io/reference/get_api-v2-1-repos-repo-id-metadata-people-photos-people-id)<br>`GET /api/v2.1/repos/{repo_id}/metadata/people-photos/{people_id}/` | 未实现 | — |  |
| [Remove people photos](https://seafile-api.readme.io/reference/delete_api-v2-1-repos-repo-id-metadata-people-photos-people-id)<br>`DELETE /api/v2.1/repos/{repo_id}/metadata/people-photos/{people_id}/` | 未实现 | — |  |
| [Add people photos](https://seafile-api.readme.io/reference/post_api-v2-1-repos-repo-id-metadata-people-photos-people-id)<br>`POST /api/v2.1/repos/{repo_id}/metadata/people-photos/{people_id}/` | 未实现 | — |  |
| [Update face name](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-face-record)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/face-record/` | 未实现 | — |  |
| [Update people cover photo](https://seafile-api.readme.io/reference/put_api-v2-1-repos-repo-id-metadata-people-cover-photo-people-id)<br>`PUT /api/v2.1/repos/{repo_id}/metadata/people-cover-photo/{people_id}/` | 未实现 | — |  |

## 这里有、官方参考没有的接口 {#undocumented}

这些路由已经提供，但不在官方参考的操作里。多数是 seahub 有、参考没写的接口（v2.1 变体、旧版传输辅助接口、trash2、related-users）；标 \* 的是 Nanofile 独有。`/seafhttp/`、`/dav/`、`/notification`，以及浏览器传输接口（`/upload-aj/`、`/download-api/`、`/zip/`、进度桩）各有一页，不在这里重复。

| 接口 | 文档位置 |
| --- | --- |
| `GET /api2/auth/ping/` | [docs](/zh/reference/api/authentication#get-api2-auth-ping) |
| `POST /api2/logout-device/` | [docs](/zh/reference/api/authentication#post-api2-logout-device) |
| `GET /api2/ping/` | [docs](/zh/reference/api/accounts#get-api2-ping) |
| `POST /api2/device-wiped/` | [docs](/zh/reference/api/authentication#post-api2-device-wiped) |
| `POST /api2/client-sso-link/` | [docs](/zh/reference/api/authentication#post-api2-client-sso-link) |
| `GET /api2/client-sso-link/{token}/` | [docs](/zh/reference/api/authentication#get-api2-client-sso-link-token) |
| `PUT /api2/account/info/` | [docs](/zh/reference/api/accounts#put-api2-account-info) |
| `POST /api2/account/` | [docs](/zh/reference/api/accounts#post-api2-account) |
| `POST /api2/accounts/` | [docs](/zh/reference/api/accounts#post-api2-accounts) |
| `GET /api2/credentials/` \* | [docs](/zh/reference/api/api-keys#get-api2-credentials) |
| `DELETE /api2/credentials/{kind}/{id}/` \* | [docs](/zh/reference/api/api-keys#delete-api2-credentials-kind-id) |
| `GET /api2/unseen_messages/` | [docs](/zh/reference/api/accounts#get-api2-unseen-messages) |
| `GET /api/v2.1/repos/` | [docs](/zh/reference/api/libraries#get-api-v2-1-repos) |
| `GET /api/v2.1/repos/{repo_id}/` | [docs](/zh/reference/api/libraries#get-api-v2-1-repos-repo-id) |
| `DELETE /api/v2.1/repos/{repo_id}/` | [docs](/zh/reference/api/libraries#delete-api-v2-1-repos-repo-id) |
| `GET /api2/repos/{repo_id}/dir/sub_repo/` | [docs](/zh/reference/api/directories#get-api2-repos-repo-id-dir-sub-repo) |
| `POST /api/v2.1/repos/{repo_id}/set-password/` | [docs](/zh/reference/api/libraries#post-api-v2-1-repos-repo-id-set-password) |
| `PUT /api/v2.1/repos/{repo_id}/set-password/` | [docs](/zh/reference/api/libraries#put-api-v2-1-repos-repo-id-set-password) |
| `GET /api/v2.1/deleted-repos/` | [docs](/zh/reference/api/history#get-api-v2-1-deleted-repos) |
| `POST /api/v2.1/deleted-repos/` | [docs](/zh/reference/api/history#post-api-v2-1-deleted-repos) |
| `DELETE /api/v2.1/deleted-repos/` | [docs](/zh/reference/api/history#delete-api-v2-1-deleted-repos) |
| `DELETE /api/v2.1/deleted-repos/{repo_id}/` | [docs](/zh/reference/api/history#delete-api-v2-1-deleted-repos-repo-id) |
| `GET /api/v2.1/repos/{repo_id}/dir/` | [docs](/zh/reference/api/directories#get-api-v2-1-repos-repo-id-dir) |
| `POST /api/v2.1/repos/{repo_id}/dir/` | [docs](/zh/reference/api/directories#post-api-v2-1-repos-repo-id-dir) |
| `DELETE /api/v2.1/repos/{repo_id}/dir/` | [docs](/zh/reference/api/directories#delete-api-v2-1-repos-repo-id-dir) |
| `POST /api2/repos/{repo_id}/dir/move/` \* | [docs](/zh/reference/api/directories#post-api2-repos-repo-id-dir-move) |
| `POST /api2/repos/{repo_id}/dir/rename/` \* | [docs](/zh/reference/api/directories#post-api2-repos-repo-id-dir-rename) |
| `POST /api2/repos/{repo_id}/file/` | [docs](/zh/reference/api/files#post-api2-repos-repo-id-file) |
| `DELETE /api2/repos/{repo_id}/file/` | [docs](/zh/reference/api/files#delete-api2-repos-repo-id-file) |
| `POST /api2/repos/{repo_id}/file/rename/` \* | [docs](/zh/reference/api/files#post-api2-repos-repo-id-file-rename) |
| `POST /api2/repos/{repo_id}/file/move/` \* | [docs](/zh/reference/api/files#post-api2-repos-repo-id-file-move) |
| `GET /api2/repos/{repo_id}/file/exif/` \* | [docs](/zh/reference/api/files#get-api2-repos-repo-id-file-exif) |
| `GET /api2/repos/{repo_id}/file/index-text/` | [docs](/zh/reference/api/metadata#get-api2-repos-repo-id-file-index-text) |
| `POST /api2/repos/{repo_id}/file/reindex/` | [docs](/zh/reference/api/metadata#post-api2-repos-repo-id-file-reindex) |
| `GET /api2/repos/{repo_id}/thumbnail/` | [docs](/zh/reference/api/files#get-api2-repos-repo-id-thumbnail) |
| `GET /api2/repos/{repo_id}/file-uploaded-bytes/` | [docs](/zh/reference/api/transfers#get-api2-repos-repo-id-file-uploaded-bytes) |
| `GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/` | [docs](/zh/reference/api/transfers#get-api-v2-1-repos-repo-id-file-uploaded-bytes) |
| `GET /repos/{repo_id}/files/{*path}` | [docs](/zh/reference/api/transfers#get-repos-repo-id-files-path) |
| `GET /api2/repos/{repo_id}/upload-blks-link/` | [docs](/zh/reference/api/libraries#get-api2-repos-repo-id-upload-blks-link) |
| `GET /api2/repos/{repo_id}/update-blks-link/` | [docs](/zh/reference/api/libraries#get-api2-repos-repo-id-update-blks-link) |
| `GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/` | [docs](/zh/reference/api/libraries#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link) |
| `POST /api2/repos/{repo_id}/fileops/delete/` | [docs](/zh/reference/api/files#post-api2-repos-repo-id-fileops-delete) |
| `POST /api2/repos/{repo_id}/fileops/copy/` | [docs](/zh/reference/api/files#post-api2-repos-repo-id-fileops-copy) |
| `POST /api2/repos/{repo_id}/fileops/move/` | [docs](/zh/reference/api/files#post-api2-repos-repo-id-fileops-move) |
| `POST /api/v2.1/copy-move-task/` | [docs](/zh/reference/api/files#post-api-v2-1-copy-move-task) |
| `POST /api/v2.1/repos/{repo_id}/trash/revert-dirents/` | [docs](/zh/reference/api/history#post-api-v2-1-repos-repo-id-trash-revert-dirents) |
| `GET /api/v2.1/repos/{repo_id}/trash2/` | [docs](/zh/reference/api/history#get-api-v2-1-repos-repo-id-trash2) |
| `GET /api/v2.1/repos/{repo_id}/trash2/search/` | [docs](/zh/reference/api/history#get-api-v2-1-repos-repo-id-trash2-search) |
| `POST /api/v2.1/repos/{repo_id}/trash2/revert/` | [docs](/zh/reference/api/history#post-api-v2-1-repos-repo-id-trash2-revert) |
| `GET /api2/repo_history_changes/{repo_id}/` | [docs](/zh/reference/api/history#get-api2-repo-history-changes-repo-id) |
| `POST /api/v2.1/repos/{repo_id}/file/revision/restore/` \* | [docs](/zh/reference/api/history#post-api-v2-1-repos-repo-id-file-revision-restore) |
| `GET /api2/shared-links/` | [docs](/zh/reference/api/sharing#get-api2-shared-links) |
| `POST /api2/shared-links/` | [docs](/zh/reference/api/sharing#post-api2-shared-links) |
| `DELETE /api2/shared-links/{token}` | [docs](/zh/reference/api/sharing#delete-api2-shared-links-token) |
| `GET /api/v2.1/share-links/{token}/` | [docs](/zh/reference/api/sharing#get-api-v2-1-share-links-token) |
| `PUT /api/v2.1/share-links/{token}/` | [docs](/zh/reference/api/sharing#put-api-v2-1-share-links-token) |
| `GET /api2/upload-links/` | [docs](/zh/reference/api/sharing#get-api2-upload-links) |
| `POST /api2/upload-links/` | [docs](/zh/reference/api/sharing#post-api2-upload-links) |
| `DELETE /api2/upload-links/{token}` | [docs](/zh/reference/api/sharing#delete-api2-upload-links-token) |
| `DELETE /api/v2.1/upload-links/clean-invalid/` | [docs](/zh/reference/api/sharing#delete-api-v2-1-upload-links-clean-invalid) |
| `GET /api/v2.1/upload-links/{token}/` | [docs](/zh/reference/api/sharing#get-api-v2-1-upload-links-token) |
| `PUT /api/v2.1/upload-links/{token}/` | [docs](/zh/reference/api/sharing#put-api-v2-1-upload-links-token) |
| `GET /api/v2.1/repos/{repo_id}/upload-links/` | [docs](/zh/reference/api/sharing#get-api-v2-1-repos-repo-id-upload-links) |
| `GET /api/v2.1/groups/` | [docs](/zh/reference/api/sharing#get-api-v2-1-groups) |
| `GET /api/v2.1/repos/{repo_id}/related-users/` | [docs](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-related-users) |
| `GET /api/v2.1/repos/{repo_id}/metadata/tags-status/` | [docs](/zh/reference/api/metadata#get-api-v2-1-repos-repo-id-metadata-tags-status) |
| `GET /api2/starredfiles/` | [docs](/zh/reference/api/sharing#get-api2-starredfiles) |
| `POST /api2/reindex/` | [docs](/zh/reference/api/metadata#post-api2-reindex) |
| `GET /api2/reindex-progress/` | [docs](/zh/reference/api/metadata#get-api2-reindex-progress) |
| `POST /api2/index-file-text/` | [docs](/zh/reference/api/metadata#post-api2-index-file-text) |
| `GET /api2/repos/{repo_id}/download-info/` | [docs](/zh/reference/api/libraries#get-api2-repos-repo-id-download-info) |
| `GET /api2/api-keys/catalog/` \* | [docs](/zh/reference/api/api-keys#get-api2-api-keys-catalog) |
| `POST /api2/api-keys/` \* | [docs](/zh/reference/api/api-keys#post-api2-api-keys) |
| `GET /api2/api-keys/{id}/` \* | [docs](/zh/reference/api/api-keys#get-api2-api-keys-id) |

## 行为差异 {#behaviour}

路由级的表格无法覆盖全部差异。以下差异适用于整个接口面；协议层细节见各接口页与 [Seafile 兼容性](/zh/dev/protocol#刻意保留的偏差)。

- **凭据。** 除了账号令牌，Nanofile 还接受 API 密钥（按能力与资料库受限），`/seafhttp/` 上使用资料库同步令牌。Repo-Token 调用方请用绑定资料库的 API 密钥；不路由 `/via-repo-token/` 镜像。
- **Authorization 头。** 所有 API 路由同时接受 `Bearer` 与旧式 `Token` 前缀，兼容 11 前后两个时代的客户端。
- **错误。** 认证失败同时带 `detail` 与 `error_msg`；其他错误带 `error_msg`。登录失败用 DRF 的 `non_field_errors`；文件被锁时返回纯文本。协议需要的地方使用 Seafile 的 440–446 与 520 状态码。
- **版本字符串。** `/api2/server-info/` 返回 `server.version`（默认 `8.0.0`），客户端据此裁剪功能；本页描述实际存在的路由，与该字符串无关。
- **时间。** 多数是 Unix 秒。`file/detail/` 与 `search-file` 用 RFC 3339；`dir/detail/` 返回数字，而 seahub 返回字符串。
- **列表。** 旧版目录列表隐藏 `_Internal`，目录在前并按小写名称排序，报告真实的 `dir_perm`/`permission`；未实现 `?oid=` 校验。
- **搜索。** `/api/v2.1/search-file/` 最多 200 条；`/api2/search/` 忽略 Pro 专属过滤参数。
- **传输。** 上传/更新链接指向 `/upload-api/` 与 `/update-api/`（没有 `/seafhttp/` 前缀），客户端原样使用返回的 URL。`zip-task` 用 `POST`。
- **WebDAV。** 位于 `/dav/{repo_id}/`（不是 `/seafdav`），Basic 凭据是邮箱加 WebDAV API 密钥，加密资料库返回 `403`，不保存属性也不强制执行锁。
- **通知。** `/notification` 使用 Seafile 通知协议与按订阅签发的 JWT；它不属于 Web API 参考。
- **没有用户间分享。** 没有分享给用户/群组、没有目录级权限、没有评论、Wiki、邀请、快照标签、OCR 与人脸识别；这些官方分组在上表中为 `未实现`。

## WebDAV {#webdav}

WebDAV 不在 Seafile 的 Web API 参考里。Nanofile 的接口面见 [WebDAV](/zh/reference/api/webdav)；刻意保留的差异是认证方式（WebDAV 密钥而不是账号密码）、`/dav/{repo_id}/` 路径、拒绝加密资料库，以及空实现的 `PROPPATCH`/`LOCK`。
