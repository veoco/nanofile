# 元数据与搜索

Nanofile 实现的 Seafile 元数据服务、标签接口，以及两套搜索 API 与索引控制。元数据在上游是 Seafile Pro 功能；这里始终可用，只保存标签与按文件的键值记录。

源码：`server/src/handler/metadata.rs`、`server/src/handler/search.rs`、`server/src/handler/reindex.rs`、`server/src/service/fs/metadata.rs`、`server/src/service/fs/search.rs`；测试：`server/tests/metadata_test.rs`、`server/tests/index_test.rs`、`server/tests/api_v21_test.rs`。

## 获取元数据配置 {#get-api-v2-1-repos-repo-id-metadata}

`GET /api/v2.1/repos/{repo_id}/metadata/`

**响应 `200`**

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

人脸识别、摘要与视图报告为关闭，且无法开启。需要 `metadata.read`。

## 启用或停用元数据 {#put-api-v2-1-repos-repo-id-metadata}

`PUT /api/v2.1/repos/{repo_id}/metadata/`

**请求体**（JSON）—— `{"enabled": true}`（缺席时默认 `true`）。

**响应 `200`** —— `{"success": true}`。需要 `metadata.write`。

官方参考另有 `DELETE` 停用；Nanofile 用同一个 `PUT` 加 `enabled: false`。

## 列出标签 {#get-api-v2-1-repos-repo-id-metadata-tags}

`GET /api/v2.1/repos/{repo_id}/metadata/tags/?start=<n>&limit=<n>`

资料库级标签。`limit` 默认 1000、上限 1000；`start` 是偏移，上限 10,000,000。`limit=0` 返回空。

**响应 `200`**

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

需要 `tag.read`。

## 创建标签 {#post-api-v2-1-repos-repo-id-metadata-tags}

`POST /api/v2.1/repos/{repo_id}/metadata/tags/`

**请求体**（JSON）—— `{"tags_data": [{"_tag_name": "Important", "_tag_color": "#ff0000"}]}`。名称会去重，空名称跳过，颜色会校验。

**响应 `200`** —— `{"tags": [<tag>]}`。需要 `tag.write`。

## 更新标签 {#put-api-v2-1-repos-repo-id-metadata-tags}

`PUT /api/v2.1/repos/{repo_id}/metadata/tags/`

**请求体**（JSON）—— `{"tags_data": [{"tag_id": "1", "tag": {"_tag_name": "…", "_tag_color": "#00ff00"}}]}`。

**响应 `200`** —— `{"success": true}`。需要 `tag.write`。

## 删除标签 {#delete-api-v2-1-repos-repo-id-metadata-tags}

`DELETE /api/v2.1/repos/{repo_id}/metadata/tags/`

**请求体**（JSON）—— `{"tag_ids": ["1", "2"]}`。

**响应 `200`** —— `{"success": true}`。需要 `tag.write`。

## 获取标签功能状态 {#get-api-v2-1-repos-repo-id-metadata-tags-status}

`GET /api/v2.1/repos/{repo_id}/metadata/tags-status/`

**响应 `200`** —— `{"enabled": true, "lang": "en"}`。需要 `tag.read`。

这个读接口是 Nanofile 的补充；官方参考只有写操作。

## 启用标签 {#put-api-v2-1-repos-repo-id-metadata-tags-status}

`PUT /api/v2.1/repos/{repo_id}/metadata/tags-status/`

开启标签功能。**响应 `200`** —— `{"success": true}`。需要 `tag.write`。

## 停用标签 {#delete-api-v2-1-repos-repo-id-metadata-tags-status}

`DELETE /api/v2.1/repos/{repo_id}/metadata/tags-status/`

关闭标签功能。**响应 `200`** —— `{"success": true}`。需要 `tag.write`。

## 列出带某标签的文件 {#get-api-v2-1-repos-repo-id-metadata-tag-files-tag-id}

`GET /api/v2.1/repos/{repo_id}/metadata/tag-files/{tag_id}/?start=<n>&limit=<n>`

带 `tag_id` 的文件与目录，使用元数据记录形状。

**响应 `200`**

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

偏移与上限同标签列表。需要 `tag.read`。

## 设置文件的标签 {#put-api-v2-1-repos-repo-id-metadata-file-tags}

`PUT /api/v2.1/repos/{repo_id}/metadata/file-tags/`

**请求体**（JSON）—— `{"file_tags_data": [{"record_id": "…", "tags": ["1", "2"]}]}`。record id 来自记录接口。

**响应 `200`** —— `{"success": true}`。需要 `tag.write`。

## 获取文件的元数据记录 {#get-api-v2-1-repos-repo-id-metadata-record}

`GET /api/v2.1/repos/{repo_id}/metadata/record/?parent_dir=<dir>&file_name=<name>`

解析文件的元数据记录，移动端据此拿到打标签所需的 record id。

**响应 `200`**

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

保存的自定义字段会合并进对象。`file_name` 也可以写成 `name`。需要 `metadata.read`。

## 更新文件的元数据记录 {#put-api-v2-1-repos-repo-id-metadata-record}

`PUT /api/v2.1/repos/{repo_id}/metadata/record/`

**请求体**（JSON）—— `{"record_id": "…", "data": {"field": "value"}}`。`data` 替换已保存的自定义字段。

**响应 `200`** —— `{"success": true}`。需要 `metadata.write`。

## 相关用户 {#get-api-v2-1-repos-repo-id-related-users}

`GET /api/v2.1/repos/{repo_id}/related-users/`

资料库成员，供「最后修改者」过滤使用。

**响应 `200`**

```json
{
  "user_list": [
    { "email": "user@example.com", "name": "User", "contact_email": "user@example.com", "avatar_url": "https://files.example.com/avatars/user/user@example.com/resized/72/" }
  ]
}
```

需要 `library.read`。

## 搜索资料库中的文件 {#get-api2-search}

`GET /api2/search/?q=<text>&per_page=<n>&page=<n>&search_repo=<id|all>&search_filename_only=<bool>`

桌面端使用的旧版跨库搜索。

| 查询参数 | 含义 |
| --- | --- |
| `q` | 搜索文本。匹配文件名；`search_filename_only` 为真时只匹配文件名。 |
| `per_page` | 每页条数，默认 10，最大 100。 |
| `page` | 页码，默认 1。 |
| `search_repo` | 资料库 id，或 seahub 的范围关键字（`all`、`mine`、`shared`）；空表示全部可见。 |
| `search_filename_only` | 跳过全文索引。 |

**响应 `200`**

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

同时输出 `last_modified` 与 `mtime`，因为桌面端读前者、iOS 读后者。需要 `search.read`，并按用户每分钟限流（`auth.search_max_per_minute`）。

**Seafile** —— 路由与外层结构相同。Pro 专属过滤参数（`search_type`、`obj_type`、`time_from`、`size_from`、`search_path`、`with_permission`）被忽略；全文索引关闭时也可用（只匹配文件名）。

## 按名称搜索资料库中的文件 {#get-api-v2-1-search-file}

`GET /api/v2.1/search-file/?repo_id=<id>&q=<text>`

`GET /api/v2.1/search-file`

单个资料库内的文件名搜索，形状是移动端解析的。

**响应 `200`**

```json
{ "data": [{ "path": "/notes.txt", "size": 1204, "mtime": "2026-10-24T09:00:00+00:00", "type": "file" }] }
```

目录在前、文件在后，各自按修改时间倒序。`type` 是 `file` 或 `folder`。需要 `search.read`，且资料库必须在凭据范围内。

路径同时注册了带尾斜杠与不带尾斜杠两种形式，因为 iOS 发前者、Android 发后者。

**Seafile** —— 路由与形状相同，但 Nanofile 把结果限制在共用的 200 条预算内；上游遍历整棵树。

## 更新文件的索引文本 {#post-api2-index-file-text}

`POST /api2/index-file-text/`

直接设置某个文件的全文索引内容，跳过提取。供转换服务使用。

**请求体**（JSON）—— `{"repo_id": "…", "path": "/a.pdf", "text": "…"}`。

**响应 `200`** —— `{"status": "ok"}`。仅资料库所有者或管理员；需要 `search.write`，且全文索引开启。

## 读取文件的索引文本 {#get-api2-repos-repo-id-file-index-text}

`GET /api2/repos/{repo_id}/file/index-text/?p=<path>`

**响应 `200`** —— `{"content": "…"}`；文件未索引时 `content` 为 `null`。需要 `file.read`，且全文索引开启。

## 重建单个文件的索引 {#post-api2-repos-repo-id-file-reindex}

`POST /api2/repos/{repo_id}/file/reindex/`

同步重建一个文件的索引。

**请求体**（JSON）—— `{"p": "/notes.txt"}`。

**响应 `200`** —— `{"status": "ok", "indexed": true}`。仅资料库所有者或管理员；需要 `search.reindex`。

## 重建资料库索引 {#post-api2-reindex}

`POST /api2/reindex/`

启动后台全文重建。

**请求体**（JSON）—— `{"repo_id": "…"}`。

**响应 `200`** —— `{"status": "ok", "task_id": "…"}`。仅所有者或管理员；需要 `search.reindex`。同一资料库同时只有一个重建任务；请求按用户每小时限流（`auth.reindex_max_per_hour`）。

## 重建进度 {#get-api2-reindex-progress}

`GET /api2/reindex-progress/?task_id=<id>`

**响应 `200`**

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

`state` 是 `running`、`completed` 或 `failed`。只有任务创建者或管理员能读；其他人返回 `404`。需要 `search.reindex`。

**Seafile** —— 元数据配置、标签、标签文件、文件标签、记录与 related-users 都是官方记录的上游路由。元数据的视图、列、标签链接、标签合并、OCR 与人脸识别接口没有实现。搜索接口与参考一致；索引控制是 seahub 有、参考没写的接口。
