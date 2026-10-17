# 数据目录

默认数据位于二进制所在目录的 `data/`，相对路径均相对该目录解析。`config.toml` 的配置段可指定绝对路径。

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

`[storage]` 与 `[index]` 设置可以移动各个目录。`[database] url` 设置数据库路径。

## 相关

- [Docker 部署](/zh/guide/docker) — 把数据目录挂载进容器。
- [配置](/zh/guide/configuration) — 存储与数据库设置。
