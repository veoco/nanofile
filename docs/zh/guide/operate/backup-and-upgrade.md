---
description: 备份数据库与块、恢复，并安全升级。
---
# 备份、恢复与升级

**适合谁：** 对数据与版本负责的运维人员。**前置条件：** 一个备份目标，以及停服务的权限。

## 要备份什么

| 内容 | 是否必须 | 说明 |
|---|---|---|
| 配置文件（或环境变量的来源） | **必须** | 里面有主密钥 `server.secret_key`。丢了它，同步客户端失效，TOTP 种子、队列邮件与已保存的密钥类设置无法解密；换了密钥还会在下次启动时让所有会话退出登录。开了静态加密时块密钥是 `storage.encryption_key`：它从不写入配置文件，只来自 `NANOFILE_STORAGE_ENCRYPTION_KEY`/`_FILE`（该来源要单独备份），未设置时由 `server.secret_key` 派生。 |
| `nanofile.db` | **必须** | 账号、资料库结构、分享链接、已保存的设置。 |
| `blocks/` | **必须** | 文件块的唯一副本，没有它就没有任何文件内容。 |
| `index/` | 可选 | 全文索引，可以由 `blocks/` 重建。 |
| `thumbnails/` | 可选 | 缩略图缓存，可由 `blocks/` 重建。 |
| `avatars/` | **建议备份** | 用户上传的头像原图——唯一副本，不存放在 `blocks/` 或数据库里。原图在时派生尺寸可重建，但该目录一旦丢失，原图无法恢复，只能让用户重新上传。 |
| `temp/` | 不需要 | 上传过程中的临时文件。 |

配置与数据目录的位置见[数据目录](/zh/guide/configure/data-directory)。

## 停机备份（最省心）

```bash
sudo systemctl stop nanofile          # 或 docker stop nanofile
cp -a /opt/nanofile/config.toml /backup/
cp -a /opt/nanofile/data /backup/
sudo systemctl start nanofile
```

停掉服务后整体复制，数据库与块目录天然是同一时刻的状态，不必考虑 WAL 与顺序。

如果开了静态加密，且密钥来自 `NANOFILE_STORAGE_ENCRYPTION_KEY` 或
`NANOFILE_STORAGE_ENCRYPTION_KEY_FILE`，这份来源也要一起备份：密钥从不写进
`config.toml`，只备份配置文件会让块解不开。

## 不停机备份

服务还在跑时做备份，这两点要注意：

- **不要直接复制数据库文件。** 它工作在 WAL 模式下，未合并的 `nanofile.db-wal` 会让复制出来的库停在旧状态。用 SQLite 自己的备份命令（需要主机上有 `sqlite3`）：

  ```bash
  sqlite3 /opt/nanofile/data/nanofile.db ".backup /backup/nanofile.db"
  ```

- **顺序：先备份数据库，再复制 `blocks/`。** SQLite 快照在开始时保持一致，而上传是先写块、后提交引用它的 commit。先取的数据库快照只会引用已经在磁盘上的块，之后复制块目录必然把它们全包含进来；期间多写的块没人引用，交给垃圾回收清理。反过来才危险：在块复制与数据库快照之间提交的 commit 可能引用复制时还不存在的块，恢复出的数据库就会指向缺失的块。

## 恢复

1. **停服务。**
2. **放回配置文件**，主密钥必须与备份时的那份一致。换了主密钥会在下次启动时让所有会话退出（服务端会记录密钥指纹），打断同步客户端，并让 TOTP 种子、队列邮件、已保存的密钥类设置以及由它派生的加密块无法使用。
3. **放回数据库**（用备份出来的那个文件即可，启动时它会自己处理日志）。
4. **放回 `blocks/`**。
5. **检查权限**：运行服务的用户要对这些文件可读写；容器里是 uid/gid `1000:1000`（`chown -R 1000:1000 data`）。
6. **启动并看日志**：日志在哪见[维护](/zh/guide/operate/maintenance)。
7. **可选校验**：`nanofile verify-blocks` 会逐块核对内容与其 SHA-1，见[命令行](/zh/reference/cli)。

`index/` 不必强求一致：索引与另一套块目录混在一起时，把索引目录清空让它重建最简单。

**静态加密的部署**：`blocks/` 依赖块密钥与 `storage.block_encryption_mode`，这两者与块必须一起备份、一起恢复。块密钥从不写进 `config.toml`：它来自 `NANOFILE_STORAGE_ENCRYPTION_KEY`/`_FILE`，两者都没设置时由 `server.secret_key` 派生，因此恢复主密钥也就恢复了块密钥。详见[安全](/zh/guide/admin/security)。

## 升级

1. **先备份。**
2. **停服务。**
3. **换二进制**：解开新归档，替换旧的那个文件；容器则是改镜像 tag 后重建容器。
4. **启动**，确认版本号（网页左下角）与日志都没有异常。
5. **看日志**：数据库迁移与块布局迁移都在启动时同步执行，早于服务器开始监听；任何一项失败都会导致启动失败，日志里会写明原因。任务管理列的是后台作业（例如 lazy 静态加密转换），不是迁移。见[监控](/zh/guide/operate/monitoring)。

几点值得一提：

- **别指望降级。** 数据库迁移只向前走，用旧版本打开新库可能直接起不来——所以第 1 步的备份不是可选项。
- **改名的设置会自动迁移。** 旧键名仍会被读取，已保存的值在升级时迁到新键，不需要手工改配置文件。
- **后台运行的部署**：换完二进制 `systemctl restart`（见[后台运行与自启动](/zh/guide/deploy/running-in-background)）；Windows 服务同样要先停再换，见[Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service)。
- **容器**：`docker compose pull && docker compose up -d`，数据卷保持不变；升级后若权限不对，先 `chown -R 1000:1000 data`（见[Docker 部署](/zh/guide/deploy/docker)）。

## 验证

- 对恢复出的块目录跑 `nanofile verify-blocks`，通过。
- 恢复出的实例版本一致，测试下载正常。
- 启动日志显示迁移均已成功完成。

## 版本渠道

| 标签 | 含义 |
|---|---|
| `ghcr.io/veoco/nanofile:latest` | 默认分支上的最新发布 |
| `ghcr.io/veoco/nanofile:vX.Y.Z`、`vX.Y` | 固定版本，生产环境推荐 |
| `ghcr.io/veoco/nanofile:edge` | 每天 UTC 10:00 从最新提交构建的 nightly，包含尚未发布的改动 |

预编译归档只有发布版本（`vX.Y.Z`），没有 `edge` 对应物。

## 相关

- [数据目录](/zh/guide/configure/data-directory) — 各个目录里有什么。
- [维护](/zh/guide/operate/maintenance) — 日志、垃圾回收与校验命令。
- [安全](/zh/guide/admin/security) — 主密钥与静态加密。
- [命令行](/zh/reference/cli) — `verify-blocks`、`verify-dir-order`、`migrate-blocks`。
