# API 密钥与凭据

Nanofile 自己的凭据模型：给脚本与第三方工具用的受限 API 密钥，以及账号持有的全部长期凭据清单。这部分不属于 Seafile API，官方客户端不会使用。

密钥管理**只接受会话**（包括 `POST /api2/auth-token/` 换来的账号令牌）：API 密钥永远不能签发或读取另一个密钥。能这么做的密钥就能自我提权。

源码：`server/src/handler/api_key.rs`、`server/src/handler/credential.rs`、`server/src/handler/devices.rs`、`server/src/service/api_key.rs`、`server/src/domain/capability.rs`。

## 能力

密钥是一组可独立授予的能力，外加资料库范围。每个接口页会写明它需要的能力。能力按域分组：

| 域 | 能力 id |
| --- | --- |
| account | `account.read`、`account.write` |
| device | `device.read`、`device.write` |
| library | `library.read`、`library.create`、`library.update`、`library.delete`、`library.password` |
| file | `file.read`、`file.write`、`file.delete` |
| trash | `trash.read`、`trash.write` |
| history | `history.read`、`history.write` |
| sharing | `share_link.read`、`share_link.write`、`upload_link.read`、`upload_link.write` |
| metadata | `metadata.read`、`metadata.write`、`tag.read`、`tag.write` |
| discovery | `star.read`、`star.write`、`activity.read`、`search.read`、`search.write`、`search.reindex` |
| sync | `sync.read`、`sync.write`、`sync.token` |
| notification | `notification.read` |
| webdav | `webdav.read`、`webdav.write` |
| admin | `admin.user.read`、`admin.user.write` |
| key | `key.read`、`key.write`（保留：永不授予，密钥管理只接受会话） |

写能力隐含对应的读能力；`sync.token` 还隐含 `sync.write` 与 `sync.read`。未知 id 会被拒绝。实时列表见 [`GET /api2/api-keys/catalog/`](#get-api2-api-keys-catalog)，它同时返回每个能力的 `enforced_by` 路由。

## 预设

目录接口会返回预设；它们展开成普通能力集合，保存前可修改。

| 预设 | 用途 |
| --- | --- |
| `sync_client_rw`、`sync_client_ro` | 第三方同步客户端 |
| `webdav_rw`、`webdav_ro` | WebDAV 客户端 |
| `ci_upload` | 只向一个资料库上传，不删除 |
| `readonly_all` | 用户接口面的只读访问 |
| `full` | 除保留的 `key.*` 之外全部 |

## 获取能力目录 {#get-api2-api-keys-catalog}

`GET /api2/api-keys/catalog/`

返回能力、预设与有效期选项，客户端可渲染与设置页相同的选择器。

**响应 `200`**

```json
{
  "capabilities": [
    { "id": "file.read", "domain": "file", "write": false, "enforced_by": ["GET /api2/repos/{repo_id}/file/"] }
  ],
  "presets": [{ "id": "readonly_all", "capabilities": ["account.read"] }],
  "ttl_presets_days": [7, 30, 90, 180, 365],
  "max_ttl_days": 0
}
```

返回的能力集合取决于调用者是否为管理员：`admin.*` 只提供给管理员。`max_ttl_days` 为 `0` 表示服务器不设上限。

## 列出 API 密钥 {#get-api2-api-keys}

`GET /api2/api-keys/`

**响应 `200`**

```json
{
  "keys": [
    {
      "id": 1,
      "name": "backup script",
      "key_prefix": "a1b2c3d4",
      "capabilities": ["library.read", "file.read"],
      "all_repos": false,
      "repos": [{ "repo_id": "…", "permission": "r" }],
      "created_at": 1760000000,
      "expires_at": 1762592000,
      "last_used_at": null
    }
  ]
}
```

这里永远不返回密钥本体，只保留前 8 个字符用于显示。

## 创建 API 密钥 {#post-api2-api-keys}

`POST /api2/api-keys/`

**请求体**（JSON）

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `name` | string | 否 | 显示名，最长 255 字符。 |
| `capabilities` | string[] | 是 | 能力 id。 |
| `all_repos` | bool | 否 | 适用于账号可见的所有资料库，默认 `false`。 |
| `repo_permissions` | array | 否 | `[{"repo_id": "…", "permission": "r" \| "rw"}]`，`all_repos` 为 false 时使用。 |
| `expires_in_days` | integer | 三选一 | 有效天数。 |
| `expires_at` | integer | 三选一 | 绝对过期时间，Unix 秒。 |
| `never` | bool | 三选一 | 永不过期。 |

必须且只能给一个有效期字段。有效期不能超过 `auth.api_key_max_ttl_days`（0 表示不设上限）。

**响应 `200`** —— 密钥视图外加一个 `key` 字段。明文只返回**一次**，服务端只存哈希。

创建成功会给所有者发一封尽力而为的通知邮件。

## 获取单个 API 密钥 {#get-api2-api-keys-id}

`GET /api2/api-keys/{id}/`

返回一个密钥的视图（与列表项相同）。未知 id 或别人的密钥返回 `404`。

## 更新 API 密钥 {#put-api2-api-keys-id}

`PUT /api2/api-keys/{id}/`

接受 `name`、`capabilities`、`all_repos`、`repo_permissions`、`expires_in_days`、`expires_at`、`never` 的任意子集。省略的字段保持不变；三个有效期字段都省略时保留当前过期时间；出现时必须只有一个。

**响应 `200`** —— 更新后的视图。

## 删除 API 密钥 {#delete-api2-api-keys-id}

`DELETE /api2/api-keys/{id}/`

立即吊销。**响应 `200`** —— `{"success": true}`。

## 列出凭据 {#get-api2-credentials}

`GET /api2/credentials/`

账号持有的全部长期凭据：客户端会话、浏览器会话、同步令牌、受信设备，以及 API 密钥数量（内容只能通过仅限会话的密钥接口读取）。

**响应 `200`**

```json
{
  "client_sessions": [
    { "platform": "linux", "platform_display": "Linux", "device_id": "…", "device_name": "laptop", "client_version": "9.0.0", "created_at": 1760000000, "is_desktop_client": true }
  ],
  "browser_sessions": [{ "id": 1, "source": "web", "client": "…", "created_at": 1760000000 }],
  "sync_tokens": [{ "id": 1, "repo_id": "…", "repo_name": "Docs", "device_name": "laptop", "peer_ip": "192.0.2.10", "client_version": "9.0.0", "created_at": 1760000000, "last_sync_at": 1760000000, "expires_at": null }],
  "device_trusts": [{ "id": 1, "device_id": "…", "device_name": "phone", "created_at": 1760000000, "expires_at": 1762592000 }],
  "api_key_count": 2
}
```

不包含令牌本体。需要 `device.read`。

## 吊销单个凭据 {#delete-api2-credentials-kind-id}

`DELETE /api2/credentials/{kind}/{id}/`

`kind` 是 `browser_session`、`sync_token` 或 `device_trust`；`id` 是清单里的行 id。客户端设备要整台吊销，走 [`DELETE /api2/devices/`](#delete-api2-devices)，因为一台设备持有多个凭据。需要 `device.write`。

**响应 `200`** —— `{"revoked": "sync_token", "id": 1}`。

## 列出设备 {#get-api2-devices}

`GET /api2/devices/`

Seafile 兼容的设备列表。Nanofile 把所有凭据行放在一个数组里，每条带 `kind`（`client`、`browser`、`sync_token`、`device_trust`），只读 Seafile 字段的客户端仍能找到客户端设备。

**响应 `200`**

```json
[
  {
    "kind": "client",
    "key": "linux:…",
    "platform": "linux",
    "device_id": "…",
    "device_name": "laptop",
    "client_version": "9.0.0",
    "last_accessed": "2026-10-24 09:00",
    "is_desktop_client": true
  }
]
```

需要 `device.read`。

## 解绑设备 {#delete-api2-devices}

`DELETE /api2/devices/`

吊销一个凭据。支持两种寻址：

- `platform` + `device_id`（表单字段）吊销该设备持有的全部凭据——官方客户端发这种。
- `kind` + `id` 吊销清单里的单行。

**响应 `200`** —— `{"success": true}`。需要 `device.write`。

**Seafile** —— `GET`/`DELETE /api2/devices/` 上游存在，寻址方式相同；Nanofile 的响应增加了额外凭据类型与 `kind` 标记。`api-keys` 与 `credentials` 是 Nanofile 独有。
