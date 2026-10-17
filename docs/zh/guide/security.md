# 安全

## HTTPS 与反向代理

部署在 HTTPS 反向代理后时，把 `server.site_url` 设为客户端实际使用的 HTTPS 地址。该设置决定：

- 会话 cookie 的 `Secure` 属性，
- 是否启用 HSTS，
- 生成分享链接时使用的地址。

会话、分享密码与 API 密钥都是持有即有效的凭据。用明文 HTTP 提供它们等于把它们暴露出去。

## 绑定地址

默认绑定 `0.0.0.0`。仅由反向代理访问时，改用 `127.0.0.1` 并在防火墙关闭对应端口。

## 受信代理

反向代理后需设置 `trusted_proxies`，它决定哪些对端地址提供的 `CF-Connecting-IP` / `X-Forwarded-For` 会被采信。未设置时这些头一律被忽略，所有客户端都会塌缩成代理地址，按 IP 的限流退化为同一个桶。

每项可填具体 IP、CIDR 网段，或关键字：`loopback`、`private`、`link-local`、`unique-local`。

### Cloudflare Tunnel

经 Cloudflare Tunnel（`cloudflared`）暴露时，源站看到的对端是回环或私有地址。设置 `trusted_proxies = ["private"]` 并把源站绑定到 `127.0.0.1`（或在 Docker 网络中隔离），Cloudflare 会通过 `CF-Connecting-IP` 提供真实访客 IP，其优先级高于 `X-Forwarded-For`。

只有在源站无法被其他来源访问时才可信任 `private`，否则同网段设备可伪造该头。

## 加密资料库

加密资料库上传同样需要资料库密码，读取也一样。未提供时，网页预览与下载返回 440，匿名上传链接不可用于该资料库，也无法为它创建。

## 凭据吊销

改密码、停用账号或远程擦除会吊销该账号持有的全部登录：其他设备、同步客户端、API 密钥，以及未使用的重置链接。见[账号安全](/zh/guide/account-security)。

## 主密钥

release 构建在未设置 `server.secret_key` 时拒绝启动。

零配置启动根本走不到这个报错：它会先进入首次初始化，生成强密钥并持久化。

debug 构建在**显式给出**配置却缺少密钥时仍会临时生成，因此会话无法跨重启保留。`NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` 让零配置启动也走这条临时密钥路径，仅用于本地与 CI。

密钥用于加密会话、邮件与静态块。不要把它写进镜像或仓库；生成与挂载方式见[Docker 部署](/zh/guide/docker)。

## 相关

- [配置](/zh/guide/configuration) — 优先级与设置页。
- [沙盒](/zh/guide/sandbox) — 不可信解析的约束。
