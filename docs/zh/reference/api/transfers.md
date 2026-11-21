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

返回文件的一个块，供逐块解密的客户端使用。令牌由 [`GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/`](/zh/reference/api/libraries#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link) 签发，绑定的是资料库与调用者而不是单个块：在有效期内，调用者能读取该库中任意文件的任意块；接口仍会校验所请求的块属于路径里给出的 `file_id`。

## 上传文件 {#post-upload-api-token}

`POST /upload-api/{token}`

把文件上传到 `parent_dir` 指定的目录；省略时用令牌绑定的目录。请求体是 `multipart/form-data`：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `file` | 是 | 文件内容；段里的 filename 是条目名。多文件时重复该段。 |
| `parent_dir` | 否 | 目标目录；默认是令牌的目录（无令牌的表单里是资料库根）。不存在时在资料库根下创建。 |
| `relative_path` | 否 | 目标目录下的子路径；不能以 `/` 开头。`parent_dir` 中出现 `//` 会被拒绝。 |
| `replace` | 否 | `1`/`true` 替换已有文件；该路径是目录时返回 `409`。 |
| `last_modify` | 否 | 单文件的 RFC 3339 mtime。 |
| `last_modify_list` | 否 | 逗号分隔的 RFC 3339 mtime，每个文件一个。 |

`POST /upload-aj/{token}` 返回已提交文件的 JSON 数组。`-api` 形式默认返回纯文本文件 id；多文件时用 tab 分隔多个 id，`ret-json` 会切换为同样的 JSON 数组。

```json
[{ "id": "…", "name": "notes.txt", "size": 1204 }]
```

**断点续传** —— 发送 `Content-Range: bytes <start>-<end>/<total>` 与 `Content-Length`；每个分块暂存到临时文件，最后一个分块提交。分块请求只能带一个文件。`/upload-aj/`、`/upload-aj/{token}`、`/upload-api/{token}`、`/update-aj/`、`/update-aj/{token}`、`/update-api/`、`/update-api/{token}` 都实现了它；`/api2/repos/{repo_id}/file/` 收到 `Content-Range` 仍返回 `400`。见[已上传字节](#get-api-v2-1-repos-repo-id-file-uploaded-bytes)。

需要写令牌；加密资料库要先缓存库密钥（上传链接请求会做），可分享的上传链接对加密库直接拒绝。

## 从浏览器上传 {#post-upload-aj-token}

`POST /upload-aj/{token}`

浏览器表单目标，multipart 字段与响应同 [`POST /upload-api/{token}`](#post-upload-api-token)。网页界面通过 `from=web` 的上传链接到达这里。

`POST /upload-aj/`

同一 handler 的无令牌变体，用浏览器会话认证，通过 `repo_id` 字段指明资料库。没有 minted 令牌就没有绑定目录，所以省略 `parent_dir` 时是资料库根。

## 更新文件 {#post-update-api-token}

`POST /update-api/{token}`

替换已有文件内容。multipart 请求体：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `file` | 是 | 新内容；只能有一个 file 段。 |
| `target_file` | 是 | 已存在文件的绝对路径；目标不存在返回 `441`。 |
| `relative_path` | 否 | 文件目录下的子路径。 |
| `last_modify` | 否 | RFC 3339 mtime。 |
| `head` | 否 | 桌面客户端的乐观基准；提交仍串行发生在当前 HEAD 上。 |

`replace` 为对称保留；更新总是替换。缺少 `file`（或 `target_file`）返回 `400`，且不改变目标。默认成功体是纯文本文件 id；带 `ret-json` 时返回 `[{"id": "…", "name": "…", "size": 1204}]`。

`POST /update-aj/{token}` 与会话形式 `POST /update-aj/` 使用相同字段，返回 JSON 数组。

`POST /update-api/` 是会话版本：用 `p` / `path` 而不是 `target_file` 指定文件，返回 JSON 数组。它属于 Nanofile 浏览器扩展，不在上游 fileserver 接口面内。

`/update-api/{token}`、`/update-aj/{token}`、`/update-aj/` 与 `/update-api/` 也支持断点续传（带一个 file 段的 `Content-Range`）；见[上传文件](#post-upload-api-token)。

`POST /update-aj/{token}`

`POST /update-aj/`

`POST /update-api/`

## 上传块 {#post-upload-blks-api-token}

`POST /upload-blks-api/{token}`

移动端的块级上传，由上传块链接签发。

没有 `commitonly` 时，每个 `file` 段是一个块：段里的 filename 是块 id，必须等于 `sha1(内容)`，否则返回 `400`。每个块写入时即计入配额（超限返回 `443`）。

带 `commitonly` 时，请求是表单：`parent_dir`、`file_name`、`blockids`（JSON 数组）、`file_size` 与可选 `replace`；服务器用已上传的块组装文件。`blockids` 只有在零字节文件时才能为空。成功时默认返回纯文本文件 id；带 `ret-json` 时返回 `[{"id": "…", "name": "…", "size": 0}]`。`replace=1` 时，`file_name` 指向已有目录会返回 `409`：文件不会替换目录。

加密资料库会被拒绝：该接口用明文 SHA-1 命名块，在服务端加密下不成立。

### 原始块 {#post-upload-raw-blks-api-token}

`POST /upload-raw-blks-api/{token}`

移动端块上传流程的原始块阶段，与提交 URL 一起由 `POST /api2/repos/{repo_id}/upload-blks-link/` 签发。每个 `file` 段是一个块：段里的 filename 是块 id，必须等于 `sha1(内容)`。响应为 `{"success": true}`；块列表随后通过 `/upload-blks-api/{token}?commitonly=true` 提交。

## 更新块 {#post-update-blks-api-token}

`POST /update-blks-api/{token}`

对应的更新入口，由更新块链接（`GET /api2/repos/{repo_id}/update-blks-link/`）签发。请求体是提交表单：`target_file`（已存在文件的绝对路径）、`blockids`（JSON 数组）与 `file_size`；`commitonly` 放在查询串里。服务器原地替换该文件。目标不存在返回 `441`（`SEAF_HTTP_RES_NOT_EXISTS`），目标是目录返回 `409`。

块必须已经在存储中（先通过 `/upload-blks-api/{token}` 的块上传阶段写入）。加密资料库会被拒绝，原因同上。

## 已上传字节 {#get-api-v2-1-repos-repo-id-file-uploaded-bytes}

`GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/?file_name=<name>&parent_dir=<dir>&blockids=<json>`

断点续传状态：服务器已经持有该文件的多少字节。暂存临时文件的实际偏移优先于已上传块数。

**响应 `200`** —— `{"uploadedBytes": 1048576}`，带 `Accept-Ranges: bytes`。

需要 `file.write`。

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

**Seafile** —— 传输流程与字段名与 seahub 相同。差异：返回的上传/更新 URL 是 `/upload-api/<token>` 与 `/update-api/<token>`，而不是 `/seafhttp/upload-api/<token>`（客户端原样使用返回的 URL）；`POST /api2/repos/{repo_id}/file/` 还接受直接的 multipart 上传，官方参考没有记录；仅有会话的 `/upload-aj/`、`/update-aj/`、`/update-api/` 是 Nanofile 扩展；省略 `parent_dir` 时回退到令牌绑定的目录，fileserver 则直接拒绝；更新请求里的 `head` 会被接受，但提交串行发生在当前 HEAD 上；`replace` 另外接受 `true`。
