# 维护与故障排查

日常要动手的部分：看日志、让空间真正释放、重建索引、校验块，以及出问题时先查哪里。

## 日志

`[logging]` 决定日志去哪、记多少：

| 键 | 作用 |
|---|---|
| `logging.level` | 日志级别。 |
| `logging.file_enabled` | 写到文件而不是 stdout。 |
| `logging.file` | 日志文件路径。 |
| `logging.max_file_size_mb` | 单个日志文件的上限。 |
| `logging.max_backups` | 保留多少个旧文件。 |

**默认会自动选**：桌面（托盘）运行写文件，无头运行写 stdout。桌面运行还会把解析出来的路径记回配置文件的 `[logging] file`，好让之后每次启动都用同一个文件。Windows 服务模式没有桌面，日志写在二进制旁的 `nanofile.log`。

**日志级别与文件上限属于"需完全重启进程"的那一类**：它们在进程启动前就已决定，管理后台里那个"重启服务"按钮不重新走这一步，只有真正停掉进程再启动才会生效。

后台运行时日志去 stdout，因此用 `journalctl -u nanofile -f`（systemd）或容器日志查看；见[后台运行与自启动](/zh/guide/running-in-background)。

## 垃圾回收

删除文件**不会立刻**把磁盘还回来。块要同时满足"没有任何提交再引用它"和"存活时间超过 `gc.min_block_age_secs`"，才会被下一次 GC 收走：

| 键 | 作用 |
|---|---|
| `gc.enabled` | 是否开启垃圾回收。 |
| `gc.interval_hours` | 每隔多少小时跑一次。 |
| `gc.min_block_age_secs` | 块至少要存在多久才会被考虑回收——给"刚上传完、还没被引用"的块留出缓冲。 |

三个键都属于**"需重启"**的一类：设置页页头的**「重启服务」**会让它们立即生效——它就地重建设置表、缓存与后台任务。真正必须完全重启进程的是另一类（日志级别、日志上限、托盘），见[配置](/zh/guide/configuration)。

## 搜索索引

| 键 | 作用 |
|---|---|
| `index.enabled` | 是否做全文索引。关掉后搜索只匹配文件名。 |
| `index.index_dir` | 索引目录。 |
| `index.backfill_enabled` | 是否为已有内容补齐索引。 |

三个键同样是**"需重启"**的一类，「重启服务」即可生效。

单个文件或一批文件可以在网页里重建：详情抽屉里的**重新索引**按钮，或多选后详情抽屉里的**重新索引**。整站重建用 `POST /api2/reindex/`。

索引依赖沙盒里的文档提取（PDF、Office、图片等）。沙盒关着的时候文档不会被索引，而是记为"可重试"，重新打开后会被重新处理——见[沙盒](/zh/guide/sandbox)。

## 块与目录校验

三个命令都是**默认只读**，加 `--repair` 才写：

| 命令 | 什么时候用 |
|---|---|
| `nanofile verify-blocks` | 怀疑块损坏（磁盘错误、复制中断、恢复后想核对）。逐块校验 SHA-1，并核对每个文件的块字节和是否等于声明大小。 |
| `nanofile verify-dir-order` | 只在同一台服务器与官方客户端混合使用、且客户端报告目录结构异常时才需要：检查目录对象是否按 Seafile 规范的顺序存储。 |
| `nanofile migrate-blocks` | 从旧版"所有库共用一个块目录"迁移到"每库独立目录"。启动时会自动做，这个命令用来预先估算复制量或在停机时手工执行。 |

`verify-blocks --repair` 不会删除坏块，而是把它隔离到 `{block_dir}/.quarantine/`——字节还在，持有正确副本的客户端会重新上传。完整参数见[命令行](/zh/reference/cli)。

## 任务页

**系统设置 → 任务管理**是观察后台作业的地方：正在跑什么、跑过什么、失败原因，以及每个已注册作业下次什么时候跑。升级后想确认迁移类作业有没有跑完，看这里。见[配置](/zh/guide/configuration)。

## 常见问题

**启动后进了初始化页面，而不是直接服务。**
说明既没有配置文件、也没有任何 `NANOFILE_*` 变量。这是"尚未初始化"的状态，不是故障：走完一次性页面即可，或自己放一个配置文件。见[安装与首次运行](/zh/guide/getting-started)。

**release 构建拒绝启动。**
缺 `server.secret_key`。生成一个并放进配置文件（或 `NANOFILE_SERVER_SECRET_KEY`）：`openssl rand -hex 32`。见[安全](/zh/guide/security)。

**端口起不来。**
被别的服务占了就换 `server.port`。Windows 下刚关掉的连接会停留在 `TIME_WAIT` 几分钟，新进程绑同一端口会失败——这也是 Windows 服务要等下次开机才接管的原因。

**忘了管理员密码。**
用命令行新建一个管理员再进去改：`./nanofile adduser --email <新地址>`（加 `--password-stdin` 可免交互）。见[命令行](/zh/reference/cli)。

**客户端连不上 / 能登录但不同步。**
按[客户端接入](/zh/guide/clients)末尾的清单逐项查，最常见的是服务器地址与 `server.site_url` 不一致。

**搜索搜不到刚上传的东西。**
先确认 `index.enabled` 是开的，再给那个文件点一次重新索引。索引是异步的，大文件会晚一点出现。

**缩略图或预览不工作。**
`[sandbox] enabled` 关着时这些功能一律不运行（且不会退回服务器进程内解析）；媒体缩略图还需要 `ffmpeg`，路径由 `storage.ffmpeg_path` 指定。见[沙盒](/zh/guide/sandbox)。

**邮件发不出。**
见[邮件通知](/zh/guide/email)：先确认 `enabled` 已开、SMTP 各项填对，再看任务页里的失败原因。

**上传失败。**
依次看：账号配额（[用户与分享](/zh/guide/users-and-sharing)）、反代或客户端的请求体上限、以及存储上限（进行中的断点续传与单个压缩包都有默认上限，设为 `0` 取消，见[配置](/zh/guide/configuration)）。

**反代之后所有人都是一个 IP。**
设 `trusted_proxies`，否则 `X-Forwarded-For` / `CF-Connecting-IP` 一律被忽略，按 IP 的限流退化成一个桶。见[安全](/zh/guide/security)。

## 相关

- [备份、恢复与升级](/zh/guide/backup-and-upgrade) — 停机备份与换版本。
- [配置](/zh/guide/configuration) — 每个设置何时生效，以及任务页。
- [命令行](/zh/reference/cli) — 全部运维命令。
- [沙盒](/zh/guide/sandbox) — 解析子进程约束了什么。
