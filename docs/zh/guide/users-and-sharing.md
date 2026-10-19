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

`share_link_enabled` 设为 `false` 后，匿名分享与上传链接全部失效；已存在的链接不再可用，也不会退化成登录访问。

## 加密资料库

加密资料库上传同样需要资料库密码，读取也一样。未提供时：

- 网页预览与下载返回 440，
- 匿名上传链接不可用于该资料库，也无法为它创建。

服务端设置见[安全](/zh/guide/security)，凭据与 API 密钥见[账号安全](/zh/guide/account-security)。
