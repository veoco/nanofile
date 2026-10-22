# 命令行

```
nanofile [--config <路径>]          启动服务器（默认）
nanofile [--config <路径>] server   显式启动服务器
```

`--config <路径>` 覆盖 `NANOFILE_CONFIG` 与默认查找顺序。见[配置](/zh/guide/configuration#配置文件的查找顺序)。

## 服务器与账号管理

```
nanofile adduser [--email <地址>] [--regular]
                 [--password <值> | --password-stdin | --password-file <路径>]
```

创建账号，默认为管理员；`--regular` 创建普通用户。优先用交互式提示、`--password-stdin` 或 `--password-file`，而不是 `--password`——后者会出现在 shell 历史与 `ps` 输出里。

## 块维护

```
nanofile migrate-blocks [--dry-run]
```

把旧版"所有库共用一个块目录"的布局（`blocks/<2hex>/<id>`）迁移成"每个库独立目录"。一般启动时会在服务第一个请求之前自动做。用这个命令可以预先估算复制量（`--dry-run`），或在服务停止时显式执行。

```
nanofile verify-blocks [--dry-run] [--repair] [--repo <id>] [--json]
```

逐块校验 sha1(id) 是否与内容一致，并校验每个文件对象"各块字节和 == 声明大小"。

- 默认只读，只有加 `--repair` 才写。
- `--repair` 会把校验失败的块隔离到 `{block_dir}/.quarantine/`。字节保留不删除，持有正确副本的客户端会重新上传。
- `--repo` 只检查一个资料库。
- `--json` 向 stdout 输出机器可读报告。
- 有发现时退出码非 0。

```
nanofile verify-dir-order [--dry-run] [--repair] [--repo <id>] [--json]
```

检查每个资料库 HEAD 可达的目录对象是否按 Seafile 的规范（名称降序）顺序存储——这是它的 `fs_id` 能被官方客户端复现的前提。

- 默认只读，只有加 `--repair` 才写。
- `--repair` 为每个受影响的资料库追加一个规范化提交。历史不重写、对象不删除，客户端会把它当作一次普通更新。
- `--repo` 与 `--json` 的行为同 `verify-blocks`。
- 有发现时退出码非 0。

## Windows 服务

安装或删除服务需要管理员权限。

```
nanofile service install [--account virtual|system|network|<账号>]
                         [--password <值> | --password-stdin]
nanofile service uninstall
nanofile service status
nanofile service run
```

- `install` 注册自动启动服务（开机即运行，无需登录），并移除本安装的"登录启动"项。默认账号是 `NT SERVICE\Nanofile`；见[桌面托盘与 Windows 服务](/zh/guide/desktop-and-service#开机启动管理员)。
- `uninstall` 停止并删除服务，回收授予它的目录权限。
- `status` 查询注册状态与运行账号；已注册时退出码 0。
- `run` 由 SCM 调用。手工执行会直接报错退出。

## 不是运维命令

二进制还接受一个隐藏的 `extract-worker` 子命令。服务器在每个沙盒请求上重新执行自己来启动它，不应手工运行。release 与 nightly 工作流用 `extract-worker --probe` 对构建产物和已发布镜像做沙盒冒烟测试。见[沙盒内部](/zh/dev/sandbox)。
