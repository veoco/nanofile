# API 参考

本节记录 Nanofile 提供的全部 HTTP 接口：Seafile 客户端与脚本使用的 REST API、同步协议、WebDAV、通知 WebSocket，以及网页界面调用的传输接口。每页描述请求、响应与错误；[兼容性页](/zh/reference/api/compatibility) 记录每个接口与 Seafile 官方 API 参考的对应关系。

本参考描述的是服务器**实际**返回的行为。某个接口与上游 Seafile 刻意不同时，差异会写在该接口页，并汇总到兼容性页。

## 接口面

| 前缀 | 用途 |
| --- | --- |
| `/api2/` | 旧版 Seafile REST API。官方桌面端与移动端客户端仍大量使用。 |
| `/api/v2.1/` | 较新的 REST API，网页界面与移动端使用。 |
| `/seafhttp/` | 客户端守护进程与服务器之间的同步协议，见 [同步协议](/zh/reference/api/sync)。 |
| `/dav/{repo_id}/` | WebDAV，见 [WebDAV](/zh/reference/api/webdav)。 |
| `/notification` | 客户端订阅资料库更新的 WebSocket，见 [通知 WebSocket](/zh/reference/api/notification)。 |
| `/upload-aj/`、`/upload-api/`、`/update-aj/`、`/update-api/`、`/download-api/`、`/blks/`、`/zip/`、`/repos/{repo_id}/files/` | 传输接口。API 返回这些 URL，网页界面也直接调用；见 [上传与下载](/zh/reference/api/transfers)。 |
| `/health` | 运行状态探针，返回 `200` 与进程代号头。不属于 API。 |

`GET /api2/server-info/` 返回本服务器对外声明的 Seafile 版本（`server.version`，默认 `8.0.0`）。客户端据此裁剪功能；本参考描述实际存在的路由，而不只是该版本号隐含的能力。

## 术语

- **资料库**（`repo`）—— 一棵顶层同步树。id 是 36 位 UUID，在路径里写作 `{repo_id}`。
- **目录项**（dirent）—— 一个目录条目，可能是文件或目录，带名称、大小、修改时间与权限。
- **提交**（commit）—— 资料库的不可变快照，id 是按 [Seafile 兼容性](/zh/dev/protocol#提交-id) 计算出的 40 位 SHA-1。
- **FS 对象** —— 存储的文件或目录对象，id（`oid`、`fs_id`）是其序列化形式的 SHA-1。目录对象是 Seafile version 1。
- **权限** —— 只读 `"r"` 或读写 `"rw"`。Nanofile 按资料库授权（所有者或成员），没有目录级分享。

## 认证 {#authentication}

除标注为公开的接口外，每个接口都要求以下四种凭据之一。

| 凭据 | 发送方式 | 适用范围 |
| --- | --- | --- |
| 账号令牌 | `Authorization: Bearer <token>`；也接受旧式 `Authorization: Token <token>` | 全部 `/api2/` 与 `/api/v2.1/` 接口 |
| 会话 Cookie | 在 `/accounts/login/` 登录后设置的 `seahub-session`；改变状态的请求还需 `X-CSRFToken`，值来自 `sfcsrftoken` Cookie | 同样的 API 接口，也是密钥管理接口唯一接受的凭据 |
| API 密钥 | `Authorization: Bearer <key>` | API 接口，受密钥的能力与资料库范围限制；从不能用于 `/seafhttp/` |
| 同步令牌 | `Seafile-Repo-Token: <token>`（或 `Authorization: Token <token>`） | 仅 `/seafhttp/`，构造上绑定一个资料库 |

账号令牌由 [`POST /api2/auth-token/`](/zh/reference/api/authentication#post-api2-auth-token) 用账号密码换取。API 密钥是 Nanofile 自己的凭据，见 [API 密钥与凭据](/zh/reference/api/api-keys)。

公开接口是 `POST /api2/auth-token/`、`GET /api2/ping/`、`GET /api2/server-info/`、两个 `/api2/client-sso-link/` 接口与 `POST /api2/device-wiped/`。其余接口没有凭据一律 `401`。

资料库只读成员（`r`）只能读；写操作返回 `403`。API 密钥还需要对应能力——每个接口页会写明能力 id。

## 请求

- 请求体是 JSON（`application/json`）、URL 编码表单，或接口页注明的 `multipart/form-data`；同时接受多种编码的接口按 `Content-Type` 分支解析。
- 路径参数写作 `{repo_id}`；查询参数单独列出。路径值必须 URL 编码。
- 文件与目录路径是绝对路径（`/a/b.txt`），通过 `p`（或 `path`）查询参数传递，不作为 URL 段。
- Seafile 在同一路径上重载 `POST`：`POST /api2/repos/{repo_id}/file/` 按表单里的 `operation` 创建、重命名、移动、复制或回退。接口页列出支持的操作。
- 布尔查询参数按 seahub 的规则大小写不敏感地接受 `true`/`false`。

## 响应

成功响应是 JSON，除非接口直接流式返回字节。错误是带 `error_msg` 字段的 JSON：

```json
{ "error_msg": "Library not found." }
```

有两种特殊形状：

- 认证失败返回 `401`，同时带 DRF 的 `detail` 与 Seafile 的 `error_msg`，两种客户端都能读到消息：`{"detail": "…", "error_msg": "…"}`。
- 登录失败使用 DRF 的序列化错误：`{"non_field_errors": ["…"]}`。文件被锁时返回 `403` 与纯文本 `File <path> is locked`，因为同步守护进程用正则解析这段正文。

### 状态码

| 状态码 | 含义 |
| --- | --- |
| `200` | 成功。 |
| `201` | 已创建（建库、建分享链接）。 |
| `204` | 成功但无正文（部分 `DELETE`）。 |
| `400` | 请求格式错误、缺少参数或取值非法。 |
| `401` | 缺少或无效凭据。 |
| `403` | 已认证但无权：只读资料库、缺少能力、账号被禁用。 |
| `404` | 对象不存在或对调用者不可见。 |
| `409` | 冲突，例如目录已存在。 |
| `413` | 请求体超过配置上限。 |
| `429` | 被限流。 |
| `440` | Seafile：需要资料库密码。 |
| `441` | Seafile：需要资料库密码哈希（`magic`），或写入目标不存在。 |
| `442` | Seafile：上传条目超过大小限制。 |
| `443` | Seafile：超过存储配额。 |
| `444` | Seafile：资料库已删除。 |
| `445` | Seafile：资料库已损坏。 |
| `446` | Seafile：上传的文件缺少块。 |
| `500` | 内部错误。 |
| `503` | 暂时不可用（维护或连接数上限）。 |
| `520` | Seafile：操作失败。 |

## ID 与时间

- `repo_id` 是 36 位 UUID。`commit_id`、`file_id`、`fs_id`、`oid`、`block_id` 是 40 位小写 SHA-1 十六进制字符串。
- 多数时间戳是 **Unix 秒**（JSON 数字）。少数较新接口用 RFC 3339 字符串（`file/detail/`、`search-file`），而 `dir/detail/` 返回数字 `mtime`，seahub 返回字符串。每个接口页会说明。

## 限制与策略

- JSON 与表单请求体受 `server.max_json_body_mb` 限制（默认 64 MiB）；上传路由用 `server.max_upload_size_mb`（4096 MiB）。
- 请求超时为 `server.request_timeout_secs`（默认 600 秒）。
- 搜索按用户每分钟限流（`auth.search_max_per_minute`）；登录失败触发按地址的锁定（`auth.max_login_attempts`）。
- 分页默认值与上限见各接口。`search` 默认每页 10 条、最多 100；`search-file` 总共最多 200 条；动态默认 25 条；回收站默认 50 条。
- API 与传输路由按 `server.cors_allowed_origins`（默认站点 URL）应答 CORS。WebDAV 刻意不套 CORS 层。

## 与 Seafile 的关系

每个接口页末尾都有 **Seafile** 说明。完整对照——官方 API 参考中的每个操作、按官方分组排列，以及 Seafile 服务端有但参考没写的接口——见 [与 Seafile API 的兼容性](/zh/reference/api/compatibility)。

- **一致** —— method 与路径模板相同；响应以接口页为准，允许 Nanofile 特有的附加字段。
- **有差异** —— 功能存在，但 method、路径、凭据或响应形状不同；说明写清差异。
- **未实现** —— 没有对应实现；说明给出最接近的替代方式。

## 页面

| 页面 | 内容 |
| --- | --- |
| [认证](/zh/reference/api/authentication) | 令牌、两步验证、客户端登录与 SSO |
| [API 密钥与凭据](/zh/reference/api/api-keys) | API 密钥、能力、会话与设备 |
| [账号与管理](/zh/reference/api/accounts) | 账号信息、头像、用户管理、服务器信息 |
| [资料库](/zh/reference/api/libraries) | 资料库的创建与元数据、链接、密码 |
| [目录](/zh/reference/api/directories) | 目录列表与目录操作 |
| [文件与批量操作](/zh/reference/api/files) | 文件元数据、文件操作、批量复制/移动/删除 |
| [上传与下载](/zh/reference/api/transfers) | 传输接口、断点续传、ZIP 下载 |
| [历史、版本与回收站](/zh/reference/api/history) | 文件历史、版本与回收站 |
| [分享](/zh/reference/api/sharing) | 分享链接、上传链接、收藏、动态与群组 |
| [元数据与搜索](/zh/reference/api/metadata) | 元数据、标签、搜索与索引 |
| [同步协议](/zh/reference/api/sync) | `/seafhttp/` 接口 |
| [通知 WebSocket](/zh/reference/api/notification) | 通知协议 |
| [WebDAV](/zh/reference/api/webdav) | `/dav/` |
| [与 Seafile API 的兼容性](/zh/reference/api/compatibility) | 逐接口对照 |
