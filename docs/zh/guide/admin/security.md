---
description: 主密钥、绑定地址、工作量证明挑战与凭据规则。
---
# 安全

**适合谁：** 把 Nanofile 暴露到可信局域网之外的管理员。

## HTTPS 与反向代理

反向代理负责终止 TLS 并转发请求。把 `server.site_url` 设为对外 HTTPS 地址；代理与服务器在同一台主机时，把服务器绑定到 `127.0.0.1`，并在 `server.trusted_proxies` 里列出代理。

nginx/Caddy 配置、Cloudflare Tunnel、受信代理的关键字与验证步骤见 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy)。

## 绑定地址

默认绑定 `0.0.0.0`。仅由反向代理访问时，改用 `127.0.0.1` 并在防火墙关闭对应端口。

## 受信代理

`trusted_proxies` 决定哪些对端地址提供的 `CF-Connecting-IP` / `X-Forwarded-For` 会被采信。未设置时这些头一律被忽略，所有客户端都会塌缩成代理地址，按 IP 的限流退化为同一个桶。可填的形式与示例见 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy#受信代理)。

## 工作量证明挑战

`auth.captcha_mode` 为匿名网页表单增加一次自托管的工作量证明（PoW）挑战。这是图片验证码的实用替代：当前的 AI 模型可以直接识别扭曲文字和物体网格，而自建视觉题需要自己的模型，却并不会真正更难解。

浏览器拿到随机种子和难度，搜索一个 nonce，使 `SHA-256(seed || nonce)` 达到足够的前导零位数。服务端重新计算哈希，只接受一次，并在十分钟后过期。挑战随页面下发、随表单 POST 校验，因此没有额外端点需要保护。

```toml
[auth]
# "off"（默认）、"adaptive" 或 "always"。
captcha_mode = "adaptive"
# 前导零位数：12–22，默认 17。
captcha_difficulty_bits = 17
# 自适应模式下，同一地址失败多少次后登录开始要求验证。
captcha_trigger_after = 2
```

- `off` 保持表单原样。
- `adaptive` 只在同一客户端地址登录失败 `captcha_trigger_after` 次后要求；开启时注册和密码重置始终要求。
- `always` 每次登录、注册和密码重置都要求。
- 难度随近期失败每次增加一位，最高 22 位。

挑战只保护**网页表单**。`POST /api2/auth-token/`、WebDAV、同步协议以及桌面/移动客户端不会收到挑战，因为它们无法运行浏览器求解器；这些路径继续使用现有的锁定、spray 检测和按 IP 限流。当页面显示挑战时，该表单需要 JavaScript；页面会通过 `<noscript>` 提示。

**局限。** 工作量证明不是人机区分器。它提高 naive 和流量型自动化的成本；有原生代码、GPU 或浏览器农场的攻击者仍然可以支付。速率限制和邮箱验证仍是主控制。如果公网部署面对有针对性的 AI 机器人，托管式风险验证（Cloudflare Turnstile 或 hCaptcha）会明显更强，但代价是外部网络调用、CSP 放行脚本/框架以及站点/密钥管理；那是独立的集成。对屏幕阅读器用户、禁用 JavaScript 的客户端或很慢的设备，优先使用 `off` 或 `adaptive`。

## 加密资料库

加密资料库上传同样需要资料库密码，读取也一样。未提供时，网页预览与下载返回 440，匿名上传链接不可用于该资料库，也无法为它创建。

## 凭据吊销

改密码、停用账号或远程擦除会吊销该账号持有的全部登录：其他设备、同步客户端、API 密钥，以及未使用的重置链接。见[账号安全](/zh/guide/use/account-security)。

## 主密钥

release 构建在未设置 `server.secret_key` 时拒绝启动。

零配置启动根本走不到这个报错：它会先进入首次初始化，生成强密钥并持久化。

debug 构建在**显式给出**配置却缺少密钥时仍会临时生成，因此会话无法跨重启保留。`NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` 让零配置启动也走这条临时密钥路径，仅用于本地与 CI。

密钥用于加密会话、邮件与静态块。不要把它写进镜像或仓库；生成与挂载方式见[Docker 部署](/zh/guide/deploy/docker)。

## 相关

- [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy) —— TLS、nginx/Caddy 与受信代理。
- [配置](/zh/guide/configure/configuration) — 优先级与设置页。
- [沙盒](/zh/guide/admin/sandbox) — 不可信解析的约束。
- [账号安全](/zh/guide/use/account-security) —— 用户侧的两步验证、会话与吊销。
