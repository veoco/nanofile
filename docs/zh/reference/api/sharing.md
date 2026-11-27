# 分享

分享链接（下载文件或目录）、上传链接（让匿名访客上传）、管理员共享库、收藏与动态流。Nanofile 没有群组：下面的群组接口只是官方客户端加载链需要的空桩。

源码：`server/src/handler/links.rs`、`server/src/handler/share.rs`、`server/src/handler/upload_link.rs`、`server/src/handler/library_share.rs`、`server/src/handler/starred.rs`、`server/src/handler/activities.rs`、`server/src/handler/groups.rs`、`server/src/handler/smart_link.rs`、`server/src/service/sharing/`；测试：`server/tests/share_link_test.rs`、`server/tests/upload_link_test.rs`、`server/tests/library_sharing_test.rs`。

## 公开页面

分享链接的 `link` 字段指向 HTML 页面，不是 API 接口：

| URL | 内容 |
| --- | --- |
| `/f/{token}` | 共享文件的预览与下载页。`POST` 提交密码表单。 |
| `/d/{token}` | 共享目录的列表页。`POST` 提交密码表单。 |
| `/d/{token}/files/{path}` | 共享目录里的一个文件。 |
| `/u/{token}` | 上传链接的匿名上传页。`POST` 上传（带密码表单）。 |

这些页面给浏览器用；API 客户端用下面的 JSON 接口。

## 列出分享链接 {#get-api2-shared-links}

`GET /api2/shared-links/`

调用者创建的全部分享链接，受 API 密钥可见范围限制。

**响应 `200`**

```json
[
  {
    "token": "…",
    "link": "https://files.example.com/d/…/",
    "repo_id": "…",
    "path": "/photos",
    "created_at": 1760000000,
    "has_password": true,
    "expire_at": 1762592000,
    "s_type": "d",
    "view_cnt": 4
  }
]
```

`s_type` 为 `d` 表示目录链接、`f` 表示文件链接。需要 `share_link.read`。

## 创建分享链接 {#post-api2-shared-links}

`POST /api2/shared-links/`

旧版创建接口。**请求体**（JSON）：`repo_id`、`path`、可选 `password` 与 `expires_at`（Unix 秒）。

**响应 `200`** —— 列表形状的一项。需要 `share_link.write` 与资料库读权限；加密资料库会被拒绝。关闭分享下载（`server.share_download_enabled`）或资料库加密时拒绝。

## 删除分享链接 {#delete-api2-shared-links-token}

`DELETE /api2/shared-links/{token}`

删除调用者的一条链接。**响应 `200`** —— `{"success": true}`。需要 `share_link.write`。

## 列出分享链接（v2.1） {#get-api-v2-1-share-links}

`GET /api/v2.1/share-links/?repo_id=<id>&path=<path>`

列出调用者的链接。两个过滤参数都可选；`path` 只有在同时给 `repo_id` 时才有意义，且调用者必须能读该资料库。

**响应 `200`** —— 与旧版列表相同的条目数组。需要 `share_link.read`。

## 创建分享链接（v2.1） {#post-api-v2-1-share-links}

`POST /api/v2.1/share-links/`

接受 JSON 或 `application/x-www-form-urlencoded`（桌面端发表单）。

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `repo_id` | 是 | 资料库 id。 |
| `path` | 是 | 分享的文件或目录路径。 |
| `password` | 否 | 访问密码。 |
| `expire_days` | 否 | 多少天后过期。 |
| `expiration_time` | 否 | RFC 3339 过期时间，`expire_days` 缺席时使用。 |
| `description` | 否 | 随链接保存的说明。 |

**响应 `200`**

```json
{
  "token": "…",
  "link": "https://files.example.com/d/…/",
  "repo_id": "…",
  "repo_name": null,
  "path": "/photos",
  "obj_name": "photos",
  "s_type": "d",
  "is_dir": true,
  "username": null,
  "view_cnt": 0,
  "ctime": 1760000000,
  "expire_date": 1762592000,
  "is_expired": false,
  "has_password": false,
  "permissions": { "can_edit": false, "can_download": true, "can_upload": false },
  "password": null,
  "description": null
}
```

`repo_name`、`username`、`password` 永远是 `null`；`permissions` 固定（链接只读）。需要 `share_link.write`。

## 创建多条分享链接 {#post-api-v2-1-multi-share-links}

`POST /api/v2.1/multi-share-links/`

与 `POST /api/v2.1/share-links/` 相同；Android 客户端向这里发送。

## 获取分享链接 {#get-api-v2-1-share-links-token}

`GET /api/v2.1/share-links/{token}/`

调用者的一条链接，detail 形状：创建形状去掉 `s_type`。需要 `share_link.read`；别人的链接返回 `404`。

## 更新分享链接 {#put-api-v2-1-share-links-token}

`PUT /api/v2.1/share-links/{token}/`

更新 `password`、`expire_days` 和/或 `description`（JSON）。发 `null` 清空字段，省略则保持不变。

**响应 `200`** —— 旧版列表形状的链接（不是创建形状）。需要 `share_link.write`。

## 删除分享链接（v2.1） {#delete-api-v2-1-share-links-token}

`DELETE /api/v2.1/share-links/{token}/`

**响应 `200`** —— `{"success": true}`。需要 `share_link.write`。别人的链接返回 `403`；令牌不存在仍返回 `200`（删除是幂等的）；链接超出 API 密钥的库范围时隐藏为 `404`。

## 列出上传链接 {#get-api2-upload-links}

`GET /api2/upload-links/`

调用者的上传链接。**响应 `200`**

```json
[
  {
    "token": "…",
    "link": "https://files.example.com/u/…/",
    "repo_id": "…",
    "path": "/incoming",
    "created_at": 1760000000
  }
]
```

需要 `upload_link.read`。

## 创建上传链接 {#post-api2-upload-links}

`POST /api2/upload-links/`

**请求体**（JSON）—— `repo_id`、`path`、可选 `password` 与 `expires_at`（Unix 秒）。

**响应 `200`** —— 旧版列表形状的一项。需要 `upload_link.write` 与资料库写权限。上传链接不能指向加密资料库；`server.share_upload_enabled` 关闭时拒绝创建。

## 删除上传链接 {#delete-api2-upload-links-token}

`DELETE /api2/upload-links/{token}`

**响应 `200`** —— `{"success": true}`。需要 `upload_link.write`。

## 列出上传链接（v2.1） {#get-api-v2-1-upload-links}

`GET /api/v2.1/upload-links/?repo_id=<id>&path=<path>`

过滤参数同分享链接列表。

**响应 `200`** —— 上传链接条目数组：

```json
[{ "token": "…", "repo_id": "…", "path": "/incoming", "has_password": false, "expire_at": null, "view_cnt": 0, "description": null }]
```

需要 `upload_link.read`。

## 创建上传链接（v2.1） {#post-api-v2-1-upload-links}

`POST /api/v2.1/upload-links/`

字段与编码同分享链接创建。

**响应 `200`** —— 创建的上传链接，包含 `link`、`ctime`、`obj_name` 等 v2.1 字段。需要 `upload_link.write`。

## 获取上传链接 {#get-api-v2-1-upload-links-token}

`GET /api/v2.1/upload-links/{token}/`

调用者的一条上传链接。需要 `upload_link.read`。

## 更新上传链接 {#put-api-v2-1-upload-links-token}

`PUT /api/v2.1/upload-links/{token}/`

更新 `password`、`expire_days` 和/或 `description`。**响应 `200`** —— `{"success": true}`。需要 `upload_link.write`。

## 删除上传链接（v2.1） {#delete-api-v2-1-upload-links-token}

`DELETE /api/v2.1/upload-links/{token}/`

**响应 `200`** —— 裸 JSON 值 `true`（Android 客户端按布尔解析）。需要 `upload_link.write`。

## 清理失效的上传链接 {#delete-api-v2-1-upload-links-clean-invalid}

`DELETE /api/v2.1/upload-links/clean-invalid/`

删除调用者那些资料库或路径已不存在的链接。

**响应 `200`** —— `{"success": true, "deleted": 2}`。需要 `upload_link.write`。

## 获取匿名上传 URL {#get-api-v2-1-upload-links-token-upload}

`GET /api/v2.1/upload-links/{token}/upload/`

为链接持有者签发短命的上传 URL，无需账号凭据。

**响应 `200`** —— `{"upload_link": "/upload-aj/<token>"}`。

带密码的链接需要密码页设置的 `visited_ufs_<token>` Cookie，否则 `403`。过期链接返回 `404`；加密资料库被拒绝。`server.share_upload_enabled` 关闭时拒绝。

## 列出资料库的上传链接 {#get-api-v2-1-repos-repo-id-upload-links}

`GET /api/v2.1/repos/{repo_id}/upload-links/`

为一个资料库中调用者自己创建的上传链接；不返回其他用户创建的链接。**响应 `200`** —— 上传链接条目数组。需要读权限与 `upload_link.read`。

## 列出收藏文件 {#get-api2-starredfiles}

`GET /api2/starredfiles/`

旧版收藏列表，与调用者仍可访问的资料库取交集。

**响应 `200`**

```json
[{ "repo_id": "…", "path": "/notes.txt", "size": null, "last_modified": null, "is_dir": false }]
```

需要 `star.read`。

## 列出收藏项 {#get-api-v2-1-starred-items}

`GET /api/v2.1/starred-items/`

v2.1 收藏列表，按资料库、目录、文件分组。

**响应 `200`**

```json
{
  "starred_item_list": [
    {
      "repo_id": "…",
      "repo_name": "Docs",
      "repo_encrypted": false,
      "is_dir": false,
      "path": "/notes.txt",
      "obj_name": "notes.txt",
      "mtime": "2026-10-24T09:00:00+00:00",
      "deleted": false,
      "user_email": "user@example.com",
      "user_name": "User",
      "user_contact_email": "user@example.com"
    }
  ]
}
```

需要 `star.read`。

## 收藏一项 {#post-api-v2-1-starred-items}

`POST /api/v2.1/starred-items/`

**请求体** —— JSON `{"repo_id": "…", "path": "/notes.txt"}`，或同样字段的 multipart。路径必须存在；收藏 `/` 表示收藏资料库。

**响应 `200`** —— 列表形状的收藏项。需要 `star.write` 与资料库读权限。

## 取消收藏 {#delete-api-v2-1-starred-items}

`DELETE /api/v2.1/starred-items/?repo_id=<id>&path=<path>`

**响应 `200`** —— `{"success": true}`。未收藏的项返回 `404`。需要 `star.write`。

## 动态流 {#get-api-v2-1-activities}

`GET /api/v2.1/activities/?page=<n>&per_page=<n>&repo_id=<id>&op_user=<email>`

调用者可见的文件与资料库事件。`page` 默认 1，`per_page` 默认 25（上限 100）；`repo_id` 与 `op_user` 过滤事件流。

**响应 `200`**

```json
{
  "events": [
    {
      "id": 1,
      "op_type": "create",
      "obj_type": "file",
      "repo_id": "…",
      "repo_name": "Docs",
      "path": "/notes.txt",
      "old_path": null,
      "commit_id": "…",
      "name": "notes.txt",
      "old_name": null,
      "author_email": "user@example.com",
      "author_name": "User",
      "author_contact_email": "user@example.com",
      "login_id": "",
      "avatar_url": "https://files.example.com/avatars/user/user@example.com/resized/256/",
      "user_name": "User",
      "user_email": "user@example.com",
      "details": [{ "repo_name": "Docs" }],
      "count": 1,
      "old_repo_name": null,
      "time": "2026-10-24T09:00:00+00:00"
    }
  ],
  "total_count": 1
}
```

头像 URL 是绝对地址，因为 Android 客户端直接加载它。`old_path`、`old_name`、`old_repo_name` 每个事件都发（seahub 按条件发）；Android 客户端要求重命名事件一定有 `old_name`。需要 `activity.read`。

一次同步提交算一个事件：其中的 `create`/`delete` 按对象类型分组，组内多于一个对象时用 `batch_create`/`batch_delete`，只有一个时用 `create`/`delete`。`count` 是本次操作的对象总数，而 `details` 最多返回 200 条，因此 `count` 可能大于 `details.length`。

**Seafile** —— 路由与外层结构相同。Nanofile 不记录评论或分享事件，动态里只有文件与资料库操作。

## 资料库共享 {#library-sharing}

资料库默认只有 owner 可见。拥有该资料库的**管理员**可以把可见性设为三种互斥模式之一：

| 可见性 | 谁可见 | 权限 |
| --- | --- | --- |
| `private` | owner | `rw` |
| `shared` | `repo_members` 里的账号 | 每个成员 `r` 或 `rw`（默认 `r`） |
| `public` | 所有启用账号 | 固定 `r` |

只有拥有该库的管理员能修改共享；不支持群组共享，也不支持目录级共享（`p` 必须是 `/`）。加密库只能是 `private`。这些是 Nanofile 对 Seafile 接口的扩展：官方客户端把 `shared` 显示为 `srepo`（“共享给我的”），把 `public` 显示为没有群组 id 的 `grepo`（“Shared with all”）。`related-users` 只列 owner 与显式成员，不会把公开库的全部账号列出来。

### 列出资料库的用户共享 {#get-api2-repos-repo-id-dir-shared-items}

`GET /api2/repos/{repo_id}/dir/shared_items/?p=/`

返回该库的用户共享，**顶层是数组**——官方桌面客户端共享对话框解析的形状。Nanofile 以前用这个路由列下载链接，现在链接由 `GET /api/v2.1/share-links/` 列出。

**响应 `200`**

```json
[
  {
    "share_type": "user",
    "user_info": {
      "name": "reader@example.com",
      "nickname": "Reader",
      "contact_email": "reader@example.com",
      "avatar_url": "https://files.example.com/avatars/reader@example.com/resized/72/"
    },
    "permission": "r",
    "is_admin": false
  }
]
```

`user_info.name` 是账号邮箱（桌面客户端把它当用户名），`nickname` 是显示名。需要 `library.read`，且只有管理员 owner 可以调用。

### 把资料库共享给用户 {#put-api2-repos-repo-id-dir-shared-items}

`PUT /api2/repos/{repo_id}/dir/shared_items/?p=/`

新增用户共享；已存在则替换权限。body 可以是桌面客户端发送的表单（`share_type=user&username=…&permission=r|rw`），也可以是同名字段的 JSON。`permission` 默认 `r`。把 `private` 库共享出去会同时变成 `shared`。

公开库不能再列用户（`400`），需先取消公开；加密库、owner 自己、未知/停用用户、非 `/` 的 `p` 都会被拒。需要 `library.update` 与管理员的 owner 身份。

**响应 `200`** —— `{"success": [<共享项>], "failed": [], "virtual_repo_id": null}`。

### 修改用户权限 {#post-api2-repos-repo-id-dir-shared-items}

`POST /api2/repos/{repo_id}/dir/shared_items/?p=/&share_type=user&username=<email>`

body `permission=r|rw`（表单或 JSON）。用户必须已在共享列表中，否则 `404`。需要 `library.update` 与管理员的 owner 身份。**响应 `200`** —— `{"success": true}`。

### 移除用户共享 {#delete-api2-repos-repo-id-dir-shared-items}

`DELETE /api2/repos/{repo_id}/dir/shared_items/?p=/&share_type=user&username=<email>`

删除授权；删掉 `shared` 库的最后一个成员后它会回到 `private`。幂等。需要 `library.update` 与管理员的 owner 身份。**响应 `200`** —— `{"success": true}`。

### 退出共享 {#delete-api2-beshared-repos-repo-id}

`DELETE /api2/beshared-repos/{repo_id}/?share_type=personal`

被共享者移除自己的授权；桌面客户端的 **Leave share** 会调用它。接受并忽略 `from=<owner>`。owner 不能退出自己的库（`400`）；非成员调用返回 `200` 且不做任何改动。需要 `library.update`。**响应 `200`** —— `{"success": true}`。

### 搜索用户 {#get-api2-search-user}

`GET /api2/search-user/?q=<pattern>`

桌面客户端共享对话框用它做用户补全。按邮箱子串匹配启用账号，最多返回 50 条，转义 `%`/`_`；空查询返回空列表而不是账号目录。需要 `admin.user.read` 且调用者是管理员。

**响应 `200`** —— `{"users": [{"email": "…", "name": "Reader", "contact_email": "…", "avatar_url": "…"}]}`。

### 设置资料库可见性 {#put-api-v2-1-repos-repo-id-visibility}

`PUT /api/v2.1/repos/{repo_id}/visibility/`

body（JSON 或表单）：`{"visibility": "private" | "shared" | "public"}`。

`private` 与 `public` 会在同一操作中清空已列用户；`shared` 保留（允许空列表）。加密库只能是 `private`。需要 `library.update` 与管理员的 owner 身份。这是 Nanofile 扩展，官方客户端没有对应接口。

**响应 `200`** —— `{"success": true}`。

## 列出群组 {#get-api2-groups}

`GET /api2/groups/`

桌面端的群组对话框在显示分享表单前会调用它。永远返回空数组：Nanofile 没有群组。

需要 `library.read`。

## 列出群组（v2.1） {#get-api-v2-1-groups}

`GET /api/v2.1/groups/?with_repos=<0|1>`

Android 的库列表加载链在列出资料库前会调用它；`404` 会让整条链失败。永远返回空数组。`with_repos` 只接受 `0` 或 `1`，其他值返回 `400`。需要 `library.read`。

## 智能链接 {#get-api-v2-1-smart-link}

`GET /api/v2.1/smart-link/?repo_id=<id>&path=<path>`

返回一个解析到文件当前下载接口的 URL，因此共享的「智能链接」总是指向最新版本。

**响应 `200`** —— `{"url": "https://files.example.com/api2/repos/<id>/file/?p=/notes.txt"}`。该 URL 需要读取者的凭据，不是公开链接。需要 `file.read`。

**Seafile** —— 官方参考还有基于令牌的后续接口（`/api/v2.1/smart-links/{token}/`），Nanofile 没有实现。
