# WebDAV

把资料库挂载成网络驱动器。实现是最小化的 WebDAV 提供者，面向 Windows、macOS、davfs2/gvfs 与移动端使用的功能，不是通用 WebDAV 服务器。

源码：`server/src/webdav/`；测试：`server/tests/webdav_test.rs`。

## 端点

```
/dav/{repo_id}/
/dav/{repo_id}/{path}
```

资料库根是 `/dav/{repo_id}/`；`{path}` 是资料库内路径，按段百分号编码。官方 Seafile 的 WebDAV 扩展用 `/seafdav`；Nanofile 用 `/dav` 并把资料库 id 放进路径，每个地址都是自包含的。

## 认证

HTTP Basic：用户名是账号**邮箱**，密码是**WebDAV API 密钥**（在**设置 → API 密钥**里用 WebDAV 预设创建）。这与 Seafile 不同——那里密码是账号密码：Nanofile 的密钥是受限的（`webdav.read` 只读挂载，`webdav.read` + `webdav.write` 读写），并可单独吊销。

失败请求返回 `401` 与 `WWW-Authenticate: Basic realm="nanofile webdav"`；同一地址连续失败会被限流（`429`）。`server.webdav_enabled` 关闭或资料库加密时返回 `403`——WebDAV 没有传递库密码的通道。

## 方法

| 方法 | 行为 |
| --- | --- |
| `OPTIONS` | `200`，`DAV: 1, 2`，`Allow: OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, PROPFIND, PROPPATCH, MOVE, COPY, LOCK, UNLOCK`。 |
| `PROPFIND` | `207 Multi-Status`，列出资源本身，带深度时还列出后代。 |
| `GET` | 文件字节；目录返回 `405`。 |
| `HEAD` | 与 `GET` 相同的头，无正文。 |
| `PUT` | 创建或替换文件（创建 `201`，替换 `204`）。 |
| `MKCOL` | 创建目录（`201`）；已存在 `405`，父目录缺失 `409`。 |
| `DELETE` | 删除文件或整棵目录（`204`）。 |
| `MOVE` | 库内移动（创建 `201`，替换 `204`）。 |
| `COPY` | 库内复制（创建 `201`，替换 `204`）。 |
| `PROPPATCH` | 接受并返回 `207` 空结果；不保存任何属性。 |
| `LOCK` | 接受，返回 `200` 与生成的 `opaquelocktoken`；锁从不强制执行。 |
| `UNLOCK` | `204`。 |

写方法需要密钥的 `webdav.write` 能力与资料库 `rw` 权限；否则 `403`。

## PROPFIND

请求体可以为空、`<D:propfind><D:allprop/></D:propfind>`、`<D:propname/>`，或 `<D:prop>` 选择。请求与返回的属性：

| 属性 | 出现于 | 值 |
| --- | --- | --- |
| `D:resourcetype` | 所有资源 | 目录为 `<D:collection/>`，文件为空 |
| `D:displayname` | 所有资源 | 根为资料库名，否则为条目名 |
| `D:getcontentlength` | 文件 | 字节数；**目录不返回** |
| `D:getlastmodified` | 所有资源 | RFC 1123 日期 |
| `D:getetag` | 文件 | FS 对象 id，带引号——与 `GET` 的 `ETag` 相同 |

`Depth` 接受 `0`、`1` 与 `infinity`（默认）。文件上的 `Depth: infinity` 返回 `400`。结果超过 `server.max_propfind_entries`（默认 100,000）的列表会被拒绝而不是截断：`403` 与 RFC 4918 前置条件正文 `<D:error xmlns:D="DAV:"><D:propfind-finite-depth/></D:error>`，提示客户端改用更浅的深度。这与 seafdav 关闭无限深度时的行为一致。

`href` 是绝对路径（`/dav/{repo_id}/…`），百分号编码，目录带尾斜杠。

## GET 与 HEAD

响应带强 `ETag: "<fs_id>"`、按名称推断的 MIME、`Cache-Control: private, max-age=604800` 与文件字节。支持单个 `Range`（返回 `206` 与 `Content-Range`）；非空文件上不可满足的区间返回 `416` 与 `Content-Range: bytes */<size>`，零字节文件仍是 `200`。`If-None-Match` 在 `Range` 之前求值，所以匹配的校验值即使带区间也返回 `304`。`HEAD` 返回相同头、无正文。

## PUT

请求体边到达边写入块。目标的父目录必须存在（否则 `409`）；已有文件被替换，`201`/`204` 表示资源此前是否存在。没有实现 `If-Match`/`If-Unmodified-Since`：并发写是后写获胜，但被锁文件返回 `403` 与客户端解析的纯文本 `File <path> is locked`。

## MOVE 与 COPY

`Destination` 指定目标（`/dav/{repo_id}/{path}`；也接受绝对 URL）。目标必须在同一资料库（否则 `403`），父目录必须存在（`409`）。目标已存在且 `Overwrite: F` 返回 `412`；默认 `T` 会替换。把目录移进自己的子树返回 `403`。同库移动是一个提交。

复制出来的树会把块复制进目标资料库，因为块按资料库存储。

## 已知限制

不保存 `PROPPATCH`，不强制锁，没有 `If-Match`/`If-Modified-Since`，没有 `REPORT`，没有配额或 ACL 属性。加密资料库完全不做。完整清单见[兼容性页](/zh/reference/api/compatibility#webdav)。
