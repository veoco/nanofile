# 同步协议

桌面端与移动端守护进程使用的 `/seafhttp/` API。它和 REST API 是两套凭据体系：请求携带**资料库同步令牌**，从不接受账号令牌或 API 密钥。

协议规则——提交 id、FS 对象 id、目录顺序、空头哨兵——见 [Seafile 兼容性](/zh/dev/protocol)；本页记录协议端点。

源码：`server/src/handler/sync/`、`server/src/middleware/auth.rs`（`SyncAuth`）；测试：`server/tests/sync_test.rs`、`server/tests/sync_merge_test.rs`、`server/tests/client_compat_test.rs`。

## 认证

| 请求头 | 含义 |
| --- | --- |
| `Seafile-Repo-Token: <token>` | 资料库同步令牌。 |
| `Authorization: Token <token>` | 同一令牌，供使用标准头的客户端。 |

同步令牌构造上绑定一个资料库。把令牌用在另一个资料库的 URL 上返回 `403`，过期令牌或账号不再活跃也一样。`head-commits-multi` 是唯一不认证的端点（见下）。

## 状态码

除常规状态码外，守护进程依赖这些：

| 状态码 | 含义 |
| --- | --- |
| `400` | 请求格式错误，或缺少 `token` 头（`token is null`）。 |
| `403` | 无权限、跨库令牌，或 `File <path> is locked`。 |
| `443` | 上传会超过所有者的存储配额。 |
| `444` | 资料库已删除。 |
| `446` | 上传的文件缺少块。 |
| `441` | 写入目标不存在。 |

## 协议版本 {#get-seafhttp-protocol-version}

`GET /seafhttp/protocol-version`

**响应 `200`** —— `{"version": 2}`。

## 头提交 {#get-seafhttp-repo-repo-id-commit-head}

`GET /seafhttp/repo/{repo_id}/commit/HEAD`

**响应 `200`** —— `{"is_corrupted": 0, "head_commit_id": "…"}`。

尚无提交的资料库返回全零 SHA-1（`000…0`）；已删除的资料库返回 `444`，客户端会映射为「服务器上已删除」，而不是无限重试 `404`。

## 更新分支 {#put-seafhttp-repo-repo-id-commit-head}

`PUT /seafhttp/repo/{repo_id}/commit/HEAD?head=<commit_id>`

把资料库 HEAD 移到 `commit_id`，该提交必须已上传。这是同步的提交点：此前上传的块与 FS 对象从此可达。

**响应 `200`** —— 空正文。需要写权限。如果客户端取回后 HEAD 又变了，服务器会做与 seafile 相同的服务端合并：干净合并产生一个合并提交（描述 `Auto merge by system`），分支最终指向合并后的树，不会丢失更新。缺块在切换前返回 `446`。

## 获取提交 {#get-seafhttp-repo-repo-id-commit-commit-id}

`GET /seafhttp/repo/{repo_id}/commit/{commit_id}`

以 JSON 返回提交对象（`application/json`），形状是 Seafile 的 `CommitData`，加密资料库会带加密信息。全零 id 返回一个合成的空提交，无提交资料库的客户端读的就是它。

## 上传提交 {#put-seafhttp-repo-repo-id-commit-commit-id}

`PUT /seafhttp/repo/{repo_id}/commit/{commit_id}`

保存一个提交对象。请求体是客户端计算 id 时用的 JSON。

**响应 `200`** —— 空正文。以下情况返回 `400`：请求体 `repo_id` 与 URL 不符、id 格式错误、id 与 URL 不符、id 不是提交字段的哈希。root id 不在这里校验，而在更新分支时校验。

## 多个资料库的头提交 {#post-seafhttp-repo-head-commits-multi}

`POST /seafhttp/repo/head-commits-multi/`

请求体：资料库 id 的 JSON 数组（最多 4096 个）。**响应 `200`** —— 每个 id 映射到头提交 id 的对象。

该端点按协议**不认证**：守护进程调用时不带凭据，因此改为严格校验 id。

## 列出 FS 对象 id {#get-seafhttp-repo-repo-id-fs-id-list}

`GET /seafhttp/repo/{repo_id}/fs-id-list/?server-head=<id>&client-head=<id>&dir-only=<0|1>`

客户端需要下载的 FS 对象 id。给了 `client-head` 时只返回两棵树的差异；否则返回整棵树。`dir-only` 只返回目录对象。

**响应 `200`** —— 40 位十六进制 id 的 JSON 数组。缺少 `server-head` 或 id 非法返回 `400`；空头返回 `[]`。

## 打包 FS 对象 {#post-seafhttp-repo-repo-id-pack-fs}

`POST /seafhttp/repo/{repo_id}/pack-fs/`

请求体：FS 对象 id 的 JSON 数组（较老客户端发 URL 编码的 `fs_ids` 表单）。**响应 `200`** —— `application/octet-stream`，zlib 打包的对象包。

请求的 id 读不到时返回 `500`，而不是返回不完整的包：守护进程会立刻无上限重试短包，静默少给会变成死循环。

## 检查 FS 对象 {#post-seafhttp-repo-repo-id-check-fs}

`POST /seafhttp/repo/{repo_id}/check-fs/`

请求体：FS 对象 id 的 JSON 数组。**响应 `200`** —— `{"missing": ["…"]}`。

## 接收 FS 对象 {#post-seafhttp-repo-repo-id-recv-fs}

`POST /seafhttp/repo/{repo_id}/recv-fs/`

请求体：FS 对象包（`pack-fs` 同一种编码）。对象按发送时的 id 保存；`sync.verify_fs_objects = strict`（默认）时，内容与 id 不符的对象返回 `400`。

**响应 `200`** —— 空正文。

## 检查块 {#post-seafhttp-repo-repo-id-check-blocks}

`POST /seafhttp/repo/{repo_id}/check-blocks/`

请求体：块 id 的 JSON 数组（或 URL 编码的 `block_ids` 表单）。**响应 `200`** —— 服务器没有的 id 数组。需要写权限，因为这个答案是一个块存在性探针。

## 获取块 {#get-seafhttp-repo-repo-id-block-block-id}

`GET /seafhttp/repo/{repo_id}/block/{block_id}`

**响应 `200`** —— 块的原始字节。别的资料库有、本库没有的块返回 `404`：块按资料库存储，A 库的令牌不能读 B 库的内容。

## 上传块 {#put-seafhttp-repo-repo-id-block-block-id}

`PUT /seafhttp/repo/{repo_id}/block/{block_id}`

请求体：块的原始字节。id 必须等于正文的 SHA-1（否则 `400`）。上传作为未提交写入计入用户配额（超限 `443`）；更新分支时释放预留。

**响应 `200`** —— 空正文。需要写权限。

## 块映射 {#get-seafhttp-repo-repo-id-block-map-file-id}

`GET /seafhttp/repo/{repo_id}/block-map/{file_id}`

一个文件对象的块偏移。**响应 `200`** —— 整数 JSON 数组。

## 权限检查 {#get-seafhttp-repo-repo-id-permission-check}

`GET /seafhttp/repo/{repo_id}/permission-check/?op=<upload|download>&client_id=<id>`

**响应 `200`** —— 空正文。只读成员请求 `upload` 得到 `403` 与 `{"reason": "no write permission"}`，守护进程能把它与普通拒绝区分开。已删除资料库返回 `444`；`op` 必须是 `upload` 或 `download`，`client_id` 发送时必须是 40 字符。

## 配额检查 {#get-seafhttp-repo-repo-id-quota-check}

`GET /seafhttp/repo/{repo_id}/quota-check/?delta=<bytes>`

上传前调用。**响应 `200`** 表示增量放得下；放不下返回 `443`；缺少或非法的 delta 返回 `400`。

## 通知 JWT {#get-seafhttp-repo-repo-id-jwt-token}

`GET /seafhttp/repo/{repo_id}/jwt-token`

**响应 `200`** —— `{"jwt_token": "…"}`，供通知 WebSocket（[通知 WebSocket](/zh/reference/api/notification)）使用。HS256，有效期 3 天，携带 `repo_id`、`username` 与 `exp`。通知服务未配置时返回 `404`。

## 加锁文件 {#put-seafhttp-repo-repo-id-lock-file}

`PUT /seafhttp/repo/{repo_id}/lock-file?p=<path>`

**响应 `200`** —— `{"success": true}`。需要写权限。锁是建议性的，见 [文件](/zh/reference/api/files#put-api2-repos-repo-id-file)。

## 解锁文件 {#put-seafhttp-repo-repo-id-unlock-file}

`PUT /seafhttp/repo/{repo_id}/unlock-file?p=<path>`

**响应 `200`** —— `{"success": true}`。需要写权限。

## 列出已锁文件 {#post-seafhttp-repo-locked-files}

`POST /seafhttp/repo/locked-files`

请求体：`{"repo_id", "token", "ts"}` 条目的 JSON 数组。

**响应 `200`** —— `{"repo_id", "ts", "locked_files": [{"path", "by_me"}]}` 数组，`by_me` 为 `1` 表示自己加的锁，`0` 表示别人。

## 目录权限 {#post-seafhttp-repo-folder-perm}

`POST /seafhttp/repo/folder-perm`

请求体：`{"repo_id", "token", "ts"}` 条目的 JSON 数组。

**响应 `200`** —— `{"repo_id", "ts", "user_perms": [{"path": "/", "permission": "rw"}], "group_perms": []}` 数组。Nanofile 没有目录级分享，所以只有根权限一项，`group_perms` 永远为空。令牌无效时返回空权限列表而不是错误。

## 可访问的资料库 {#get-seafhttp-accessible-repos}

`GET /seafhttp/accessible-repos`

令牌用户可同步的全部资料库，每个带一个可用令牌。

**响应 `200`**

```json
[
  { "repo_id": "…", "repo_name": "Docs", "repo_desc": "", "owner_email": "user@example.com", "token": "…", "permission": "rw" }
]
```

**Seafile** —— `/seafhttp/` 的路径、参数与状态码与 seafile-server 的 fileserver 一致。Nanofile 不实现独立 seafile-server 的 relay/`jwt-token` 变体；由于没有目录级分享，`folder-perm` 只报告根权限。
