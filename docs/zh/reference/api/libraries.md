# 资料库

资料库（Seafile 的 `repo`）是用户同步与分享的顶层树。本页覆盖列表、创建、修改、传输链接，以及加密资料库的密码。目录与文件内容见 [目录](/zh/reference/api/directories) 与 [文件](/zh/reference/api/files)。

源码：`server/src/handler/repos.rs`、`server/src/handler/password.rs`、`server/src/handler/chunked_upload.rs`、`server/src/service/repo/service.rs`、`server/src/service/repo/password.rs`。

## 列出资料库 {#get-api2-repos}

`GET /api2/repos/`

返回调用者可见的全部资料库，按成员关系（创建）顺序；官方客户端会自行排序。绑定资料库的 API 密钥只能看到范围内的库。

**响应 `200`**

```json
[
  {
    "id": "…",
    "name": "Docs",
    "desc": "",
    "owner": "user@example.com",
    "owner_name": "User",
    "encrypted": false,
    "size": 22020651,
    "mtime": 1760000000,
    "permission": "rw",
    "head_commit_id": "…",
    "type": "repo",
    "virtual": false,
    "root": "…",
    "salt": null
  }
]
```

`permission` 是 `"rw"` 或 `"r"`。加密资料库还会带 `enc_version`，以及调用者有权拿到的 `magic`/`random_key`/`salt`。

## 创建资料库 {#post-api2-repos}

`POST /api2/repos/`

创建归调用者所有的资料库。请求体可以是 JSON、URL 编码表单（桌面端）或 `multipart/form-data`（Android），按 `Content-Type` 选择。

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `name` | 是 | 资料库名称。 |
| `desc` | 否 | 描述。 |
| `repo_id` | 否 | 克隆时客户端指定的 UUID。 |
| `encrypted` | 否 | 加密资料库填 `1`。 |
| `enc_version` | 否 | 加密版本（`2` 或 `4`）；未加密时忽略。 |
| `magic`、`random_key`、`salt` | 否 | 客户端自己计算的加密材料。 |
| `pwd_hash`、`pwd_hash_algo`、`pwd_hash_params` | 否 | Seafile 11+ 的密码校验三元组，`/api2/server-info/` 声明算法时发送。 |
| `passwd` | 否 | 明文密码，由服务器自行计算上述值。 |

**响应 `201`** —— 创建的资料库，形状同 `GET /api2/repos/{repo_id}/`。需要 `library.create`。

加密资料库由客户端创建，密码只在客户端；服务器只保存 `magic`/`random_key`（或 `pwd_hash` 三元组），永远无法解密其块。`passwd` 供让服务器代算的客户端使用。

## 获取资料库 {#get-api2-repos-repo-id}

`GET /api2/repos/{repo_id}/`

返回一个资料库的元数据，形状同列表项，包含 `permission` 与 `head_commit_id`。尚无提交的资料库 `head_commit_id` 与 `root` 为 `null`。

## 修改资料库 {#post-api2-repos-repo-id}

`POST /api2/repos/{repo_id}/?op=…`

Seafile 重载了这个路径，查询参数 `op` 选择操作：

| `op` | 请求体 | 含义 |
| --- | --- | --- |
| `rename` | `repo_name`（JSON、表单或 multipart） | 重命名；仅所有者。 |
| `update` | JSON `{repo_name?, description?, history_limit?, history_ttl_days?}` | 更新元数据；仅所有者。 |
| `setpassword` | `password`（JSON、表单或 multipart） | 校验加密资料库密码；错误返回 `400` `Incorrect password`。 |
| `checkpassword` | `magic`（JSON、表单或 multipart） | 校验客户端计算的 `magic`。 |
| *省略* | `password` | 桌面端的密码表单；等价于 `setpassword`，但缺字段时响应不同。 |

**响应 `200`** —— `rename`/`update` 返回 JSON 字符串 `"success"`；密码操作返回 `{"success": true}`。

所有密码校验共用一个按（用户, 资料库）的限流器；连续失败返回 `429`。需要 `library.update`（密码操作需要 `library.password`），密钥还需要该库的写权限。

## 删除资料库 {#delete-api2-repos-repo-id}

`DELETE /api2/repos/{repo_id}/`

删除资料库及其数据。**响应 `200`** —— JSON 字符串 `"success"`。需要 `library.delete`；仅所有者。

## 列出资料库（v2.1） {#get-api-v2-1-repos}

`GET /api/v2.1/repos/`

移动端与网页界面使用的 v2.1 形状。

**响应 `200`**

```json
{
  "repos": [
    {
      "repo_id": "…",
      "repo_name": "Docs",
      "repo_desc": "",
      "permission": "rw",
      "encrypted": false,
      "type": "repo",
      "size": 22020651,
      "last_modified": "2026-10-24T09:00:00+00:00",
      "mtime": 1760000000,
      "owner_email": "user@example.com",
      "owner_name": "User",
      "owner_contact_email": "user@example.com"
    }
  ]
}
```

`last_modified` 是 RFC 3339；`mtime` 是同一时刻的 Unix 秒。需要 `library.read`。

## 获取资料库（v2.1） {#get-api-v2-1-repos-repo-id}

`GET /api/v2.1/repos/{repo_id}/`

上表形状中的一项。需要 `library.read`。

## 删除资料库（v2.1） {#delete-api-v2-1-repos-repo-id}

`DELETE /api/v2.1/repos/{repo_id}/`

与旧路径同一操作。**响应 `200`** —— `"success"`。需要 `library.delete`。

## 获取默认资料库 {#get-api2-default-repo}

`GET /api2/default-repo/`

seahub 的主资料库查询。

**响应 `200`** —— `{"exists": true, "repo_id": "…"}` 或 `{"exists": false}`。需要 `library.read`。

## 创建默认资料库 {#post-api2-default-repo}

`POST /api2/default-repo/`

查找用户的默认资料库；用户没有任何库时创建一个名为「My Library」的库。

**响应 `200`** —— `{"repo_id": "…"}`。需要 `library.create`。

## 签发同步令牌 {#get-api2-repo-tokens}

`GET /api2/repo-tokens/?repos=<id>,<id>`

为每个调用者可读的资料库返回一个同步令牌。调用者无权签发的库会被忽略，而不是让整批失败。

**响应 `200`**

```json
{ "…": "…" }
```

映射是 `repo_id → token`。令牌只在 `/seafhttp/`（`Seafile-Repo-Token`）被接受，有效期为 `auth.sync_token_ttl_days`，并随资料库一起失效。签发是 `GET` 上的状态变更，浏览器会话必须带 `X-CSRFToken`；bearer 凭据不需要。

**Seafile** —— 这是 seahub 的旧版 `/api2/repo-tokens/`；官方参考记录的是更新的 `/api/v2.1/repos/{repo_id}/repo-api-tokens/` 接口面，Nanofile 没有实现。需要长期、受限的 HTTP 凭据请用 API 密钥。

## 下载信息 {#get-api2-repos-repo-id-download-info}

`GET /api2/repos/{repo_id}/download-info/`

同步客户端的握手：返回服务端资料库元数据加一个新同步令牌。

**响应 `200`**

```json
{
  "repo_id": "…",
  "repo_name": "Docs",
  "token": "…",
  "email": "user@example.com",
  "relay_id": null,
  "relay_addr": null,
  "relay_port": null,
  "enc_version": 0,
  "encrypted": "",
  "repo_version": 1
}
```

加密资料库的 `encrypted` 为 `1`，并按需出现 `magic`、`random_key`、`salt`。需要读权限与 `sync.token` 能力。

## 上传链接 {#get-api2-repos-repo-id-upload-link}

`GET /api2/repos/{repo_id}/upload-link/?p=<dir>&from=web&replace=1`

返回调用者上传文件用的绝对 URL。

| 查询参数 | 含义 |
| --- | --- |
| `p` | 目标目录，默认 `/`。 |
| `from` | `web` 返回浏览器表单接口（`/upload-aj/<token>`），否则返回客户端接口（`/upload-api/<token>`）。 |
| `replace` | `1` 表示同名文件替换而不是失败。 |

**响应 `200`** —— URL 的 JSON 字符串。需要写权限与 `file.write`。加密资料库拒绝上传链接。

链接令牌只对一个目录有效，见 [上传与下载](/zh/reference/api/transfers#post-upload-api-token)。

## 更新链接 {#get-api2-repos-repo-id-update-link}

`GET /api2/repos/{repo_id}/update-link/?p=<dir>&from=web`

返回更新已有文件的绝对 URL；正文是 JSON 字符串，与上传链接相同。需要写权限与 `file.write`。

## 上传块链接 {#get-api2-repos-repo-id-upload-blks-link}

`GET /api2/repos/{repo_id}/upload-blks-link/?p=<dir>`

返回移动端使用的分块上传接口 URL。加密资料库拒绝。需要写权限与 `file.write`。

## 更新块链接 {#get-api2-repos-repo-id-update-blks-link}

`GET /api2/repos/{repo_id}/update-blks-link/?p=<dir>`

更新块上传。需要写权限与 `file.write`。

## 块下载链接 {#get-api2-repos-repo-id-files-file-id-blks-block-id-download-link}

`GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/`

返回逐块下载文件时使用的绝对 URL。

**响应 `200`** —— URL 的 JSON 字符串。需要读权限与 `file.write` 能力（生成的链接是传输凭据）。

## 设置加密资料库密码 {#post-api-v2-1-repos-repo-id-set-password}

`POST /api/v2.1/repos/{repo_id}/set-password/`

在客户端下载加密资料库前校验密码。

**请求体**（JSON）—— `{"password": "…"}`。

**响应 `200`** —— `{"success": true}`。密码错误返回 `440` 与 `{"error_msg": "repo password is required"}`。连续失败返回 `429`。需要读权限与 `library.password` 能力。

**Seafile** —— 路由相同。旧版 `/api2/repos/{repo_id}/?op=setpassword` 表单返回 `400`，因为桌面端按状态码分支、iOS 按消息匹配。

## 修改加密资料库密码 {#put-api-v2-1-repos-repo-id-set-password}

`PUT /api/v2.1/repos/{repo_id}/set-password/?operation=change-password`

轮换资料库级加密密码。

**请求体**（JSON）—— `{"old_password": "…", "new_password": "…"}`。

**响应 `200`** —— `{"success": true}`。仅所有者，因为轮换会重写所有成员的库级密钥材料。旧密码连续错误返回 `429`。

带 `?operation=check-password` 时改为报告当前用户是否已提供密码：`{"is_set": true|false}`。其他操作返回 `400`。

**Seafile** —— 路由与操作相同。
