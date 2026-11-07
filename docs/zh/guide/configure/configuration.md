---
description: 配置文件、环境变量与设置页如何分层。
---
# 配置

**适合谁：** 通过配置文件或环境变量修改设置的管理员。

每个设置都有内置默认值。改一个设置有两个入口：

- **网页界面。** "系统设置 → 设置"渲染了每一个设置，按服务器、安全、身份验证、速率限制、存储、加密、维护、沙盒、邮件、通知、高级分页，并可在浏览器里按名称、配置键、说明或 `NANOFILE_*` 变量名筛选。
- **`config.toml` 与 `NANOFILE_*` 环境变量。** 复制 [`config.toml.example`](https://github.com/veoco/nanofile/blob/main/config.toml.example)。模板刻意只保留两类键：网页无法保存的（主密钥、数据库 URL、静态加密模式、首启管理员、日志文件），以及必须在网页可达之前就定下来的（对外地址、监听、状态目录、语言）。

设置页是完整参考：每一行说明这个设置做什么、哪个 `NANOFILE_*` 变量会覆盖它、当前值来自哪里，以及修改后是否需要重启。本文只解释分层方式与最常手改的键。

## 优先级

从高到低：

1. 环境变量（`NANOFILE_*`）。
2. 配置文件。
3. "系统设置"保存的值。
4. 内置默认值。

环境变量始终优先，且永不写回文件，因此容器部署主要依赖它们。

## 配置文件的查找顺序

先命中先得：

1. `--config <路径>`
2. `NANOFILE_CONFIG`
3. `./config.toml`（历史默认值，保证已有部署继续读原来的文件）
4. `~/.nanofile/config.toml` —— 首次初始化写出的用户级文件

只要设置了任何 `NANOFILE_*` 环境变量，就视为已配置，即使没有配置文件也不会进入首次初始化。例外是不决定配置本身的几个变量：`NANOFILE_DOTENV`、`NANOFILE_TEST_FFMPEG_PATH`、`NANOFILE_BUILD_TS`。

显式指定的配置文件不存在（`--config` 或 `NANOFILE_CONFIG`）会**拒绝启动**，而不是悄悄退回内置默认值——那会以另一个端口、另一个数据库启动一个貌似的服务。

## 常用项

| 键 | 含义 |
|---|---|
| `server.site_url` | 对外访问地址（HTTPS 域名）。决定会话 cookie 的 `Secure` 属性、是否启用 HSTS，也用于生成分享链接。 |
| `server.addr` / `server.port` | 监听地址与端口。默认 `0.0.0.0:8082`。 |
| `server.secret_key` | 主密钥，只能写在文件里；见[安全](/zh/guide/admin/security)。 |
| `storage.block_dir`、`storage.temp_dir`、`storage.thumbnail_dir`、`storage.avatar_dir`、`index.index_dir` | 状态目录。相对路径相对于二进制所在目录解析；绝对路径可放到其他位置。 |
| `storage.block_encryption_mode` | `off`、`on` 或 `lazy`。只能写在文件里：它决定已有密文如何读取。 |

存储上限：进行中的断点续传合计默认最多声明 32 GiB，单个压缩包默认最多 8 GiB。在存储页（或 `[storage]`）把对应项设为 `0` 即取消上限。

`server.max_chunk_size_mb` 限制单个断点续传分块（默认 32 MiB）。官方客户端发送 4 MiB 分块；这个上限用来避免单个请求缓冲任意多的内存。只有确实发送更大分块的客户端才需要调高它。

## 何时生效

大部分设置立即生效。监听地址、传输上限、缓存目录在下次启动后生效。设置页每一行都会说明属于哪种情况。

设置页页头的**「重启服务」**按钮会就地重启服务：进程、托盘图标与监听端口都不变，但设置表、数据库连接、缓存与后台任务全部重建，因此"需重启"的项会立即生效，管理员会话保持有效。

日志级别/文件上限，以及桌面托盘是否显示、托盘语言，在进程启动前就已决定。页面会单独标注"需完全重启进程"，只有停止再启动进程才能生效。

## `[settings]` 分层

`config.toml.example` 里有一个 `[settings]` 段，决定已保存设置的含义。它不能从网页修改。

- `config_policy = "bootstrap"`（默认）让配置文件成为首启种子：文件里的值给没有已保存值的键提供初始值，某个键被管理员保存后，文件里的值对它就不再起作用。环境变量仍然优先。
- `config_policy = "override"` 让文件对它设置的每个键都优先（低于环境变量）。
- `config_override_keys` 列出在 `"bootstrap"` 下仍由文件胜出的键。
- `refresh_interval_secs` 定期重读已保存设置，使别处做的修改（或从备份恢复的设置）无需重启即可生效。`0` 表示关闭。

## 任务管理

**系统设置 → 任务管理**显示服务器负载、后台运行与已注册的作业。每页显示什么、该关注什么，见[监控](/zh/guide/operate/monitoring)。

## 配置段

`[server]` 网络与功能开关；`[database]` 数据库连接；`[storage]` 存储目录与配额；`[auth]` 登录/密码/限流；`[ui]` 语言；`[email]` 发信；`[admin_init]` 首次启动自动建管理员；`[logging]` 日志；`[gc]` 垃圾回收；`[index]` 搜索；`[face_tags]`
自动人物标签；`[ocr]` 写入搜索索引的图片与扫描版 PDF 文字；`[sandbox]` 不可信解析的沙盒；`[notification]` 通知；`[tasks]` 后台任务；`[settings]` 文件与已保存设置的分层策略。

配置文件中没写的键使用内置默认值。服务器**不会改写配置文件**，你写的什么样就一直是什么样。唯一例外是 `[logging] file`：桌面运行会把解析出的日志路径记进去，好让后续每次启动都用同一个文件。

## 相关

- [网页界面](/zh/guide/use/web-interface) — 设置页在界面里的位置。
- [监控](/zh/guide/operate/monitoring) — 任务管理、负载与磁盘。
- [维护](/zh/guide/operate/maintenance) — 日志、垃圾回收与索引这些设置什么时候用得上。
- [后台运行与自启动](/zh/guide/deploy/running-in-background) — 服务化部署时怎么把配置传进去。
- [命令行](/zh/reference/cli) — 运维命令。
