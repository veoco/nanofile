# 数据目录

默认数据位于二进制所在目录的 `data/`，相对路径均相对该目录解析。配置文件的 `[storage]`、`[index]` 与 `[database]` 段可以指定绝对路径，把各部分放到别处。

## 首次初始化决定的位置

首次初始化会把这个位置定下来：

- 默认建议 `~/.nanofile/data`（与它即将写出的配置文件同目录），
- 如果二进制旁已经存在 `data/`，则默认沿用该目录——配置文件被删、数据还在的安装不会因此看起来"数据丢了"，
- 并把结果以**绝对路径**写进 `~/.nanofile/config.toml`。

自己写的配置文件里的相对路径仍然以二进制所在目录为基准。

## 目录结构

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

各部分的性质不一样，备份与清理时区别对待：

| 目录 | 丢了会怎样 | 可否重建 |
|---|---|---|
| `nanofile.db` | 账号、资料库结构、分享链接、设置全部丢失 | 不能 |
| `blocks/` | 所有文件内容丢失 | 不能 |
| `index/` | 搜索只匹配文件名 | 可以，重建索引即可 |
| `thumbnails/`、`avatars/` | 缩略图与头像需要重新生成 | 可以 |
| `temp/` | 无影响（上传临时文件） | 不需要备份 |

对应的设置：`[storage]` 的 `block_dir`、`temp_dir`、`thumbnail_dir`、`avatar_dir`，`[index]` 的 `index_dir`，以及 `[database] url`。

## 想换个位置

**停掉服务再动。** 目录位置属于"下次启动生效"的设置，服务还在跑时移动目录，只会让它继续往旧位置写。

1. 停服务。
2. 把目录移到新位置（`mv /opt/nanofile/data /srv/nanofile-data`）。
3. 在配置文件里把对应的键改成**绝对路径**（相对路径是相对二进制目录解析的，不是相对旧位置）。
4. 启动，确认日志里没有"目录不可用"，再随手下载一个文件验证。

数据库的路径改的是 `[database] url`，例如 `sqlite:/srv/nanofile-data/nanofile.db?mode=rwc`。

## 权限

运行服务的用户要对整个数据目录可读写；容器里是 uid/gid `1000:1000`：

```bash
chown -R 1000:1000 data
```

Windows 服务会由安装过程为服务账号授予所需权限，见[桌面托盘与 Windows 服务](/zh/guide/desktop-and-service)。

## 相关

- [备份、恢复与升级](/zh/guide/backup-and-upgrade) — 哪些必须备份、怎么恢复。
- [Docker 部署](/zh/guide/docker) — 把数据目录挂载进容器。
- [配置](/zh/guide/configuration) — 存储与数据库设置。
