# 邮件通知

邮件通知默认不可用，需先配置 SMTP。配置位置有两处：

- **网页界面：**"系统设置 → 邮件"——填写主机、端口、账号、密码、发件人，然后开启 `enabled`。
- **环境变量：**例如 `NANOFILE_EMAIL_ENABLED=1`、`NANOFILE_EMAIL_HOST=…`。覆盖某个设置的 `NANOFILE_*` 变量名都写在对应的设置页上。

启用后发送的邮件包括：

- 密码重置链接，
- 新设备/新浏览器登录提醒，
- 新建 API 密钥通知。

## 让密钥不出现在进程列表里

密码等密钥类值可通过 `*_FILE` 环境变量从文件读取，例如 `NANOFILE_EMAIL_PASSWORD_FILE`。这样它们不会出现在命令行与进程列表中。

## 相关

- [配置](/zh/guide/configuration) — 优先级与设置页。
- [账号安全](/zh/guide/account-security) — 登录提醒覆盖哪些事件。
