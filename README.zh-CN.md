# Nanofile

[English](README.md) | [简体中文](README.zh-CN.md)

Nanofile 是一个可自行部署的文件同步与分享服务。它兼容 Seafile 的同步协议与接口，官方桌面端、移动端客户端可直接连接；同时自带网页界面，无需另装 seaf-server + seahub。

## 功能

- **文件同步**：与官方 Seafile 服务器一致，桌面端与移动端客户端自动同步资料库。
- **网页端**：浏览、预览、下载、收藏，以及动态与回收站。
- **分享**：分享链接支持密码与过期时间；支持匿名上传链接。
- **登录安全**：两步验证（TOTP）、受信设备、登录失败锁定。第三方工具（WebDAV、同步客户端）可用 API 密钥接入，无需共享账号密码。
- **管理后台**：用户、配额、邮件与系统设置。
- **其他**：资料库加密、全文搜索（含中文分词）、历史版本与回收站、后台垃圾回收。

## 快速开始

从源码构建：

```bash
npm install                       # 前端打包工具（esbuild 必需）
cargo build --release -p server   # 产物是 nanofile（不是 server）
cp config.toml.example config.toml
./target/release/nanofile
```

服务监听 http://localhost:8082。首次启动需要一个管理员账号，可用 CLI 创建：

```bash
# 交互式输入密码
./target/release/nanofile adduser --email admin@example.com
# 或免交互
printf '%s\n' 'secret123' | ./target/release/nanofile adduser --email admin@example.com --password-stdin
```

`--regular` 创建普通用户账号。预构建容器镜像见「用 Docker 部署」。

## 使用

### 用户
`adduser` 创建账号，默认为管理员；`[auth]` 中可开启邀请码注册，由用户自行注册。

### 分享
在网页文件浏览器选中文件或文件夹并选择"分享"，生成分享链接，可设访问密码、过期时间与查看次数。匿名上传链接允许无账号上传到资料库。`share_link_enabled` 设为 false 后，匿名分享与上传链接全部失效，已存在的链接不再可用。

### 邮件通知
邮件通知默认不可用，需先配置 SMTP。配置位置有两处：网页后台"系统设置 → 邮件"（填写 host、端口、账号、密码、发件人等并开启 `enabled`），或环境变量（如 `NANOFILE_EMAIL_ENABLED=1`、`NANOFILE_EMAIL_HOST=…`）。

启用后发送的邮件包括：密码重置链接、新设备/新浏览器登录提醒、新建 API 密钥通知。密码类值可通过 `*_FILE` 环境变量从文件读取（如 `NANOFILE_EMAIL_PASSWORD_FILE`），不出现在命令行与进程列表中。

### 账号安全
"设置 → 安全"开启两步验证并保存备用码；"设置 → 会话与凭据"列出账号当前登录的设备、资料库同步令牌与 API 密钥，可逐个吊销。改密码会吊销其他设备的会话、同步令牌与全部 API 密钥。

## 用 Docker 部署

镜像为 `scratch` 容器，仅含 `nanofile` 一个二进制，无配置文件与数据目录，以 `1000:1000` 运行，挂载的数据卷需对该用户可写。

主密钥（`secret_key`）生成一次并长期保存：它用于加密会话、邮件与静态块。更换后所有会话登出、同步客户端失效、已加密块无法解密。

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret

docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -v "$PWD/config.toml:/etc/nanofile/config.toml:ro" \
  -v "$PWD/nanofile-secret:/run/secrets/nanofile-secret:ro" \
  -e NANOFILE_CONFIG=/etc/nanofile/config.toml \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_INDEX_INDEX_DIR=/data/index \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/<owner>/nanofile:latest
```

配置文件可选，其余由内置默认值填充：

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/<owner>/nanofile:latest
```

`nanofile-secret` 为上方生成、跨启动复用的持久值。

## 配置

配置来源为工作目录下的 `config.toml`（复制 `config.toml.example` 修改，每个键上方的注释给出对应环境变量名），或 `NANOFILE_*` 环境变量（优先级最高、不写回文件）。容器部署主要依赖环境变量。

常用项：

- `site_url`：对外访问地址（HTTPS 域名），决定会话 cookie 的 `Secure` 属性与 HSTS 是否启用，也用于生成分享链接。
- 绑定地址与端口（`[server]` 的 `addr`/`port`）：默认 `0.0.0.0:8082`。
- 存储目录（`[storage]`）：默认位于二进制所在目录的 `data/` 下；相对路径相对于二进制所在目录解析，绝对路径可放到其他位置。
- `secret_key`：主密钥（见「用 Docker 部署」）。

大部分设置立即生效；监听地址、传输上限、缓存目录等在"重启"后生效。网页"系统设置 → 设置"可修改部分运行期设置，保存值优先于配置文件同名项。这些页面分为服务器、安全、身份验证、速率限制、存储、加密、维护、邮件、通知、高级十类，各自按分组列出条目，并可在浏览器里按名称、配置键或说明筛选；每一行都会说明取值的来源，以及修改后是否需要重启。

网页"系统设置 → 任务管理"分为两页。**任务列表**先给出服务器负载——在途请求、运行中任务、排队任务、数据库连接、工作线程占用与活跃任务，因为它们才解释得清一个作业为什么在等——然后才是运行本身：正在运行的作业，以及服务器真正做过的事。已完成列表只保留有内容可报的运行（失败，或确实做了事），因此"什么都没找到"的定时清理不会把它们淹没；列表可以按任务、按成功／失败筛选，且两个筛选条件互不覆盖。每一个运行行都给出作业名与结果、提交者，以及右对齐的完成时间与耗时两列；失败的运行把作业自己的报错直接显示在行内，运行 ID、作业自报的摘要与完整报错放在行内展开里。历史保留多久写在列表上方。**已注册的任务**列出本服务器注册的每一个作业与常驻服务，并按各自的触发方式分组；每一行说明它什么时候运行、做什么，并用同样的右对齐列给出上次运行——它是从历史里读回来的，所以重启后不会让每个作业都看起来从未运行过。展开一行可以看到它的累计计数（每次触发都计入，包括什么都没找到的那些）与运行策略——优先级、并发上限、超时、重试、是否会等待服务器空闲、能以什么粒度中断、崩溃后会留下什么。子系统未开启的作业或服务也会在这里列出，并说明开启哪个开关能让它回来；可以手动运行的作业，行内就有触发按钮。

设置页页头的**「重启服务」**按钮会就地重启服务：进程、托盘图标与监听端口都不变，但设置表、数据库连接、缓存与后台任务全部重建，因此"需重启"的项会立即生效（管理员会话保持有效）。日志级别/文件上限以及桌面托盘是否显示、托盘语言在进程启动前就已决定，只有**完全重启进程**才能生效，页面会单独标注。

升级时若配置格式变化，`config.toml` 就地更新（保留注释），原文件备份为 `config.toml.bak`。

配置段：`[server]` 网络与功能开关；`[database]` 数据库连接；`[storage]` 存储目录与配额；`[auth]` 登录/密码/限流；`[ui]` 语言；`[email]` 发信；`[admin_init]` 首次启动自动建管理员；`[logging]` 日志；`[gc]` 垃圾回收；`[index]` 搜索；`[sandbox]` 不可信解析的沙盒；`[notification]` 通知；`[tasks]` 后台任务。具体键名见 `config.toml.example`。

## 沙盒

所有解析用户提供内容的功能都在独立且受限的子进程中运行，不在服务器进程内：搜索索引的文档正文提取、图片缩略图、媒体缩略图（经 `ffmpeg`）、EXIF 与头像处理。子进程在读取请求之前先自我约束，并打印它实际建立的保护；服务器按这份报告决策，而不是按它提出的要求。

管理后台的 **沙盒** 页面显示本机实际达到的等级：

- **完整** —— 本平台能提供的全部保护：资源限制（内存、CPU、句柄、文件大小）、不可访问文件、不可访问网络、不可启动其他程序。
- **部分** —— 有资源限制，且其余三项中至少一项在位，并列出缺失的那一项。
- **无** —— 只有资源限制。

每一项旁边会注明本平台固有的残余：macOS 必须为解析器线程放行 `fork`；Windows 容器可读取它启动所需的系统目录；媒体 worker 只允许执行配置好的 helper 以及内核启动它所需的加载器，并且只能读取交给它的那个临时文件。

媒体 worker 在页面上单独一行，并且按设计在所有平台上都是 **部分**：它的存在意义就是启动程序，而任何启动程序的方式都要先复制进程，因此它不可能具备文档与图片 profile 具备的“进程”这一项。约束它的是文件层（只允许执行 helper 与内核为它启动的解释器，绝不放行任何目录）以及时间限制（约束副本数）。因此把 `sandbox.min_level` 提到 `full` 会停用媒体缩略图，页面会说明这一点。

两个设置决定其行为。`sandbox.enabled` 是总开关：关闭后上述功能一律不运行，也不会回退到服务器进程内解析。`sandbox.min_level` 是本机必须达到的等级——`full`、`partial`（默认）或 `none`；低于它时相关功能停用，页面会说明原因。未达到“完整”的本机仍会运行这些功能，但会给出警告。

`storage.ffmpeg_path` 指定媒体 profile 可以执行的辅助程序。保存它会通过与上面两个设置相同的钩子重新解析这份授权，因此下一次媒体请求执行的就是刚配置的二进制。在 Windows 上，helper 必须放在容器能够运行程序的位置——系统目录或 `Program Files`；装在用户目录下的 helper 容器读得到却启动不了，页面会如实报告，而不是让缩略图默默消失。容器中可能需要放行 Landlock 系统调用，等级才能达到“完整”。

## 安全

- 部署在 HTTPS 反向代理后时，将 `site_url` 设为 HTTPS 地址。该设置决定会话 cookie 的 `Secure` 属性与 HSTS 是否启用。会话、分享密码与 API 密钥均为持有即有效的凭据。
- 默认绑定 `0.0.0.0`；仅由反向代理访问时，改用 `127.0.0.1` 并关闭防火墙对应端口。
- 反向代理后需设置 `trusted_proxies`，否则 `X-Forwarded-For` 可被伪造，绕过按 IP 的限流。
- 加密资料库上传同样需要资料库密码；未提供时网页预览与下载返回 440，匿名上传链接不可用于加密库。
- 改密码、停用账号或远程擦除会吊销该账号持有的全部登录：其他设备、同步客户端、API 密钥，以及未使用的重置链接。
- release 构建在未设置 `secret_key` 时拒绝启动；debug 构建自动生成，但会话无法跨重启保留。`NANOFILE_SERVER_ALLOW_EPHEMERAL_SECRET_KEY=1` 仅用于本地与 CI。

## 命令行

```
nanofile [--config <path>]          启动服务器（默认）
nanofile [--config <path>] adduser  建用户（默认管理员；--regular 建普通用户）
                                    密码：交互输入，或 --password-stdin / --password-file <path>
nanofile [--config <path>] migrate-blocks [--dry-run]
                                     把旧版"所有库共用一个块目录"迁移成"每个库独立目录"
                                     （一般启动时自动做；--dry-run 只预览不执行）
```

Windows 服务（需管理员权限）：

```
nanofile [--config <path>] service install [--account virtual|system|network|<账号>]
                                     注册为自动启动的 Windows 服务（开机即运行，无需登录）
                                     默认账号：NT SERVICE\Nanofile（见下）
nanofile [--config <path>] service uninstall  停止、删除该服务，并回收它的目录权限
nanofile [--config <path>] service status     查询注册状态与运行账号（已注册退出码 0）
nanofile [--config <path>] service run        SCM 调用，手工执行会直接报错退出
```

`service install` 与托盘菜单「开机启动（管理员）」含义一致：同样会移除本安装的「登录启动」项，
避免两者同时启动。指定具体账号时需要 `--password`、`--password-stdin` 或交互输入密码。

## 桌面与 Windows 服务

`--features tray` 构建的二进制带系统托盘菜单：**登录启动**（写入当前用户注册表 `Run` 项，登录后启动）、Windows 上的**开机启动（管理员）**（见下）、打开网页、打开配置文件、退出。

**开机启动（管理员）**会向 SCM 注册一个自动启动的服务，**开机即启动，不需要任何人登录**，适合无人值守的机器；命令行 `nanofile service install` 等价，并且同样会移除「登录启动」项。

- 勾选后会弹出管理员授权提示（注册服务需要管理员权限），取消授权不会产生任何改动。弹授权提示**之前**会先检查这个服务是否真能跑起来（配置文件、各个数据目录、日志目录、`ffmpeg`、端口）：过不了的检查会点名并拒绝注册，需要注意的项会先让你确认。
- **服务以 `NT SERVICE\Nanofile` 运行**——每服务独立的虚拟账号：无密码、独立 SID、不与他人共用身份，访问网络时以机器账号身份。这是微软推荐的做法（而不是 `LocalSystem`）；`LocalSystem` 仍可通过 `--account system` 指定，用于虚拟账号拿不到权限的目录场景，也支持 `--account network` 或具体账号。账号会写在确认框里、由 `service status` 报告、并在每次启动时记入日志。
- 安装时会**为该账号授予**各个数据目录、日志目录与配置文件的修改权限（服务身份默认什么都访问不了），卸载服务时会回收；如果有目录它写不了，会在注册之前拒绝，而不是等到下次开机才失败。
- 两种自启方式是二选一：注册服务会移除「登录启动」项（确认框会说明），服务注册期间托盘禁用该菜单项。**卸载服务只会让该菜单项恢复可用，不会自动把自启项加回来**：之后是否自动启动由你决定。
- 两种注册都记录绝对路径，因此会"活过"一次移动：登录自启项指向的可执行文件或配置文件已不存在时，下次启动托盘会自动改为指向当前这份（健康项、或本版本读不懂的项，一律不动）——但服务已经接管时不会这么做；服务注册指向的目录已不存在时，确认框会明确说明再改写。另外，显式指定的配置文件不存在（`--config` 或 `NANOFILE_CONFIG`）会**拒绝启动**，而不是悄悄退回内置默认值——那会以另一个端口、另一个数据库启动一个貌似的服务。
- **从下次系统启动开始生效**：当前这份托盘程序继续提供服务。不让服务立刻接管端口，是因为 Windows 下刚关闭的连接会停留在 `TIME_WAIT` 数分钟，新进程绑定同一端口会失败。
- 服务运行时手动打开托盘程序会进入"客户端模式"：不再启动第二份服务，且**点「退出」会停止该服务**——与普通托盘里"退出即停服务器"的含义一致；服务会在下次开机再次启动。该菜单同样可以安装/卸载服务。
- 服务模式没有桌面，因此不会显示托盘图标；日志写入二进制旁的 `nanofile.log`。服务启动时会记录账号、可执行文件、配置文件、工作目录，以及每一条目录检查的结果；预检不过时会直接拒绝启动（退出码 2），而不是稍后抛一个不带路径的错误。

## 数据目录

默认数据位于二进制所在目录的 `data/`，相对路径均相对该目录解析；配置段可指定绝对路径。

```
data/
├── nanofile.db        # 数据库（WAL 模式，权限 0600）
├── nanofile.db-wal    # WAL 日志
├── blocks/            # 文件块：repos/{库id的sha1}/{2位前缀}/{40位SHA-1}
├── temp/              # 上传临时文件
├── thumbnails/        # 缩略图缓存
├── avatars/           # 头像
└── index/             # 全文搜索索引
```

## 开发

**架构**：Cargo workspace，四个 crate——`base`（基础类型）、`infra`（数据库/存储/加密/配置）、`server`（HTTP 服务、同步协议、WebDAV、Web 界面）、`migration`（数据库迁移）。依赖方向 `base → infra → server`。

**前端**：网页由 Askama 服务端渲染 + Tailwind + `server/frontend/` 下的模块化 JS 组成。`server/build.rs` 用 esbuild 把 `frontend/entries/*.js` 打包进二进制（esbuild 必需，Tailwind 可选）。改前端后需重新 `cargo build`，无热重载。

**测试**：`cargo test --workspace`（Rust）、`node --test "server/frontend/**/*.test.js"`（前端）、`cd e2e && npx playwright test`（浏览器端到端）。CI 还会跑 `cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings`。

**CI 与发布**：`ci.yml` 在 push/PR 时跑测试；`nightly.yml` 每日构建多架构镜像（`:edge`）；`release.yml` 在版本 tag 发布带版本号的镜像与 GitHub Release。

## 许可证

MIT
