# 用户与分享

## 用户

`adduser` 创建账号，默认为管理员；`--regular` 创建普通用户。

```bash
./target/release/nanofile adduser --email admin@example.com
./target/release/nanofile adduser --email someone@example.com --regular
```

默认从交互式提示读取密码。`--password-stdin` 从标准输入的第一行读取，`--password-file <路径>` 从文件读取；两者都能避免密码出现在 shell 历史与 `ps` 输出里。

`[auth]` 中可开启邀请码注册，由用户自行注册。

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
