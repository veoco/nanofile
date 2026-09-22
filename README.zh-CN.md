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
邮件通知默认不可用，需先配置 SMTP。配置位置有两处：网页后台"系统管理 → 邮件"（填写 host、端口、账号、密码、发件人等并开启 `enabled`），或环境变量（如 `NANOFILE_EMAIL_ENABLED=1`、`NANOFILE_EMAIL_HOST=…`）。

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

大部分设置立即生效；监听地址、日志与索引目录在重启后生效。网页"系统管理 → 设置"可修改部分运行期设置，保存值优先于配置文件同名项。

升级时若配置格式变化，`config.toml` 就地更新（保留注释），原文件备份为 `config.toml.bak`。

配置段：`[server]` 网络与功能开关；`[database]` 数据库连接；`[storage]` 存储目录与配额；`[auth]` 登录/密码/限流；`[ui]` 语言；`[email]` 发信；`[admin_init]` 首次启动自动建管理员；`[logging]` 日志；`[gc]` 垃圾回收；`[index]` 搜索；`[notification]` 通知；`[tasks]` 后台任务。具体键名见 `config.toml.example`。

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
