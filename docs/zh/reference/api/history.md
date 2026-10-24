# 历史、版本与回收站

文件版本历史、下载或恢复某个版本，以及两套回收站 API（seahub 的游标式 `/trash/` 与分页式 `/trash2/`）。

源码：`server/src/handler/history.rs`、`server/src/handler/trash.rs`、`server/src/service/repo/history.rs`、`server/src/fs/core/trash.rs`；测试：`server/tests/api_v21_test.rs`、`server/tests/repo_trash_test.rs`、`server/tests/trash_restore_test.rs`。

## 一次提交的变更 {#get-api2-repo-history-changes-repo-id}

`GET /api2/repo_history_changes/{repo_id}/?commit_id=<id>`

返回某次提交引入的文件变更，按 seahub 资料库历史视图的分组排列。

**响应 `200`**

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

需要 `history.read`。

**Seafile** —— 这是 seahub 的内部历史接口（`ajax/repo/...`），不是官方参考的 `GET /api/v2.1/repos/{repo_id}/history/`，后者 Nanofile 没有实现。

## 文件历史 {#get-api-v2-1-repos-repo-id-file-history}

`GET /api/v2.1/repos/{repo_id}/file/history/?p=<path>&limit=<n>`

一个文件的版本，最新在前。`limit` 默认 25，上限 100；响应永远只有一页。

**响应 `200`**

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

没有改变文件内容的提交会被跳过。需要 `history.read`。

## 下载某个版本 {#get-api-v2-1-repos-repo-id-file-revision}

`GET /api/v2.1/repos/{repo_id}/file/revision/?p=<path>&commit_id=<id>`

以 `Content-Disposition: attachment` 流式返回该提交中的文件字节。需要 `history.read`。

**Seafile** —— 官方参考把该操作记录在旧版 `/api2/repos/{repo_id}/file/revision/`；Nanofile 提供在 `/api/v2.1/` 下（网页界面使用的路径）。字节与查询参数相同。

## 恢复某个版本 {#post-api-v2-1-repos-repo-id-file-revision-restore}

`POST /api/v2.1/repos/{repo_id}/file/revision/restore/?p=<path>&commit_id=<id>`

把文件指回该版本的内容并提交这次变更，不重写历史。

**响应 `200`** —— `{"success": true}`。需要 `history.write`。

## 列出文件回收站 {#get-api-v2-1-repos-repo-id-trash}

`GET /api/v2.1/repos/{repo_id}/trash/?cursor=<ts>&limit=<n>`

按游标分页列出已删除条目，最新在前。`limit` 默认 50，上限 100。

**响应 `200`**

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

下一页把最后一项的 `deleted_time_ts` 作为 `cursor`。需要 `trash.read`。

## 清空文件回收站 {#delete-api-v2-1-repos-repo-id-trash}

`DELETE /api/v2.1/repos/{repo_id}/trash/`

清空资料库回收站。可选 JSON 请求体 `{"keep_days": <n>}` 保留最近 `n` 天删除的条目。

**响应 `200`** —— `{"success": true}`。需要 `trash.write`。

## 恢复已删除条目 {#post-api-v2-1-repos-repo-id-trash-revert-dirents}

`POST /api/v2.1/repos/{repo_id}/trash/revert-dirents/`

旧版恢复接口。请求体是 URL 编码表单，带 `commit_id` 与可选的冒号分隔 `file_names`（为空表示那次提交的全部条目）。

**响应 `200`**

```json
{ "success": [{ "path": "/old.txt", "is_dir": false }], "failed": [] }
```

需要 `trash.write`。Seafile 发送重复的 `path` 字段；Nanofile 读取一个冒号分隔的 `file_names` 字段。

## 列出文件回收站（分页） {#get-api-v2-1-repos-repo-id-trash2}

`GET /api/v2.1/repos/{repo_id}/trash2/?page=<n>&per_page=<n>`

网页界面使用的分页形状。`page` 默认 1，`per_page` 默认 50（上限 100）。

**响应 `200`**

```json
{ "items": [], "total_count": 0, "can_search": true }
```

条目字段与游标列表相同。需要 `trash.read`。

## 搜索回收站 {#get-api-v2-1-repos-repo-id-trash2-search}

`GET /api/v2.1/repos/{repo_id}/trash2/search/?q=<text>&page=&per_page=&op_users=&time_from=&time_to=&suffixes=`

过滤已删除条目。`q` 匹配名称与路径；`op_users` 是逗号分隔的删除者列表；`time_from`/`time_to` 是 Unix 秒；`suffixes` 按扩展名过滤。

**响应 `200`** —— 同样的 `{items, total_count, can_search}` 结构。需要 `trash.read`。

## 从回收站恢复 {#post-api-v2-1-repos-repo-id-trash2-revert}

`POST /api/v2.1/repos/{repo_id}/trash2/revert/`

恢复指定条目。请求体把提交 id 映射到完整路径：

```json
{ "<commit_id>": ["/old.txt", "/photos"] }
```

**响应 `200`** —— `{"success": [{"path": "/old.txt", "is_dir": false}], "failed": []}`。失败项带 `{"commit_id", "path", "error_msg"}`。需要 `trash.write`。

## 列出已删除资料库 {#get-api-v2-1-deleted-repos}

`GET /api/v2.1/deleted-repos/`

调用者回收站里的资料库。

**响应 `200`**

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

需要 `library.read`。

## 恢复已删除资料库 {#post-api-v2-1-deleted-repos}

`POST /api/v2.1/deleted-repos/`

从回收站恢复资料库。请求体是 URL 编码表单（`repo_id=…`）或 JSON `{"repo_id": "…"}`。

**响应 `200`** —— `{"success": true}`。需要 `library.delete`。

## 清空资料库回收站 {#delete-api-v2-1-deleted-repos}

`DELETE /api/v2.1/deleted-repos/`

永久删除调用者回收站里的所有资料库并回收其块。

**响应 `200`** —— `{"success": true}`。需要 `library.delete`。

## 清除一个已删除资料库 {#delete-api-v2-1-deleted-repos-repo-id}

`DELETE /api/v2.1/deleted-repos/{repo_id}/`

永久删除回收站里的一个资料库。

**响应 `200`** —— `{"success": true}`。需要 `library.delete`。

**Seafile** —— 官方参考用 seahub 的 path/`scan_stat` 形状记录 `GET /api/v2.1/repos/{repo_id}/trash/`。Nanofile 的版本按游标分页，返回的条目集不同（没有 `scan_stat`，但有 `deleted_time_ts`、`repo_id`、`repo_name`）。分页的 `/trash2/` 是 seahub 有、参考没写的接口。资料库级历史（`/api/v2.1/repos/{repo_id}/history/`）与快照相关接口没有实现。
