# 文件与批量操作

文件元数据、单文件操作、批量复制/移动/删除，以及异步任务接口。字节的上传下载见 [上传与下载](/zh/reference/api/transfers)，目录列表见 [目录](/zh/reference/api/directories)。

源码：`server/src/handler/file.rs`、`server/src/handler/fileops.rs`、`server/src/handler/batch.rs`、`server/src/handler/async_batch.rs`、`server/src/handler/thumbnail.rs`、`server/src/handler/exif.rs`、`server/src/service/fs/file.rs`；测试：`server/tests/client_compat_test.rs`、`server/tests/api_v21_test.rs`。

## 下载文件 {#get-api2-repos-repo-id-file}

`GET /api2/repos/{repo_id}/file/?p=<path>&op=<download|view|downloadblks>`

返回有时效的下载 URL，不返回字节（字节来自 [`GET /repos/{repo_id}/files/{path}`](/zh/reference/api/transfers#get-repos-repo-id-files-path)）。

| 查询参数 | 含义 |
| --- | --- |
| `p` | 文件路径。 |
| `op` | `download`（默认）与 `view` 都返回 URL；`downloadblks` 返回块列表。 |

**响应 `200`**

```json
"/files/<token>/<filename>"
```

响应头 `oid` 是文件的 FS 对象 id。`op=downloadblks` 时正文改为：

```json
{ "file_id": "…", "blklist": ["…"], "encrypted": false, "enc_version": 0 }
```

需要读权限与 `file.read`。加密资料库返回加密块，由客户端用库密码解密。

**Seafile** —— 路由与操作相同。

## 创建、上传、重命名或移动文件 {#post-api2-repos-repo-id-file}

`POST /api2/repos/{repo_id}/file/?p=<path>&reloaddir=<true|false>`

Seafile 重载的文件接口。请求体形状决定操作：

- **`multipart/form-data` 带 `file` 字段** —— 直接上传字节（网页界面与 Android）。`parent_dir` 指定目标目录，`replace=1` 覆盖，`operation=rename` 配 `newname` 则重命名 `p`。
- **URL 编码表单** —— 以下几种之一：
  - `operation=create` —— 在 `?p=` 创建空文件。
  - `operation=rename` + `newname` —— 重命名 `?p=`。
  - `operation=move` + `dst_repo` + `dst_dir` —— 把 `?p=` 移到另一个目录（仅同一资料库）。
  - `operation=revert` + `commit_id` —— 把 `?p=` 恢复为该提交里的版本并提交这次变更；文件已删除时会重建。文件被其他账号锁定时返回 `403` 与 `File <path> is locked`。

`reloaddir=true` 时返回受影响目录的列表（移动时是目标目录），而不是 `"success"`。

**响应 `200`** —— `{"success": true}` 或 `"success"`，或 `reloaddir=true` 时的裸列表。

需要写权限与 `file.write`。

## 加锁或解锁文件 {#put-api2-repos-repo-id-file}

`PUT /api2/repos/{repo_id}/file/?p=<path>`

为其他同步客户端加锁或解锁。请求体是 URL 编码表单，`operation=lock` 或 `operation=unlock`。

**响应 `200`** —— `{"success": true}`。需要写权限与 `file.write`。锁是排他的：对他人持有的路径加锁返回 `403` 与纯文本 `File <path> is locked`，解锁他人持有的锁同样返回 `403`。没有锁记录时解锁成功：文件本来就未锁定。同步提交按树 diff 检查锁，创建、编辑、删除、重命名或移动他人持有的路径时以同样的 `403` 纯文本拒绝；不涉及该路径的提交不受影响。REST 与 WebDAV 删除也检查锁，返回 `423` 与 `{"error_msg": "File <path> is locked"}`。上传与覆盖不查锁，后写者胜。锁对应的文件已从资料库消失时会被清除，因此不会阻塞之后同路径的新文件。

## 删除文件 {#delete-api2-repos-repo-id-file}

`DELETE /api2/repos/{repo_id}/file/?p=<path>`

删除文件并记入资料库回收站。

**响应 `200`** —— `{"success": true}`。需要写权限与 `file.delete`。

## 创建文件（v2.1） {#post-api-v2-1-repos-repo-id-file}

`POST /api/v2.1/repos/{repo_id}/file/`

创建空文件。路径来自 JSON 请求体的 `{"p": "/a.txt"}` 或 `?p=`；iOS 客户端在表单体里发 `operation=create`。带 `operation=rename` 与 `newname` 时改为重命名 `p`。

**响应 `200`**

```json
{
  "type": "file",
  "repo_id": "…",
  "parent_dir": "/",
  "obj_name": "a.txt",
  "obj_id": "…",
  "size": 0,
  "mtime": "2026-10-24T09:00:00+00:00",
  "is_locked": false,
  "can_preview": true,
  "can_edit": true
}
```

需要写权限与 `file.write`。

## 删除文件（v2.1） {#delete-api-v2-1-repos-repo-id-file}

`DELETE /api/v2.1/repos/{repo_id}/file/?p=<path>`

与旧路径同一操作与响应。需要 `file.delete`。

## 文件详情 {#get-api2-repos-repo-id-file-detail}

`GET /api2/repos/{repo_id}/file/detail/?p=<path>`

一个文件的元数据，包括最后修改者与收藏状态。

**响应 `200`**

```json
{
  "id": "…",
  "type": "file",
  "name": "notes.txt",
  "permission": "rw",
  "mtime": 1760000000,
  "size": 1204,
  "last_modified": "2026-10-24T09:00:00+00:00",
  "last_modifier_name": "user@example.com",
  "last_modifier_email": "user@example.com",
  "last_modifier_contact_email": "user@example.com",
  "last_modifier_avatar": "https://files.example.com/api/avatars/user/user@example.com/resized/72/",
  "starred": false,
  "comment_total": 0,
  "can_edit": true
}
```

`comment_total` 永远是 `0`（Nanofile 没有评论）。需要 `file.read`。

## EXIF 元数据 {#get-api2-repos-repo-id-file-exif}

`GET /api/repos/{repo_id}/file/exif/?p=<path>`

返回图片的 EXIF 标签对象，没有时返回 `null`（非图片，或沙盒无法解析）。

需要 `file.read`。

**Nanofile** —— 此接口位于 `/api/` 下，不属于 Seafile 的 API。

## 缩略图 {#get-api2-repos-repo-id-thumbnail}

`GET /api2/repos/{repo_id}/thumbnail/?p=<path>&size=<px>`

返回图片缩略图（不透明用 JPEG，带透明用 PNG）。`size` 默认 48，最大 1024。响应带强 `ETag` 与 `Cache-Control: private, max-age=604800`；`If-None-Match` 命中 ETag 时返回 `304`。

需要 `file.read`。加密资料库无法生成缩略图：存储的是密文，尝试会失败而不是泄露内容。

## 删除多个文件 {#post-api2-repos-repo-id-fileops-delete}

`POST /api2/repos/{repo_id}/fileops/delete/?p=<dir>&reloaddir=<bool>`

删除一个目录里的多个条目。请求体是 URL 编码表单，`file_names` 是冒号分隔的名称列表。

**响应 `200`** —— 空正文；`reloaddir=true` 时返回目录列表。空名称列表是无操作，返回 `{}`。

需要写权限与 `file.delete`。

## 复制多个文件 {#post-api2-repos-repo-id-fileops-copy}

`POST /api2/repos/{repo_id}/fileops/copy/?p=<src-dir>&reloaddir=<bool>`

**请求体**（URL 编码表单）—— `file_names`（冒号分隔）、`dst_repo`（必须同一资料库）、`dst_dir`。

**响应 `200`** —— 新条目数组：

```json
[{ "repo_id": "…", "parent_dir": "/target", "obj_name": "a.txt" }]
```

`reloaddir=true` 时返回**源**目录的列表。需要 `file.write`。

## 移动多个文件 {#post-api2-repos-repo-id-fileops-move}

`POST /api2/repos/{repo_id}/fileops/move/?p=<src-dir>&reloaddir=<bool>`

请求与响应同复制，但执行移动。同库内移动是一个提交。需要 `file.write`。

## 批量移动（同步，v2.1） {#post-api-v2-1-repos-sync-batch-move-item}

`POST /api/v2.1/repos/sync-batch-move-item/`

**请求体**（JSON）

```json
{
  "src_repo_id": "…",
  "src_parent_dir": "/a",
  "src_dirents": ["one.txt", "two"],
  "dst_repo_id": "…",
  "dst_parent_dir": "/b"
}
```

`src_repo_id` 必须等于 `dst_repo_id`；跨库移动返回 `400`。**响应 `200`** —— `{"success": true}`。需要 `file.write`。

## 批量复制（同步，v2.1） {#post-api-v2-1-repos-sync-batch-copy-item}

`POST /api/v2.1/repos/sync-batch-copy-item/`

请求体与限制同批量移动。**响应 `200`** —— `{"success": true}`。需要 `file.write`。

## 批量删除 {#post-api-v2-1-repos-batch-delete-item}

`POST /api/v2.1/repos/batch-delete-item/`

`DELETE /api/v2.1/repos/batch-delete-item/`

两种方法都接受，请求体相同。

**请求体**（JSON）—— `{"repo_id": "…", "parent_dir": "/a", "dirents": ["one.txt"]}`。

**响应 `200`** —— `{"success": true}`。需要 `file.delete`。

**Seafile** —— 官方参考记录的是 `DELETE` 形式；Nanofile 两种都接受，因为桌面端用 `DELETE`、网页界面用 `POST`。

## 批量复制（异步，v2.1） {#post-api-v2-1-repos-async-batch-copy-item}

`POST /api/v2.1/repos/async-batch-copy-item/`

请求体同同步复制。启动后台任务。`src_repo_id` 必须等于 `dst_repo_id`：任务只在单个资料库内执行，跨库复制返回 `400`。

**响应 `200`** —— `{"task_id": "…"}`。用 [`query-copy-move-progress`](#get-api-v2-1-query-copy-move-progress) 轮询。需要 `file.write`。

## 批量移动（异步，v2.1） {#post-api-v2-1-repos-async-batch-move-item}

`POST /api/v2.1/repos/async-batch-move-item/`

请求体同同步移动。`src_repo_id` 必须等于 `dst_repo_id`：任务只在单个资料库内执行，跨库移动返回 `400`。

**响应 `200`** —— `{"task_id": "…"}`。需要 `file.write`。

## 复制或移动任务 {#post-api-v2-1-copy-move-task}

`POST /api/v2.1/copy-move-task/`

桌面端的合并入口。

**请求体**（JSON）—— 批量请求体加 `"operation": "copy"`（默认）或 `"move"`。`src_repo_id` 必须等于 `dst_repo_id`：跨库操作返回 `400`。

**响应 `200`** —— `{"task_id": "…"}`。需要 `file.write`。

## 查询复制/移动进度 {#get-api-v2-1-query-copy-move-progress}

`GET /api/v2.1/query-copy-move-progress/?task_id=<id>`

报告一个异步复制/移动任务。只有任务所有者能读；未知、过期或他人的 id 返回 `404`。

**响应 `200`**

```json
{
  "state": "processing",
  "done": false,
  "failed": false,
  "successful": false,
  "description": "",
  "total": 3,
  "done_count": 1,
  "failed_count": 0,
  "cancelable": false
}
```

`state` 是 `processing`、`completed` 或 `failed`。`cancelable` 永远是 `false`：移动是一个提交，中途打断会丢条目，所以没有可以安全取消的东西。

**Seafile** —— 路由相同。官方参考还有用于取消的 `DELETE /api/v2.1/copy-move-task/`；Nanofile 没有实现取消。
