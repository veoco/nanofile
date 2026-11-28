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
  "nickname": "User",
  "id": 2,
  "usage": 22020651,
  "total": 30000000,
  "avatar_url": "https://files.example.com/avatar/render_primary/user@example.com/80/",
  "contact_email": "user@example.com",
  "space_usage": "73.40217%"
}
```

`total` 是存储配额（字节），`-1` 表示不限。`nickname` 始终存在且等于 `name`：有显示名时取显示名，否则取账号名，再否则取邮箱的本地部分。API 密钥需要 `account.read`。

**Seafile** —— 路由相同。响应不包含 seahub 的 `login_id`、`is_staff`、`email_notification_interval`、`institution`、`department`；官方客户端都不读这些字段。

## 更新账号信息 {#put-api2-account-info}

`PUT /api2/account/info/`

修改显示名。用户改昵称时官方客户端发送 `{"name": "…"}`。

**请求体** —— JSON：`{"name": "New name"}`（提取器要求必填）。

**响应 `200`** —— 与 `GET /api2/account/info/` 相同。

需要 `account.write`；带该能力的密钥可以调用。


## 获取头像 {#get-api2-avatars-user-email-resized-size}

`GET /api2/avatars/user/{email}/resized/{size}/`

返回用户头像的 URL。URL 是绝对地址，指向图片接口；`size` 会吸附到 32、48、64、72、80、96、128、256 之一（更大的请求用 256，超过 1024 返回 `400`）。

**响应 `200`**

```json
{ "url": "https://files.example.com/avatar/render_primary/user@example.com/256/", "is_default": false, "mtime": 1760000000 }
```

接口从不泄露邮箱是否存在：未知用户或没有头像时返回默认头像 URL，`is_default: true`、`mtime: 0`。需要 `account.read`。

## 获取头像图片 {#get-avatar-image}

`GET /avatar/render_primary/{email}/{size}/`

上面 `url` 指向的图片字节。匿名提供，因为头像 URL 会嵌在页面与客户端里；未知用户或没有头像的账号得到 SVG 默认图。缩略图首次访问时生成并缓存（`Cache-Control: private, max-age=86400`）。

该路径是 seahub 公开的 `avatar_render_primary` 视图——它的 `primary_avatar` 模板标签就把 `<img>` 指到这里。seahub 对该视图返回到 `/media/avatars/…` 的重定向；本服务没有 `/media` 前缀，直接返回图片字节，浏览器两种方式都能拿到图。0.1.23 及更早版本的根路径拼写 `/avatars/user/{email}/resized/{size}/` 会永久重定向到这里。

## 上传头像 {#post-api-v2-1-user-avatar}

`POST /api/v2.1/user-avatar/`

为当前账号上传新头像。

**请求体** —— `multipart/form-data`，文件字段名 `avatar`（也接受 `file`）。最大 1 MiB。原图按自己的扩展名保存（`.png`、`.jpg`、`.jpeg` 或 `.gif`）；所有缩略图统一渲染为 PNG。需要 `account.write`。

**响应 `200`**

```json
{ "avatar_url": "/avatar/render_primary/user@example.com/256/" }
```

## 列出所有用户信息 {#get-api-v2-1-admin-users}

`GET /api/v2.1/admin/users/?page=&per_page=&is_active=&role=&source=&order_by=&direction=`

仅管理员。以 seahub 的信封返回账号：

```json
{
  "data": [
    {
      "avatar_url": "https://files.example.com/avatar/render_primary/user@example.com/80/",
      "email": "user@example.com",
      "name": "user",
      "contact_email": "user@example.com",
      "login_id": "",
      "is_staff": false,
      "is_active": true,
      "quota_usage": 22020651,
      "quota_total": 30000000,
      "role": "default",
      "create_time": "2026-10-24T09:00:00+00:00",
      "last_login": "2026-10-24T09:00:00+00:00",
      "last_access_time": "2026-10-24T09:00:00+00:00",
      "social_auth": []
    }
  ],
  "total_count": 1
}
```

`page` 默认 1，`per_page` 默认 25（上限 500）。`is_active` 接受 `1` 或 `0`。`source` 接受 `db`（默认）与 `ldapimport`（永远返回空页：没有 LDAP 后端）。`role` 接受 `default`；其他值匹配不到账号。`order_by=quota_usage` 配 `direction=asc|desc` 按用量排序，账号超过 500 个时拒绝排序。

列表项字段照 seahub 的 `get_user_info()` 再加 `avatar_url`；差异：`login_id` 永远为空、`contact_email` 等于 `email`、`role` 永远是 `default`、`social_auth` 永远为空。`quota_total` 是字节数（`-1` 表示不限），`quota_usage` 是已用字节，时间戳是 ISO 8601。

需要 `admin.user.read`。

**Seafile** —— 路由与信封相同；差异即上面列出的字段取值。

## 添加用户 {#post-api-v2-1-admin-users}

`POST /api/v2.1/admin/users/`

仅管理员。接受 JSON 或 `application/x-www-form-urlencoded`（seahub 的 DRF 视图两种都收）。

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `email` | 是 | 登录地址，必须合法且唯一。 |
| `password` | 是 | 必须满足 `auth.password_min_length` 与 `require_strong_password`，与其他建号路径一致。 |
| `is_staff` | 否 | 默认 `false`；存为账号的管理员标记。 |
| `is_active` | 否 | 默认 `true`。 |
| `name` | 否 | 昵称，最多 64 字符且不能含 `/`。 |
| `quota_total` | 否 | 兆字节；`0` 表示不限，省略则用服务器默认值。 |
| `role` | 否 | 只接受 `""` 或 `default`；Nanofile 没有自定义角色。 |

**响应 `200`** —— 创建出的账号，形状同列表项。

邮箱非法、账号已存在（`User <email> already exists.`）或密码不合策略时返回 `400`。需要 `admin.user.write`。

自助注册是独立的网页流程（`/accounts/register/`）：它处理注册模式、邀请码、邮箱验证与限流。

**Seafile** —— 路由、请求体字段与响应相同；`role` 按上文限制。

## 获取某个用户的信息 {#get-api-v2-1-admin-users-user-id}

`GET /api/v2.1/admin/users/{user_id}/`

仅管理员。路径段是账号的**邮箱**：seahub 的 URL 正则在这里匹配邮箱，虽然参考文档把参数写作 `{user_id}`。

**响应 `200`** —— 一个列表项（含 `last_login`、`last_access_time` 与 `social_auth`）。账号不存在返回 `404`（`User <email> not found.`）。需要 `admin.user.read`。

## 更新用户信息 {#put-api-v2-1-admin-users-user-id}

`PUT /api/v2.1/admin/users/{user_id}/`

仅管理员；路径段是邮箱。**部分更新**：未给的字段保持当前值。

| 字段 | 含义 |
| --- | --- |
| `is_staff` | 管理员标记。 |
| `is_active` | 停用会立即撤销该账号的会话、同步令牌、API 密钥与未到期的能力 URL。 |
| `quota_total` | 兆字节；`0` 表示不限。 |
| `name` | 昵称；空字符串清除。 |
| `password` | 新密码，须符合配置的策略。 |
| `role` | 只接受 `""` 或 `default`。 |

**响应 `200`** —— 更新后的列表项，另带 `"update_status_tip": ""`（本服务不发通知邮件）。需要 `admin.user.write`。

`login_id`、`contact_email`、`institution` 与 Pro 的限速字段在本服务没有存储，会被忽略；没有 `keep_sharing` 开关（停用会保留分享，与 seahub 默认模式一致）。

## 删除用户 {#delete-api-v2-1-admin-users-user-id}

`DELETE /api/v2.1/admin/users/{user_id}/`

仅管理员；路径段是邮箱。删除账号及其数据。**响应 `200`** —— `{"success": true}`。需要 `admin.user.write`。

拥有共享或公开资料库的账号在把资料库恢复为私有前不能删除（返回 `400`）；本服务宁可拒绝，也不静默撤销所有接收者的访问。
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

**Seafile** —— 路由与形状相同。`encrypted_library_version` 默认是 2，与 seahub 一致；也支持 4（每库盐）。客户端新建加密资料库时会采用服务器公告的值，所以默认 2 时新建的就是 v2 库；要让新库使用每库盐，需把它设为 4。客户端只发送 `passwd`、由服务端代生成密钥时，也使用这个值。
