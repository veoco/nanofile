---
description: 配置 SMTP、观察队列并排查投递问题。
---
# 邮件通知

**适合谁：** 配置密码重置、验证或通知邮件的管理员。

邮件默认不可用，需先配置 SMTP 并开启。

## 在哪里配置

SMTP 的每一项都是普通设置，在**系统设置 → 邮件管理**里改；页面上每一行还会标出当前值来自哪里（环境变量、已保存的设置、配置文件，或内置默认值）。环境变量的名字也写在行里。

常见项：

| 键 | 作用 |
|---|---|
| `email.enabled` | 总开关。关着的时候不投递，依赖邮件的注册页也会返回 404。 |
| `email.paused` | 暂停投递：邮件照常进队列，但先不发。 |
| `email.host`、`email.port` | SMTP 服务器地址与端口。 |
| `email.tls` | `starttls`、`tls` 或 `none`。选 `none` 时管理页会给出明文警告。 |
| `email.username`、`email.password` | SMTP 凭据。 |
| `email.from_address`、`email.from_name` | 发件人地址与显示名。 |
| `email.timeout_secs` | 单次连接超时。 |
| `email.max_attempts` | 一封邮件最多投递几次。 |

环境变量形式例如 `NANOFILE_EMAIL_ENABLED=1`、`NANOFILE_EMAIL_HOST=smtp.example.com`。

## 什么时候发信

- 密码重置链接；
- 开放注册的邮箱验证链接（未验证的账号不能登录）；
- 新建 API 密钥通知。

登录提醒按三个开关分别控制：`email.notify_new_login`（新登录）、`email.notify_new_device`（新设备）、`email.notify_api_key_created`（新建密钥）。

## 让密钥不出现在进程列表里

密码这类值可以用 `*_FILE` 变量从文件读取，例如 `NANOFILE_EMAIL_PASSWORD_FILE`。这样它们不出现在命令行与 `ps` 输出里。设置页会标明哪些键支持这种写法。

## 发信队列与排障

**系统设置 → 邮件管理**（`/sysadmin/email/`）是看发信状态的地方：

- **状态** —— 开关是否打开、投递是否就绪（缺哪几项会点名）、是否暂停。
- **测试邮件** —— 填一个收件地址发一封，验证配置最省事的一步。
- **发件箱** —— 待发、已发、失败各多少，清掉已完成的。
- **队列** —— 每封邮件的收件人、状态、已尝试次数，失败的可**重试**或删除。

投不出去的邮件会按 `email.max_attempts` 重试；超过次数后停在失败状态，等你在队列里处理。失败原因同时会出现在**系统设置 → 任务管理**，见[监控](/zh/guide/operate/monitoring)。

## 验证

1. 从**系统设置 → 邮件管理**向一个你能控制的地址发测试邮件；检查收件箱与队列。
2. 为一个测试账号请求密码重置；确认邮件到达且链接可用。
3. 如果开启了开放注册，注册一个测试账号并打开验证链接。

## 相关

- [配置](/zh/guide/configure/configuration) — 优先级与设置页。
- [账号安全](/zh/guide/use/account-security) — 登录提醒覆盖哪些事件。
- [用户与分享](/zh/guide/admin/users-and-sharing) — 开放注册需要可用的邮件。
