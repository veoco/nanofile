# 用户与分享

## 用户

`adduser` 创建账号，默认为管理员；`--regular` 创建普通用户。

```bash
./target/release/nanofile adduser --email admin@example.com
./target/release/nanofile adduser --email someone@example.com --regular
```

默认从交互式提示读取密码。`--password-stdin` 从标准输入的第一行读取，`--password-file <路径>` 从文件读取；两者都能避免密码出现在 shell 历史与 `ps` 输出里。

`[auth]` 中可开启邀请码注册，由用户自行注册。

### 注册模式

**系统设置 → 身份认证** 中的两个开关决定访客能否自行创建账号：

| 状态 | `auth.enable_registration` | `auth.require_invitation` |
|---|---|---|
| 关闭注册 | `false` | 任意 |
| 仅邀请码（默认） | `true` | `true` |
| 开放注册 | `true` | `false` |

对应环境变量为 `NANOFILE_AUTH_ENABLE_REGISTRATION` 与
`NANOFILE_AUTH_REQUIRE_INVITATION`。旧名 `auth.enable_invitations` 和
`NANOFILE_AUTH_ENABLE_INVITATIONS` 仍作为废弃别名兼容，已保存的设置行会在升级时迁移到新 key。

邀请码注册立即生效：邀请码本身就是凭据。**开放注册要求可用的邮件配置**：账号先以未验证状态创建，
服务器发送 3 天内有效的验证链接，打开链接前不能登录。没收到邮件时，可以用密码登录以重新发送
（5 分钟内最多一次）。邮件关闭、暂停或配置不完整时，注册页返回 404，登录页隐藏**创建账户**链接。
超过 7 天仍未验证的账号会被清理任务删除。

管理员仍可直接创建账号；管理员创建的账号、`adduser` 创建的账号、首次初始化管理员和邀请码注册的账号
都视为已验证。

## 分享链接

在网页文件浏览器选中文件或文件夹并选择**分享**，生成的链接支持：

- 访问密码，
- 过期时间，
- 查看次数。

匿名**上传链接**允许无账号上传到资料库。

分享有两个开关，位于**系统设置 → 安全 → 分享功能**：

| 方向 | 键 | 环境变量 |
|---|---|---|
| 分享下载（分享链接） | `server.share_download_enabled` | `NANOFILE_SERVER_SHARE_DOWNLOAD_ENABLED` |
| 分享上传（上传链接） | `server.share_upload_enabled` | `NANOFILE_SERVER_SHARE_UPLOAD_ENABLED` |

两者默认都是 `true`，修改后立即生效。关闭其中一个方向后，网页端与文件浏览器行菜单不再显示该方向的入口，该方向的公开页面关闭（已存在的 `/f/`、`/d/` 或 `/u/` 链接返回"链接已失效"，不会退化成登录访问），创建该方向的链接会被拒绝。另一个方向照常工作，"分享"页只列出仍开启方向的内容；两个方向都关闭时左侧"分享"入口消失，页面返回 404。管理员的"分享管理"始终可用，仍可删除失效链接。

桌面端与移动客户端只有一个能力位 `share-link-disabled`，它跟随分享下载：关闭分享下载期间，客户端隐藏整个分享菜单。关闭前已发出的上传链接在关闭后也会被拒绝使用。

旧版本的单开关 `share_link_enabled` / `NANOFILE_SERVER_SHARE_LINK_ENABLED` 仍会被读取，并同时关闭两个方向；已保存的值在升级时迁移到两个新键。

## 加密资料库

加密资料库上传同样需要资料库密码，读取也一样。未提供时：

- 网页预览与下载返回 440，
- 匿名上传链接不可用于该资料库，也无法为它创建。

服务端设置见[安全](/zh/guide/security)，凭据与 API 密钥见[账号安全](/zh/guide/account-security)。
