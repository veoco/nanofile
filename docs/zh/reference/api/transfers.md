# 上传与下载

搬运字节的接口。多数不直接用账号令牌认证：API 先签发短命的**能力 URL**（`/download-api/<token>`、`/upload-api/<token>`、`/blks/<token>/…`），客户端再去取或 POST。生成链接的接口在 [资料库](/zh/reference/api/libraries#get-api2-repos-repo-id-upload-link)。

源码：`server/src/handler/web/upload.rs`、`server/src/handler/web/download.rs`、`server/src/handler/web/zip_download.rs`、`server/src/handler/file.rs`；测试：`server/tests/sync_test.rs`、`server/tests/upload_link_test.rs`。

## 一次传输怎么发生

1. 客户端调用 `GET /api2/repos/{repo_id}/upload-link/?p=/dir`（或 `.../update-link/`），拿到绑定资料库、用户与目录的绝对 URL。
2. 客户端向该 URL POST `multipart/form-data`。链接接口接受相同表单：可选的 `parent_dir`（上传）或 `target_file`（更新）、`relative_path`、`replace`，以及每个文件一个 `file` 段。
3. 下载时，`GET /api2/repos/{repo_id}/file/?p=…` 返回 `/download-api/<token>` URL；取它会流式返回字节，支持 `Range` 与 `If-None-Match`。

令牌在内存里、寿命很短，使用时重新核对当前成员关系，因此吊销权限会让未使用的链接失效。

## 下载文件字节 {#get-repos-repo-id-files-path}

`GET /repos/{repo_id}/files/{path}?dl=<0|1>`

用调用者自己的凭据（账号令牌、会话或 API 密钥）流式返回文件字节。`dl=1` 强制 `Content-Disposition: attachment`；默认内联，供预览使用。

支持 `Range`（单个区间；`416` 带 `Content-Range: bytes */<size>`）与针对文件 FS 对象 id 的 `If-None-Match`。需要 `file.read`。

## 通过链接下载 {#get-download-api-token}

`GET /download-api/{token}`

流式返回签发令牌时指定的文件。`Range` 与 `If-None-Match` 处理同上。每次请求都会重新核对令牌权限；无效或过期令牌返回 `400`。

## 下载单个块 {#get-blks-token-file-id-block-id}

`GET /blks/{token}/{file_id}/{block_id}`

返回文件的一个块，供逐块解密的客户端使用。令牌由 [`GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/`](/zh/reference/api/libraries#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link) 签发，只对该块有效。

## 上传文件 {#post-upload-api-token}

`POST /upload-api/{token}`

把文件上传到令牌绑定的目录。请求体是 `multipart/form-data`：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `file` | 是 | 文件内容；段里的 filename 是条目名。 |
| `parent_dir` | 否 | 覆盖令牌里的目录。 |
| `relative_path` | 否 | 目标目录下的子路径。 |
| `replace` | 否 | `1`/`true` 替换已有文件。 |

**响应 `200`**

```json
[{ "id": "…", "name": "notes.txt", "size": 1204 }]
```

断点续传还在接收分块时，响应是 `[{"name": "notes.txt", "uploaded": true}]`。

**断点续传** —— 发送 `Content-Range: bytes <start>-<end>/<total>` 与 `Content-Length`；每个分块暂存到临时文件，最后一个分块提交。见[已上传字节](#get-api-v2-1-repos-repo-id-file-uploaded-bytes)。

需要写令牌；加密资料库要先缓存库密钥（上传链接请求会做），可分享的上传链接对加密库直接拒绝。

## 从浏览器上传 {#post-upload-aj-token}

`POST /upload-aj/{token}`

浏览器表单目标，multipart 字段与响应同 [`POST /upload-api/{token}`](#post-upload-api-token)。网页界面通过 `from=web` 的上传链接到达这里。

`POST /upload-aj/`

同一 handler 的无令牌变体，用浏览器会话认证，通过 `repo_id` 字段指明资料库。

## 更新文件 {#post-update-api-token}

`POST /update-api/{token}`

替换已有文件内容。multipart 请求体：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `file` | 是 | 新内容。 |
| `target_file` | 是 | 要替换的文件的绝对路径。 |
| `relative_path` | 否 | 文件目录下的子路径。 |
| `replace` | 否 | 为对称保留；更新总是替换。 |

**响应 `200`** —— `[{"id": "…", "name": "notes.txt", "size": 1204}]`。

没有 `target_file` 的请求是无操作，返回 `{"success": true}`，不会创建新文件。配额按大小差值计算，而不是整个新文件。

`POST /update-aj/{token}`

`POST /update-aj/`

`POST /update-api/`

浏览器变体（分别为令牌与会话）以及网页界面使用的会话形式；字段与响应同 `/update-api/{token}`。

## 上传块 {#post-upload-blks-api-token}

`POST /upload-blks-api/{token}`

移动端的块级上传，由上传块或更新块链接签发。

没有 `commitonly` 时，每个 `file` 段是一个块：段里的 filename 是块 id，必须等于 `sha1(内容)`，否则返回 `400`。每个块写入时即计入配额（超限返回 `443`）。

带 `commitonly` 时，请求是表单：`parent_dir`、`file_name`、`blockids`（JSON 数组）、`file_size` 与可选 `replace`；服务器用已上传的块组装文件并返回 `{"id": "<file_fs_id>"}`。

加密资料库会被拒绝：该接口用明文 SHA-1 命名块，在服务端加密下不成立。

## 已上传字节 {#get-api-v2-1-repos-repo-id-file-uploaded-bytes}

`GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/?file_name=<name>&parent_dir=<dir>&blockids=<json>`

断点续传状态：服务器已经持有该文件的多少字节。暂存临时文件的实际偏移优先于已上传块数。

**响应 `200`** —— `{"uploadedBytes": 1048576}`，带 `Accept-Ranges: bytes`。

需要 `file.write`。

### 已上传字节（旧路径） {#get-api2-repos-repo-id-file-uploaded-bytes}

`GET /api2/repos/{repo_id}/file-uploaded-bytes/`

与上面的 v2.1 接口相同，查询参数与响应一致。

## 上传进度 {#get-upload-progress}

`GET /upload_progress`

网页界面轮询进度用的兼容接口。永远返回 `{"progress": 100}`：上传按请求同步完成。

## 索引进度 {#get-idx-progress}

`GET /idx_progress`

索引进度的兼容接口；全文索引开启时永远返回 `{"progress": 100}`。

## 启动 ZIP 下载 {#post-api-v2-1-repos-repo-id-zip-task}

`POST /api/v2.1/repos/{repo_id}/zip-task/`

为选定条目创建压缩包令牌。

**请求体** —— JSON `{"parent_dir": "/", "dirents": ["a", "b"]}`，或 `multipart/form-data`（`parent_dir` 字段 + 重复的 `dirents` 字段，网页界面用这种）。

**响应 `200`** —— `{"zip_token": "…"}`。压缩包从 [`GET /zip/{token}`](#get-zip-token) 取。

请求的名称会去重并限制数量；空列表返回 `400`，选不出文件返回 `404`，超过单包条目/字节上限返回 `429`。需要 `file.read`。

## 下载 ZIP {#get-zip-token}

`GET /zip/{token}`

流式返回压缩包并消耗令牌。使用令牌时会重新核对条目列表与用户权限，因此被吊销的用户无法下载之前生成的包。一个令牌只能下载一次。

**Seafile** —— 传输流程与字段名与 seahub 相同。两处差异：返回的上传/更新 URL 是 `/upload-api/<token>` 与 `/update-api/<token>`，而不是 `/seafhttp/upload-api/<token>`（客户端原样使用返回的 URL）；`POST /api2/repos/{repo_id}/file/` 还接受直接的 multipart 上传，官方参考没有记录。
