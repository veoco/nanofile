# Nanofile

[English](README.md) | [简体中文](README.zh-CN.md)

用 Rust 编写的、与 [Seafile](https://www.seafile.com/) 协议兼容的服务器。

Nanofile 实现了 Seafile 同步协议和 REST API，因此官方 Seafile 桌面 / 移动客户端以及
`seaf-cli` 等工具可以直接指向它。它还自带一套 Web UI（文件浏览器、分享、管理后台），
打包为单个静态二进制文件——无需安装独立的 `seaf-server` + `seahub` 组合。

## 功能特性

- **Seafile 同步协议**（`/seafhttp/`，协议版本 2）：内容寻址的 commit 和 FS 对象、带 SHA-1
  校验的块传输、打包的 FS 对象、`check-fs` / `check-blocks`、配额与权限预检查、每仓库同步
  token、文件锁定。
  - **安全说明**：`/seafhttp/repo/head-commits-multi/` 按协议要求是未认证的——官方桌面客户端
    调用它时不带任何凭据（seafile `daemon/http-tx-mgr.c`），官方服务器也不校验 token
    （`server/http-server.c`）；要求认证会导致桌面客户端静默地无法检测远程更新。暴露面有限：
    库 ID 是 128 位随机 UUID（盲枚举不可行），知道一个 ID 只能确认其存在并暴露 head-commit
    SHA，而没有仓库同步 token 时该 SHA 无法使用。该端点仍会拒绝非 UUID 的 ID，并将数组上限
    限制为 4096。
- **REST API**：旧版 v1（`/api2/*`）和 v2.1（`/api/v2.1/*`）接口，覆盖库、文件、目录、分享、
  动态、搜索、回收站、设备、头像等——与官方移动应用兼容。
- **API 密钥**：面向外部客户端的统一凭据，在*设置 → API 密钥*中管理。每个密钥持有一组细粒度
  能力（`file.read`、`share_link.write`、`sync.token`、`webdav.write` 等共 45 项，按领域分组），
  可绑定到指定资料库（每个库单独设置读写上限）或该账号可访问的全部资料库，并按可配置的有效期过期。
  内置同步客户端、WebDAV、CI 上传、只读等预设。写能力会自动包含对应的读能力；`admin.*` 需要管理员
  身份；密钥永远不能管理密钥。
- **凭据清单**：*设置 → 会话与凭据* 列出所有能长期访问本账号的凭据——客户端会话、浏览器会话
  （由 `User-Agent` 生成标签）、资料库同步令牌，以及免二次验证的设备——并可逐个吊销。API 密钥仍
  有自己的页面。同样的数据也可通过 `GET /api2/credentials/`（需要 `device.read`）与
  `DELETE /api2/credentials/{kind}/{id}/`（需要 `device.write`）取得；同步令牌的**值**从不包含在
  内，只有元数据。
- **WebDAV**（`/dav/...`）：使用上述密钥中的 `webdav.*` 能力认证，由 `webdav_enabled` 控制。
- **Web UI**：带预览和缩略图的文件浏览器、星标文件、动态流、回收站、设置（个人资料、会话与
  凭据、2FA、邀请、API 密钥），以及**系统管理后台**（用户、分享、后台任务）。支持中英文界面。
- **分享**：分享链接（可选密码 / 过期时间 / 浏览计数）、匿名上传链接、带 rw/r 权限的用户
  分享、自定义分享权限。全局 `share_link_enabled` 开关可完全禁用匿名分享 / 上传链接（已有
  链接将不可访问，并广播 `share-link-disabled` 特性让客户端隐藏分享功能）。
- **安全**：TOTP 双因素认证（含备用码和受信设备）、SSO / "在网站上查看" 登录、邀请码注册、
  带锁定机制的登录限流、密码重置（邮件门控）、哈希会话 cookie 与 CSRF 防护、防路径穿越的
  文件名处理。
  - **安全说明**：API、S2FA、SSO 登录和客户端登录的 bearer token 以 SHA-256 哈希存储，因此
    数据库泄露不会得到可用的凭据。同步 token 需要可回显（客户端会重新提交），因此以由
    `secret_key` 派生的 AEAD 密钥**加密**存储；分享链接 token 仍保持明文，因为"我的分享"列表
    会显示可复制的 URL——与官方 Seafile 的明文 URL 模型一致。**release** 构建下必须显式设置
    `NANOFILE_SERVER_SECRET_KEY` / `[server] secret_key`（否则拒绝启动，避免用临时密钥派生出
    无法跨重启解密的密钥）；debug 构建会自动生成。
- **加密库**：AES-256-CBC 块，带 Seafile 兼容的 `magic` / `random_key`，内存密码缓存带 TTL。
  - **安全说明**：存储的 `magic` 使用 PBKDF2-SHA256 在 **1000 次迭代** 下派生，这是 Seafile
    线上协议固定的，无法提高而不破坏客户端互操作。`magic` 是等价于密码的值，因此数据库泄露
    可让弱密码被离线暴力破解。请使用长而随机的库密码，并优先选择 **enc_version 4**（每库随机
    盐）而非 v2（固定全局盐）。服务器只接受 enc_version 2/4，并在创建时校验 magic/random_key
    格式。已知限制：`/api2/repos/` 创建 API 没有 `salt` 字段，因此通过它创建的库按 v2 派生——
    真正的每库 v4 盐是通过同步协议创建的。
- **存储与版本管理**：每用户配额、**按库命名空间**的内容寻址块存储
  （`data/blocks/repos/<sha1(repo_id)>/…`）、完整历史（含版本浏览 / 恢复）、每仓库历史
  限制与 TTL、垃圾回收（历史修剪 + 不可达 FS 对象清理）、带还原的回收站、已删除库恢复。
  可选的透明静态块加密（`block_encryption_mode`：`off` / `on` / `lazy`），块 id（逻辑字节的
  SHA-1）保持不变，因此 Seafile 客户端和内容寻址去重继续正常工作。
  - **回收站（库级）**：删除库时会保留其内容——提交图和 FS 对象在同一事务里被复制到
    `deleted_repo_commits` / `deleted_repo_fs_objects`（相当于官方服务端的 `deleted_store/`），
    块文件留在磁盘上。`POST /api/v2.1/deleted-repos/` 恢复该库（文件、历史、head 提交一并恢复）；
    `DELETE /api/v2.1/deleted-repos/{repo_id}/` 彻底删除单个库、`DELETE /api/v2.1/deleted-repos/`
    清空整个回收站，两者都会释放对应的块目录。网页端回收站页新增 **已删除的资料库** 标签页
    （恢复 / 彻底删除 / 清空库回收站），与"已删除的文件"标签页并列。**在本次构建之前**删除的库
    从未归档：回收站条目仍可恢复，但恢复出来是空库，服务器会在日志中说明原因。
  - **从旧版本升级**：块过去存在一个全局扁平的目录树里（`data/blocks/<2 位十六进制>/<id>`）。
    该布局只按内容 id 索引块，因此任何已认证用户都能借自己所属的库读取其他库的块。现在服务器会
    把每个被引用的块复制到拥有它的库下，然后删除旧目录树——在启动时、处理第一个请求之前自动完成。
    可用 `nanofile migrate-blocks --dry-run` 预估复制量，或用 `nanofile migrate-blocks` 在停机状态
    下手动执行。迁移只做复制（不使用硬链接），可断点续跑，并且只有在确认每个被引用的块都落到新位置
    后才删除旧目录树；如需回滚路径请先备份 `data/blocks`。去重现在按库进行，跨库重复的内容会在每个
    库各存一份，磁盘占用会相应增加。
- **全文搜索**：内置 Tantivy 索引，带 jieba 中文分词器；跨库的文件名和内容搜索。
- **实时通知**：WebSocket 推送仓库更新、文件锁定、文件夹权限和评论更新。
- **运维**：可续传 / 分块上传（`Content-Range` 组装）、zip 批量下载、带指标的后台调度器，
  可从管理后台手动触发。

## 架构

Nanofile 是一个包含四个 crate 的 Cargo workspace：

| Crate | 职责 |
|-------|------|
| `base` | 纯基础类型——`AppError`、路径 / 文件名净化、Seafile 存储格式类型和常量。除非启用 `with-axum` 特性，否则无 HTTP 依赖。 |
| `infra` | 基础设施——SeaORM 实体、内容寻址块存储后端、加密（AES / 密钥派生 / magic）、配置 + 环境变量覆盖、限流、数据库初始化。 |
| `server` | 应用本体——HTTP 处理器、服务、仓库、同步协议、WebDAV、WebSocket 通知、全文索引器、Askama Web UI。 |
| `migration` | SeaORM 迁移（从首次启动开始的 schema 演进）。 |

依赖方向：`base → infra → server`（编译期强制）；`migration` 被 `server` 使用。

### Web 前端

UI 是服务端渲染的（Askama），使用 Tailwind CSS 和以 ES 模块编写的模块化 JavaScript 前端：

```
server/frontend/
├── core/       # 纯函数（i18n、格式化、文件元数据、API 辅助）——无 DOM，可单元测试
├── browser/    # DOM 层（列表、选择、右侧面板、操作、上传、查看 …）
├── entries/    # esbuild 入口点（common.js、file-browser.js）
```

`server/build.rs` 将 `entries/` 打包为 `static/js/*.bundle.js`（esbuild），并把
`static/css/input.css` 编译为 `app.css`（Tailwind），然后 `rust-embed` 将两者嵌入二进制。
esbuild 是**必需**的；Tailwind 可选（见 [开发](#开发)）。

## 快速开始

```bash
# 1. 安装前端构建依赖——esbuild 必需；Tailwind 可选但推荐（没有它 UI 会无样式渲染）
npm install

# 2. 构建服务器（二进制名是 `nanofile`，不是 `server`）
cargo build --release -p server

# 3. 配置
cp config.toml.example config.toml   # 按需编辑——见下方"配置"

# 4. 运行
./target/release/nanofile
```

打开 `http://localhost:8082` 并登录。

需要一个管理员账号。可以在首次启动时通过 `config.toml` 中的 `[admin_init]` 自动创建
（或使用 `NANOFILE_ADMIN_INIT_EMAIL` / `NANOFILE_ADMIN_INIT_PASSWORD_FILE`），也可以用 CLI
创建：

```bash
./target/release/nanofile adduser --email admin@example.com
```

会交互式提示输入口令。想跳过提示可以改用管道或文件——用 `--password` 传参会让口令同时出现在
shell 历史与本机 `ps` 输出中：

```bash
printf '%s\n' 'secret123' | ./target/release/nanofile adduser --email admin@example.com --password-stdin
./target/release/nanofile adduser --email admin@example.com --password-file /run/secrets/admin
```

传入 `--regular` 可创建非管理员账号。

## 配置

设置从工作目录下的 `config.toml` 读取。可用 `--config <path>`（优先级最高）或
`NANOFILE_CONFIG` 环境变量覆盖路径。如果文件缺失，服务器回退到内置默认值，因此可以零配置
启动——通过 `NANOFILE_*` 环境变量提供所需内容即可。每个键也可以用 `NANOFILE_*` 环境变量
覆盖——随附的 `config.toml` 在每个键上方的注释中列出了确切的变量名（例如
`NANOFILE_DATABASE_URL`、`NANOFILE_SERVER_PORT`）。环境变量在运行时始终生效，且永远不会被
写回文件。

升级到新版本时，如果配置格式发生变化，`config.toml` 会在原地自动迁移（保留注释），并先备份为
`config.toml.bak`；在只读挂载上，迁移仅在内存中应用。

| 配置段 | 用途 |
|---------|---------|
| `[server]` | 绑定地址 / 端口、`site_url`（用于下载 / 分享链接和 cookie 的外部 URL——在 TLS 代理后请设为你的 HTTPS 域名）、最大上传大小、请求超时、CORS、WebDAV 开关、功能开关（`sso_enabled`、`file_search_enabled`、`share_link_enabled`、`tray`）、桌面客户端品牌定制（`desktop_custom_brand` / `desktop_custom_logo`）、受信反向代理（`trusted_proxies`）。 |
| `[database]` | SeaORM/SQLite 连接 URL（默认 `sqlite:data/nanofile.db?mode=rwc`）和连接池大小。 |
| `[storage]` | 块存储、临时、缩略图和头像目录，全局存储配额上限（`max_storage_bytes`，`0` = 不限）、视频缩略图的 ffmpeg 路径、可续传上传临时限制（`max_temp_uploads`、`max_temp_upload_bytes`、`temp_upload_ttl_hours`）、zip 归档上限（`max_zip_entries`、`max_zip_bytes`，`0` = 不限），以及透明静态块加密（`block_encryption_mode` / `encryption_key`）。 |
| `[auth]` | 密码哈希成本、token TTL、API 密钥有效期预设（`api_key_ttl_presets_days`）及其上限（`api_key_max_ttl_days`，`0` = 不限；非 0 时不允许创建永不过期的密钥）、登录锁定、邀请注册、密码策略，以及每 IP 限流（密码重置、注册、TOTP 验证、分享 / 上传链接密码、匿名分享下载）。 |
| `[ui]` | 默认 UI 语言（`en` / `zh`）、托盘菜单语言（`tray_language`：`auto` 跟随系统区域设置，`en`/`zh` 强制指定）。 |
| `[email]` | 邮件后端总开关。密码重置链接只投递到所有者的收件箱，服务器从不回显，因此在存在 SMTP 后端之前重置流程保持禁用。 |
| `[admin_init]` | 可选的首次启动管理员自动创建。密码优先使用 `NANOFILE_ADMIN_INIT_PASSWORD_FILE`。 |
| `[logging]` | 日志级别、可选轮转日志文件（`file_enabled`、`file`、`max_file_size_mb`、`max_backups`）。 |
| `[gc]` | 启用 / 调度垃圾回收。 |
| `[index]` | 全文搜索开关（`enabled`）和索引目录。 |
| `[notification]` | WebSocket 通知设置和 JWT 私钥，以及连接上限（`max_connections`、`max_connections_per_ip`）和未认证连接的订阅超时（`subscribe_timeout_secs`）。 |
| `[tasks]` | 最大并发后台复制 / 移动任务数（`max_active_tasks`，`0` = 不限；超出请求返回 HTTP 429）。 |

`secret_key` 是唯一主密钥：通知密钥和 CSRF 签名密钥都由它派生。生产环境请用
`openssl rand -hex 32` 生成唯一值，并通过 `NANOFILE_SERVER_SECRET_KEY` 设置（空值会在启动时
自动生成随机密钥，这会使重启后会话失效）。

## 安全

单机部署下服务器已给出安全默认值，但有几项取决于你的运行方式：

- **在 nanofile 前面终止 TLS，并把 `site_url` 设为 HTTPS 地址。** 这一个设置同时决定会话/链接
  cookie 的 `Secure` 属性和 `Strict-Transport-Security`；保持纯 HTTP 时两者都不会下发（局域网
  部署不应被钉死在它无法提供的 HTTPS 上）。会话、分享链接口令与 API token 都是持有即有效的凭据。
- **文件块按资料库分开存储**（`data/blocks/repos/<sha1(repo_id)>/…`），每次读写块都必须指明所属
  资料库。因此块 id 只能通过调用者仍是成员的那个资料库访问：被移除的协作者即使在本服务器上还有
  其他资料库，也无法读取客户端之前从已失去的资料库缓存下来的块。这也意味着去重是按资料库进行的，
  而不是全服务器共享。
- **已删除的资料库在回收站条目被彻底清除前仍占用磁盘。** 垃圾回收不会回收仍列在回收站中的资料库
  的块——这正是"恢复后文件依旧可下载"的前提。要释放空间请彻底删除该资料库（或清空回收站）。
- **`addr = "0.0.0.0"` 是默认值**，因此监听主机的所有接口。若只有反向代理需要访问，请改为
  `127.0.0.1` 并用防火墙关闭端口。
- **放在反向代理后请设置 `trusted_proxies`。** 只有当 TCP 对端在列表内时才会采信
  `X-Forwarded-For`，否则外部无法伪造客户端 IP 绕过按 IP 限流。
- **`share_link_enabled = false`** 可完全关闭匿名分享/上传链接（既有链接立即失效，已签发的上传链接
  token 不能再换取上传 URL，已签发的链接 token 也停止接收上传）。`site_url` 未设置时，
  `allowed_hosts` 用于固定生成绝对下载 URL 的 Host；该列表为空时只回显字面地址
  （`192.168.1.20`、`[fe80::1]`、`localhost`），因为这类 URL 携带 capability token，而 `Host`
  头里的 DNS 名称是攻击者可控的。
- **加密资料库的写入（而不只是读取）同样需要资料库口令。** 在所有 HTTP 上传路径（包括断点续传）
  上，客户端未先调用 `?op=setpassword` / `set-password/` 之前都会收到 440（"需要资料库口令"，正是
  Android / iOS 据此重试的状态码），随后写入的块会用以缓存密钥加密后的密文保存。匿名上传链接既不
  能为加密资料库创建，也不能用于加密资料库：匿名访问者没有可缓存密钥的身份。同步协议不变（客户端
  本地加密）。
- **账号处置是彻底的。** 改密/重置口令、停用账号、远程擦除都会吊销该账号持有的全部凭证——会话
  token、API 密钥（含 WebDAV 密钥）、资料库同步 token *以及*内存中的 `/download-api/…`、
  `/upload-api/…`、`/blks/…` capability URL；改密/重置还会作废未使用的口令重置链接。把成员移出
  资料库会吊销其同步 token 与该资料库的 capability URL。
  - 绑定到某资料库的密钥会在其持有者失去成员身份的那一刻停止工作，因为每次请求都会重新校验成员
    身份；绑定关系本身会保留，所以重新加回成员后原有密钥即可恢复。删除资料库会一并清理只绑定到
    它的密钥。
  - API 密钥无法访问 `/api2/api-keys/`：能签发密钥的密钥就能给自己扩权。管理密钥必须使用浏览器
    会话。
- **每个长期凭据都可见、可单独吊销。** 持有者看不见的凭据就是无法吊销的凭据——默认寿命 365 天的
  资料库同步令牌过去正是如此：设备页只列客户端会话，而同步令牌和 90 天的 2FA 设备信任完全没有
  读取入口。现在账号 token 会记录**自己从哪来**（`api_tokens.source`），而不再从 `platform`
  推断——后者只在客户端上报设备信息时才存在，于是浏览器会话、什么都不上报的客户端、桌面客户端
  的"在网站上查看"，三者此前无法区分，而没有上报 `platform` 的客户端更是哪里都不显示。清单只
  展示同步令牌的元数据；它存储的值是服务端可解密的密文，因此从不参与序列化。
- **同一张路由表对所有凭据分类，因此新增端点默认关闭。** 每个请求先解析为一个 `Credential`——
  浏览器/客户端会话、统一 API 密钥，或资料库同步 token——路由表对三者都生效，而不只对密钥生效。
  会话本身就是账号，因此满足全部能力，但它同样要经过分类；密钥只拿到自己持有的能力，并按资料库
  绑定进一步收紧；而表中未登记的路由对所有人一律拒绝。表的缺口会按路由各报告一次，并**让端到端
  测试失败**（`e2e/global-teardown.ts` 从服务端日志里读回），所以忘记分类新端点会得到一个红的构建，
  而不是一扇静默敞开的大门。
- **上传与下载在写入前就已记账。** 尚未被任何 commit 引用的块字节会先记在调用者的配额上，因此
  "只写块不提交"是被约束的而不是免费的；提交时释放预留，被回收的废弃上传也会释放预留——回收会删除
  该上传写入的块，但删除前会复查没有任何 FS 对象引用它们，因此绝不会删掉已提交文件仍需要的块。
- **目录不能被移动或复制进自己的子树。** 树更新是先删后加，子树内的目标会被第一次提交销毁；因此
  移动与复制都在入口处拒绝，与 WebDAV 行为一致。
- **桌面客户端"在网站中查看"的 URL 是一个经过校验的重定向。**
  `/library/{repo-id}/{repo-name}/…` 是 seahub 的写法（第二段名称只是修饰）；它现在会重定向到本
  服务器自己的 `/libraries/{id}/files/…` 而不是 404，因此桌面端登录后回跳的 `next`
  （`repo-tree-view.cpp:578`）能真正落到该资料库。重定向只由通过 id 字符集校验的资料库 id 和
  在资料库内部归一化后的路径拼成，并按段重新百分号编码——两者中的 `%0d%0a` 都无法注入响应头或把
  浏览器带离本站。
- **用尽量精简的 `PATH` 运行服务器。** 辅助程序（视频缩略图的 `ffmpeg`、托盘动作的
  `xdg-open`/`launchctl`）是通过 `PATH` 查找的；请把 `storage.ffmpeg_path` 指向绝对路径，并避免把
  不可信目录（全局可写的工作目录、`node_modules/.bin`）放进服务器的 `PATH`。
- **用户较多时请收紧示例中的有限上限**（`max_zip_bytes`、`max_temp_upload_bytes`）；`0` 表示
  不限制。
- **release 构建在密钥为临时或占位值时会拒绝启动**：开启块加密时使用临时 `secret_key` 会导致每次
  重启后已存块永久不可读，配置的密钥形如占位符时同样拒绝。仅本地/CI 可用
  `NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` 覆盖。

以下是刻意保留并写入文档的取舍（每条都有其他机制兜底，并非无人看管）：

- **加密资料库口令。** 加密资料库的密钥派生迭代次数由同步协议固定为 1000：官方客户端自行派生
  数据密钥，改动会导致它们的资料库无法解密。`encrypted_library_pwd_hash_algo` /
  `encrypted_library_pwd_hash_params` 可以对新资料库提高**服务端校验哈希**的代价（桌面端会读
  取这两个字段；移动端只支持协议版本 ≤ 2），但默认值保持兼容。在线猜测由
  `repo_password_max_per_hour` 限制。
- **zip 下载（`/zip/{token}`）属于 capability URL**，与上游 fileserver 的 token 模型一致：一次性、
  有 TTL、不可猜测、日志中已脱敏。与上游不同的是，消费该 token 时会重新校验请求者对该资料库的权限，
  因此权限被回收（或账号被停用）的用户即使仍在 token 的一小时 TTL 内也无法再拉取压缩包。请仍把
  zip 链接当作口令对待。
- **`head-commits-multi` 与 `check_blocks`** 对匿名/已认证调用方的响应与上游一致（资料库元数据
  与块存在性）。它们是同步协议必需的，仅通过限流约束请求速率。
- **未提交上传的记账在内存中。** 尚未被任何 commit 引用的块所占的配额预留（`QuotaCache`）在启动时
  是空的，因此上传途中重启会忘掉这笔预留，上传者可能以"在途字节"为上限短暂超出配额，直到废弃的
  上传被回收。已提交的用量是持久化的并会重新读取，所以无法靠重启累积数据。
- **只有开启 GC 才会回收孤儿块。** `gc.enabled` 默认为 `false`，因此最后一块分片之前被放弃的上传
  留下的块会一直占磁盘（服务器启动时会告警）。这些字节仍计入配额，所以这是磁盘占用问题而不是绕过；
  需要回收请启用 `[gc]`。
- **被移除的协作者需要重新登录才能继续同步。** 取消共享（以及停用账号、改密、远程擦除）会删除数据库
  中的同步 token，因此该客户端的下一次 `/seafhttp/` 调用会被拒绝，必须重新认证。这是刻意的——保留
  token 就等于继续通过它提供该资料库——也与官方服务器在权限被回收时的行为一致。
- **Web UI 不提供加密资料库的解锁。** 目录与文件名可以浏览（Seafile 只加密文件内容，不加密 FS 树与
  commit），但浏览器里的预览、下载与上传都会收到 440（`RepoPasswdRequired`），因为只有 API 与官方
  客户端才能把资料库口令交给服务器，UI 里没有口令输入界面。UI 会把它标记为加密，而不是让它看起来
  只是不可用。
- **`validate_origin` 仍接受没有 `Origin`/`Referer` 的请求。** 登录、注册、口令重置表单也会被非浏览器
  调用（curl、集成测试）提交，因此不能把头缺失当作敌意。攻击者的浏览器一定会带上该头，代码在它存在时
  会校验；而需要认证的状态变更端点还额外要求与会话绑定的 CSRF token。
- **断点续传只以 `(repo_id, path)` 为键。** 同一资料库的两个可写用户可能在临时上传表里撞车并互相
  干扰对方的续传状态。影响范围限于单个资料库（不泄露跨用户数据，键也不含任何秘密），为每个上传调用点
  贯穿一个 user id 的改动被判定不值得。
- **配置中的密钥在内存里是普通字符串**（服务器主密钥、通知密钥、静态加密密钥、数据库 URL、
  管理员口令），进程退出前不做清零；而 token/TOTP/块加密所用的派生密钥会被清零。要清理常驻
  配置需要把密钥类型贯穿整个配置结构，在威胁模型下收益有限（能读进程内存的攻击者已经拿下了
  运行中的服务器）。

## 日志

无头运行（服务器、Docker、CLI 子命令）照常输出到 stdout，由 `[logging] level`（或
`NANOFILE_LOG_LEVEL`）控制。

桌面（托盘）运行改为输出到大小受限的轮转文件：

- 默认位置是 **nanofile 二进制旁边** 的 `nanofile.log`；首次运行时解析出的绝对路径会写回
  `config.toml`（`[logging] file`），因此登录启动的实例无论工作目录如何都使用同一文件，你
  也可以在那里修改。
- `[logging] file` 接受显式路径；相对路径相对于二进制所在目录解析（绝不使用工作目录，这对
  自动启动的实例没有意义）。
- `max_file_size_mb`（默认 10）限制每个文件大小；超过后轮转为 `nanofile.log.1`、`.2`、…，
  保留 `max_backups`（默认 3）个旧文件。`max_backups = 0` 表示原地截断。
- `file_enabled = true/false` 强制文件 / stdout 输出；未设置表示自动（桌面模式用文件，否则用
  stdout）。如果日志文件无法打开（例如二进制目录只读），服务器回退到工作目录，再回退到
  stdout。

## 系统托盘（可选）

名称以 `-tray` 结尾的发布归档（Windows / macOS / Linux-amd64）包含可选系统托盘图标，通过
`tray` 特性编译进去。普通构建完全不包含托盘代码，因此没有桌面的服务器不受影响。

右键点击托盘图标会打开一个菜单（翻译为系统语言——中文系统显示中文菜单；可用 `[ui]` 中的
`tray_language = "en"/"zh"` 强制语言）：

- **打开 Web UI**——在默认浏览器中打开 `site_url`
- **开机自启**（可勾选）——为当前用户注册 / 注销自启动：
  - Windows：`HKCU\...\CurrentVersion\Run` 注册表值（无需管理员权限）。当服务器以提升权限
    （"以管理员身份运行"）运行时，注册前会弹窗确认，因为该条目属于提升后的账户；登录启动的
    实例始终以非提升权限运行。
  - macOS：`~/Library/LaunchAgents/com.nanofile.nanofile.plist` 下的 LaunchAgent
  - Linux：`~/.config/autostart/nanofile.desktop` 下的 XDG 自启动条目（GNOME 和 KDE）
- **打开配置文件**——在资源管理器 / Finder / 文件管理器中显示实际使用的配置文件
- **退出**——触发与 Ctrl+C 相同的优雅关闭

自启动条目始终指向正在运行的二进制，并传入 `--config <绝对路径>`，因此自动启动的实例无论
工作目录如何都使用同一配置。

注意事项：

- 用 `[server]` 中的 `tray = false`（或 `NANOFILE_SERVER_TRAY=false`）关闭托盘，例如用于应保持
  不可见的自动启动实例。
- 在 Linux 上托盘需要桌面会话；没有 `DISPLAY`/`WAYLAND_DISPLAY`（或桌面会话损坏）时，服务器
  记录警告并以无头方式运行，而不是失败。
- GNOME 只有安装了 "AppIndicator and KStatusNotifierItem Support" 扩展才显示托盘图标；KDE
  Plasma 开箱即用。
- 在 Windows 上，`-tray` 构建是 GUI 子系统二进制：永远不会出现控制台窗口（双击、自启动或
  终端）。日志写入轮转日志文件（见"日志"）。`--version` 或 `adduser` 提示等纯 CLI 输出只有
  从终端启动时才可见（二进制会重新附加到终端，但 `cmd` 不会等待进程）——如需完整控制台，
  请使用普通（非 `-tray`）构建，其行为与之前完全一致。

自行构建托盘变体（Linux 还需要 `libgtk-3-dev` 和 `libayatana-appindicator3-dev`）：

```bash
cargo build --release -p server --features tray
```

托盘图标和 Windows 可执行文件图标在编译时从 `server/static/img/favicon.svg` 栅格化——仓库中
不附带任何图片资源。

## Docker

发布镜像是一个 `scratch` 容器，只包含 `nanofile` 二进制——没有配置文件或数据目录。容器以
`1000:1000` 运行，因此数据卷必须对该用户可写（从旧版本升级时执行
`chown -R 1000:1000 ./data`，或用 `--user "$(id -u):$(id -g)"` 对齐你自己的账号）。挂载一个
配置文件和一个持久化数据卷，并将数据路径指向该卷：

```bash
mkdir -p data
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -v "$PWD/config.toml:/etc/nanofile/config.toml:ro" \
  -e NANOFILE_CONFIG=/etc/nanofile/config.toml \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_INDEX_INDEX_DIR=/data/index \
  -e NANOFILE_SERVER_SECRET_KEY="$(openssl rand -hex 32)" \
  ghcr.io/<owner>/nanofile:latest
```

或者完全不使用配置文件——内置默认值填充其余部分，其他一切来自环境变量：

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(openssl rand -hex 32)" \
  ghcr.io/<owner>/nanofile:latest
```

## CLI

```
nanofile [--config <path>]          启动服务器（默认）
nanofile [--config <path>] adduser  创建用户（默认管理员；--regular 创建普通用户）
                                    口令：默认交互式输入，也可用
                                    --password-stdin / --password-file <path>
nanofile [--config <path>] migrate-blocks [--dry-run]
                                    把旧的全局扁平块目录树迁移为按库布局
                                    （服务器启动时也会自动执行；--dry-run 只报告不落盘）
```

## 数据布局

所有状态都位于工作目录下（显示默认值）：

```
data/
├── nanofile.db        # SQLite 数据库（WAL 模式，文件权限 0600）
├── nanofile.db-wal    # WAL 日志
├── blocks/            # 内容寻址块存储：repos/{sha1(repo_id)}/{2 位十六进制前缀}/{40 位 SHA-1}
├── temp/              # 可续传 / 分块上传暂存
├── thumbnails/        # 生成的图片 / 视频缩略图缓存
├── avatars/           # 用户头像图片
└── index/             # Tantivy 全文搜索索引
```

## 开发

前端构建作为 `cargo build` 的一部分运行（见 [Web 前端](#web-前端)）：

- **esbuild** 将 `frontend/entries/*.js` 打包为 `static/js/*.bundle.js`。它是必需的——如果
  esbuild 不在 `PATH` 或 `node_modules/.bin` 中，构建会 panic。用 `npm install` 安装。
- **Tailwind** 将 `static/css/input.css` 编译为 `app.css`。它是可选的——如果 Tailwind CLI 不可
  用，构建仍会成功，但 UI 会无样式渲染。

`build.rs` 通过 `rerun-if-changed` 跟踪 `frontend/`、`static/css/` 和 `templates/`，因此编辑
前端源码会在下次 `cargo build` 时触发重新打包。没有热重载——资源嵌入在二进制中，因此需要
重新构建才能应用前端改动。

## 测试

测试分为三层：

| 层 | 命令 | CI 任务 |
|-------|---------|--------|
| Rust 单元 + 集成 | `cargo test --workspace` | `test` |
| 前端单元 | `node --test "server/frontend/**/*.test.js"`（零依赖 `node:test`） | `frontend-test` |
| 浏览器端到端 | `cd e2e && npm install && npx playwright install --with-deps chromium && npx playwright test` | `e2e` |

Playwright 套件会启动一个真实的 `nanofile` 二进制，使用隔离的临时数据库，并在 Chromium 中驱动
UI，覆盖登录、选择、视图切换、排序 / 过滤、上传、文件操作、分享、历史、预览、标签和搜索。
失败的运行会在 `e2e/test-results/server.log` 捕获后端日志。

CI 还强制格式化和 lint 检查：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## CI 与发布

- **`ci.yml`**（push / PR 到 `main`、`master`、`develop`）：格式化、clippy（`-D warnings`）、
  前端单元测试、Playwright e2e 和 Rust 测试套件。
- **`nightly.yml`**（每日 / 手动）：多架构发布构建（Linux amd64/arm64/loong64 × gnu/musl、
  macOS arm64、Windows amd64），并向 `ghcr.io` 发布 OCI 镜像（`:edge`、`:sha-<sha>`）。
- **`release.yml`**（tag `v*.*.*` / 手动）：相同的多架构构建，外加带自动生成变更日志的 GitHub
  发布和带版本号的镜像（`:latest`、`:vX.Y.Z`、`:vX.Y`）。

## 许可证

MIT
