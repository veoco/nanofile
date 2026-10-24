# 账号与管理

已认证账号的自身资料、头像、少量管理接口，以及公开的服务器信息接口。

源码：`server/src/handler/account.rs`、`server/src/handler/avatar.rs`、`server/src/handler/user_avatar.rs`、`server/src/handler/users.rs`、`server/src/handler/server_info.rs`、`server/src/handler/notifications.rs`、`server/src/service/user/account.rs`。

## 获取账号信息 {#get-api2-account-info}

`GET /api2/account/info/`

返回当前账号与配额使用情况。

**响应 `200`**

```json
{
  "email": "user@example.com",
  "name": "User",
  "nickname": "Display name",
  "id": 2,
  "usage": 22020651,
  "total": 30000000,
  "avatar_url": "https://files.example.com/avatars/user/user@example.com/resized/256/",
  "contact_email": "user@example.com",
  "space_usage": "73.40217%"
}
```

`total` 是存储配额（字节），`-1` 表示不限。没有显示名时省略 `nickname`。API 密钥需要 `account.read`。

**Seafile** —— 路由相同。响应不包含 seahub 的 `login_id`、`is_staff`、`email_notification_interval`、`institution`、`department`；官方客户端都不读这些字段。

## 更新账号信息 {#put-api2-account-info}

`PUT /api2/account/info/`

修改显示名。用户改昵称时官方客户端发送 `{"name": "…"}`。

**请求体** —— JSON：`{"name": "New name"}`（提取器要求必填）。

**响应 `200`** —— 与 `GET /api2/account/info/` 相同。

需要 `account.write`；带该能力的密钥可以调用。

## 创建账号 {#post-api2-account}

`POST /api2/account/`

仅管理员。用 `email` 与 `password` 表单字段创建账号，密码策略照常生效。同一 handler 也挂在 `POST /api2/accounts/` 上，供使用 seahub 另一种拼写的客户端。

**请求体** —— URL 编码表单：

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `email` | 是 | 登录地址，必须唯一。 |
| `password` | 是 | 必须满足 `auth.password_min_length` 与强度策略。 |

**响应 `200`** —— 空正文。非管理员返回 `403`。

### 创建账号（另一种拼写） {#post-api2-accounts}

`POST /api2/accounts/`

与 `POST /api2/account/` 是同一个 handler，供使用 seahub 另一种拼写的客户端。字段、响应与权限相同。

自助注册是网页流程（`/accounts/register/`），不走这个接口：它要处理注册模式、邀请码、邮箱验证与限流。

## 获取头像 {#get-api2-avatars-user-email-resized-size}

`GET /api2/avatars/user/{email}/resized/{size}/`

返回用户头像的 URL。URL 是绝对地址，指向图片接口；`size` 会吸附到 32、48、64、72、80、96、128、256 之一（更大的请求用 256，超过 1024 返回 `400`）。

**响应 `200`**

```json
{ "url": "https://files.example.com/avatars/user/user@example.com/resized/256/", "is_default": false, "mtime": 1760000000 }
```

接口从不泄露邮箱是否存在：未知用户或没有头像时返回默认头像 URL，`is_default: true`、`mtime: 0`。需要 `account.read`。

图片本身在 `GET /avatars/user/{email}/resized/{size}/` 匿名提供（没有头像时返回 SVG 默认图），因为头像 URL 会嵌在页面与客户端里。

## 上传头像 {#post-api-v2-1-user-avatar}

`POST /api/v2.1/user-avatar/`

为当前账号上传新头像。

**请求体** —— `multipart/form-data`，文件字段名 `avatar`（也接受 `file`）。最大 1 MiB；图片会重新编码为 PNG。需要 `account.write`。

**响应 `200`**

```json
{ "avatar_url": "/avatars/user/user@example.com/resized/256/" }
```

## 列出用户 {#get-api2-admin-users}

`GET /api2/admin/users/`

仅管理员。返回全部账号。

**响应 `200`**

```json
[
  {
    "id": 2,
    "email": "user@example.com",
    "is_active": true,
    "is_admin": false,
    "storage_quota": 30000000,
    "usage": 22020651,
    "created_at": 1760000000,
    "last_login_at": 1760000000
  }
]
```

需要 `admin.user.read`。这是 Nanofile 自己的管理接口，不是 seahub 的 `/api/v2.1/admin/users/`，字段名与选项都不同。

## 创建用户 {#post-api2-admin-users}

`POST /api2/admin/users/`

仅管理员。

**请求体**（JSON）

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `email` | 是 | 登录地址。 |
| `password` | 是 | 必须满足密码策略。 |
| `is_admin` | 否 | 默认 `false`。 |
| `is_active` | 否 | 默认 `true`。 |
| `storage_quota` | 否 | 字节数；`null` 用服务器默认值。 |

**响应 `200`** —— 创建的用户，形状同列表项。需要 `admin.user.write`。

## 更新用户 {#put-api2-admin-users-user-id}

`PUT /api2/admin/users/{user_id}/`

仅管理员。整体替换管理标记。

**请求体**（JSON）—— `is_admin`、`is_active`、`storage_quota` 都会被读取；发送你想要的完整状态。`user_id` 是列表里的数字 `id`。

**响应 `200`** —— 更新后的用户。需要 `admin.user.write`。

## 删除用户 {#delete-api2-admin-users-user-id}

`DELETE /api2/admin/users/{user_id}/`

仅管理员。删除账号及其数据。**响应 `200`** —— `{"success": true}`。需要 `admin.user.write`。

## 未读消息 {#get-api2-unseen-messages}

`GET /api2/unseen_messages/`

seahub 的通知计数。Nanofile 没有通知收件箱，计数永远是 `0`。

**响应 `200`** —— `{"count": 0}`。需要 `notification.read`。

## Ping {#get-api2-ping}

`GET /api2/ping/`

公开的连通性探针。

**响应 `200`** —— JSON 字符串 `"pong"`。

## 服务器信息 {#get-api2-server-info}

`GET /api2/server-info/`

公开。返回本服务器声明的 Seafile 版本与面向客户端的功能开关。

**响应 `200`**

```json
{
  "version": "8.0.0",
  "encrypted_library_version": 2,
  "features": ["seafile-basic", "seafile-pro", "file-search"]
}
```

`version` 是 `server.version`，客户端解析它并据此裁剪功能。`desktop-custom-brand` 与 `desktop-custom-logo` 只在配置后出现；`encrypted_library_pwd_hash_algo`/`encrypted_library_pwd_hash_params` 只在该服务器声明客户端密码哈希算法时出现。`features` 始终包含 `seafile-basic` 与 `seafile-pro`；`file-search` 与 `client-sso-via-local-browser` 跟随配置，关闭分享下载时额外加上 `share-link-disabled`。

**Seafile** —— 路由与形状相同。`encrypted_library_version` 默认是 2，与 seahub 一致；也支持 4（每库盐），客户端新建加密资料库时会用它。
