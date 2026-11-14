---
description: 排查启动、同步、搜索、媒体、邮件、上传与反向代理问题。
---
# 故障排查

从症状入手，不要先猜设置。下面的“最初五分钟”用于缩小范围，之后按主题展开。

**适合谁：** 负责这台服务器的人。**前置条件：** 能访问服务管理器、日志和数据目录。

## 最初五分钟

1. **进程还在吗？** `systemctl status nanofile`、`docker ps`，或 Windows 服务状态。如果它重启过，先看日志最后几行。
2. **还在服务吗？** `curl -fsS http://127.0.0.1:8082/health`。200 只说明监听还在，不代表数据库和每个功能都健康。
3. **读日志。** `journalctl -u nanofile -n 100`、`docker logs --tail 100 nanofile`，或 Windows 服务旁的 `nanofile.log`。
4. **看磁盘和权限。** 对数据文件系统跑 `df -h`，并确认服务用户能写数据目录。
5. **看最近改了什么。** 新代理、新证书、设置变更、升级或磁盘写满，能解释大多数突然故障。

## 初始化与启动

**启动后进了初始化页面，而不是直接服务。** 说明既没有配置文件、也没有任何 `NANOFILE_*` 变量，这是“尚未初始化”的状态。走完一次性页面，或自己放一个配置文件。见[安装与首次运行](/zh/guide/start/getting-started)。

**release 构建拒绝启动。** 缺 `server.secret_key`。生成一个并放进配置文件或 `NANOFILE_SERVER_SECRET_KEY`：`openssl rand -hex 32`。见[安全](/zh/guide/admin/security)。

**显式指定的配置文件不存在。** `--config` 或 `NANOFILE_CONFIG` 指向不存在的文件会拒绝启动，不会悄悄退回默认值。检查路径和服务 unit 的工作目录。

**端口起不来。** 别的进程占着 `server.port`；换端口或停掉那个进程。Windows 下刚关掉的连接会停留在 `TIME_WAIT` 几分钟，这也是 Windows 服务要等下次开机才接管的原因。

**数据目录不可写。** 运行服务的用户要对整个数据目录可读写；容器里是 uid/gid `1000:1000`。见[数据目录](/zh/guide/configure/data-directory)。

## 客户端与同步

**客户端连不上，或能登录但不同步。** 按[客户端接入](/zh/guide/use/clients)末尾的清单逐项查。最常见的是服务器地址与 `server.site_url` 不一致。

**HTTPS 失败或客户端拒绝证书。** 官方客户端不接受自签名证书。检查代理链、证书名称和系统时钟。见 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy)。

**两步验证循环。** 检查服务器时钟与验证器时钟；TOTP 基于时间。受信设备有效期为 90 天，可在**设置 → 会话与凭据**里吊销。见[账号安全](/zh/guide/use/account-security)。

**只有一台客户端受影响。** 吊销那台设备的会话或同步令牌，再重新登录。改密码会吊销该账号的全部同步令牌。

**上传好像每 30 秒重来一次。** 桌面客户端重试的是整个上传任务，而不是失败的那一个请求：任何一次失败都会结束任务，约 30 秒后从头再来。`400` 对客户端不是致命错误，所以确定性的拒绝会无限循环。服务端日志里会反复出现同一串 `permission-check → quota-check → check-fs → recv-fs → check-blocks`；状态码为 4xx/5xx 的那个请求才是要修的地方（权限被收回、缺块返回 `446`、超出大小限制），不要去查网络。

## 搜索与索引

**搜索搜不到刚上传的东西。** 先确认 `index.enabled` 是开的，再给那个文件点一次重新索引。索引是异步的，大文件会晚一点出现。

**PDF 或 Office 文档搜不到。** 文档提取在沙盒里运行。`[sandbox] enabled` 关着时，文档被记为可重试，重新打开后会被重新处理。见[沙盒](/zh/guide/admin/sandbox)。

**图片或扫描版 PDF 搜不到。** OCR 是另一个可选项。确认 `index.enabled` 与 `ocr.enabled` 都开着，且沙盒开着。见[图片与 PDF 文字识别](/zh/guide/use/ocr)。

**混合 PDF 仍缺扫描页。** 它是在开启 OCR 之前按文本层索引的；重建该资料库索引。之后才开启 OCR 只会重新检查被索引跳过的文件。

**整体很慢。** 调低 OCR 的 `max_side`、调高 `min_score`，或关闭 OCR；看任务管理里有没有排队任务。见[监控](/zh/guide/operate/monitoring)。

## 缩略图与媒体

**缩略图或预览不工作。** `[sandbox] enabled` 关着时这些功能一律不运行，也不会退回服务器进程内解析。见[沙盒](/zh/guide/admin/sandbox)。

**媒体缩略图失败。** 需要 `ffmpeg`，路径由 `storage.ffmpeg_path` 指定。保存路径会立即重新解析这份授权。

**HEIC/HEIF/AVIF 图片被跳过。** 它们在做人物检测或 OCR 前要经媒体 helper 解码。配置 `ffmpeg`；helper 可用后补扫会重试这些文件。

## 邮件

**邮件发不出。** 先确认 `email.enabled` 已开、SMTP 各项填对，再从**系统设置 → 邮件管理**发一封测试邮件。见[邮件通知](/zh/guide/configure/email)。

**邮件一直停在失败状态。** 在邮件队列或任务管理里看失败原因；邮件按 `email.max_attempts` 重试，超过次数后停在失败状态，等你在队列里重试或删除。

**开放注册返回 404。** 开放注册依赖可用的邮件。邮件关闭、暂停或配置不完整时，注册页返回 404，登录页隐藏创建账户链接。见[用户与分享](/zh/guide/admin/users-and-sharing)。

## 上传与配额

**上传失败。** 依次看：账号配额（[用户与分享](/zh/guide/admin/users-and-sharing)）、反向代理或客户端的请求体上限，以及存储上限（进行中的断点续传与单个压缩包都有默认上限，设为 `0` 取消，见[配置](/zh/guide/configure/configuration)）。

**单个大文件上传缓冲太多内存。** `server.max_chunk_size_mb` 限制单个断点续传分块（默认 32 MiB）。只有确实发送更大分块的客户端才需要调高。

**恢复或复制后丢了块。** 跑 `nanofile verify-blocks`；`--repair` 会把坏块隔离到 `{block_dir}/.quarantine/`，持有正确副本的客户端会重新上传。见[命令行](/zh/reference/cli)。

## 反向代理与客户端 IP

**反代之后所有人都是一个 IP。** 设 `trusted_proxies`，否则 `X-Forwarded-For` / `CF-Connecting-IP` 一律被忽略，按 IP 的限流退化成一个桶。见 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy)。

**大文件上传到固定大小就失败。** 代理的请求体上限小于文件；nginx 里设 `client_max_body_size 0`，其他代理找等价设置。

**WebSocket 通知循环重连。** 代理把 `Upgrade`/`Connection` 头剥掉了。

## 相关

- [监控](/zh/guide/operate/monitoring) —— 健康、任务、负载与告警。
- [维护](/zh/guide/operate/maintenance) —— 日志、垃圾回收与校验命令。
- [配置](/zh/guide/configure/configuration) —— 每个设置在哪、何时生效。
- [命令行](/zh/reference/cli) —— `verify-blocks`、`verify-dir-order`、`migrate-blocks`。
