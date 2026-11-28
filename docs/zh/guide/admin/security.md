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
- 自适应模式下难度在配置的基数上最多增加三位（默认 17 升到 20），且不会超过 22 位的上限。

挑战只保护**网页表单**。`POST /api2/auth-token/`、WebDAV、同步协议以及桌面/移动客户端不会收到挑战，因为它们无法运行浏览器求解器；这些路径继续使用现有的锁定、spray 检测和按 IP 限流。当页面显示挑战时，该表单需要 JavaScript；页面会通过 `<noscript>` 提示。

**局限。** 工作量证明不是人机区分器。它提高 naive 和流量型自动化的成本；有原生代码、GPU 或浏览器农场的攻击者仍然可以支付。速率限制和邮箱验证仍是主控制。如果公网部署面对有针对性的 AI 机器人，托管式风险验证（Cloudflare Turnstile 或 hCaptcha）会明显更强，但代价是外部网络调用、CSP 放行脚本/框架以及站点/密钥管理；那是独立的集成。对屏幕阅读器用户、禁用 JavaScript 的客户端或很慢的设备，优先使用 `off` 或 `adaptive`。

## 加密资料库

加密资料库上传同样需要资料库密码，读取也一样。未提供时，网页预览与下载返回 440，匿名上传链接不可用于该资料库，也无法为它创建。

## 凭据吊销

改密码或停用账号会吊销该账号持有的全部登录：其他设备、同步客户端、API 密钥，以及未使用的重置链接。远程擦除的范围更窄：只吊销被擦除设备自己的会话、同步令牌与两步验证设备信任。见[账号安全](/zh/guide/use/account-security)。

## 主密钥

release 构建在未设置 `server.secret_key` 时拒绝启动。

零配置启动根本走不到这个报错：它会先进入首次初始化，生成强密钥并持久化。

debug 构建在**显式给出**配置却缺少密钥时仍会临时生成：每次启动都换一把新密钥，由它加密的数据（同步令牌、TOTP 种子、队列邮件与已保存的密钥类设置）因此不可读，启动时的指纹校验还会清掉所有已存会话——会话不会跨重启保留。若启用了静态块加密，服务端会拒绝启动而不是生成临时密钥——那会让所有已存块永久无法解密。`NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` 让零配置启动也走这条临时密钥路径，仅用于本地与 CI。

主密钥派生：CSRF 签名密钥、同步令牌的加密密钥、TOTP 加密密钥、邮件队列密钥、通知 JWT 密钥（未显式配置时），以及静态块密钥（未设置 `storage.encryption_key` 时）。会话与账号 token 是随机值、库里只存 SHA-256 摘要，因此不由该密钥加密；服务端在启动时把密钥指纹记入数据库，指纹变化时删除所有已存会话。所以轮换主密钥会让所有浏览器与 API 会话退出登录。API key 同样只存哈希，但不与指纹绑定：若怀疑主密钥泄露，请显式吊销。轮换还会打断同步客户端，并使 TOTP 种子、队列邮件与已保存的密钥类设置无法解密；只有块密钥由它派生时，加密块才受影响。

不要把它写进镜像或仓库；生成与挂载方式见[Docker 部署](/zh/guide/deploy/docker)。

## 静态加密块存储 {#at-rest-block-encryption}

`storage.block_encryption_mode`（只能写在文件里）决定块在磁盘上怎么存：

| 模式 | 新写入 | 读取 |
|---|---|---|
| `off`（默认） | 明文 | 按原样读出 |
| `lazy` | 密文 | 两者都接受 |
| `on` | 密文 | 只接受密文 |

开启是一次性的**转换**，只有转换完成后 `on` 才安全：

1. **准备好密钥。** `storage.encryption_key` 来自 `NANOFILE_STORAGE_ENCRYPTION_KEY`/`_FILE`；都没设置时由 `server.secret_key` 派生。无论哪种，密钥都必须跨重启保持不变：解不开自己块的服务器等于没有数据。见[主密钥](#主密钥)。
2. **把 `storage.block_encryption_mode` 设为 `"lazy"` 并重启。** 服务器会启动**块加密迁移**作业：逐块读一遍，把明文块（以及旧版本写下的无 header 密文块）按同一个 id 重写为带版本头的密文，并报告检查了多少块。它分批执行、在服务器繁忙时暂停，每完成一个库就把进度记到磁盘，重启或取消后从那里继续。它出现在**任务管理**里，不是启动迁移。
3. **等转换完成。** `nanofile convert-blocks --status` 会打印状态，未完成时退出码非 0；完成标记是 `{block_dir}/.encryption_converted`。
4. **把 `storage.block_encryption_mode` 设为 `"on"` 并重启。**

`on` 读不了转换前的明文，过早切换会让已有文件不可读。因此块目录里已经有库、却没有完成标记时，服务器在 `on` 模式下拒绝启动；空块目录会自动记为已转换。停机执行 `nanofile convert-blocks` 可以补完或校验转换并写下标记——一直以 `on` 模式运行的部署只需要跑一次。

转换之后不要再回到 `off`：磁盘上是密文，`off` 会把它们当作文件内容发出去。若从备份把明文块恢复进已转换的块目录，需要删掉标记并重跑转换。

## 相关

- [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy) —— TLS、nginx/Caddy 与受信代理。
- [配置](/zh/guide/configure/configuration) — 优先级与设置页。
- [沙盒](/zh/guide/admin/sandbox) — 不可信解析的约束。
- [账号安全](/zh/guide/use/account-security) —— 用户侧的两步验证、会话与吊销。
