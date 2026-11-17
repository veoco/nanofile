# 命令行

```
nanofile [--config <路径>]          启动服务器（默认）
nanofile [--config <路径>] server   显式启动服务器
```

`--config <路径>` 覆盖 `NANOFILE_CONFIG` 与默认查找顺序。见[配置](/zh/guide/configure/configuration#配置文件的查找顺序)。

## 服务器与账号管理

```
nanofile adduser [--email <地址>] [--regular]
                 [--password <值> | --password-stdin | --password-file <路径>]
```

创建账号，默认为管理员；`--regular` 创建普通用户。优先用交互式提示、`--password-stdin` 或 `--password-file`，而不是 `--password`——后者会出现在 shell 历史与 `ps` 输出里。

## 块维护

```
nanofile gc [--dry-run] [--mode audit|incremental]
```

手动跑一轮垃圾回收：按各库的历史保留设置裁剪历史，删除不再被任何提交引用的块，以及已不存在的库的整个前缀。

- `--dry-run` 只报告会删除什么，不动任何数据。
- `--mode` 覆盖本次运行的 `[gc] mode`。`audit` 会列举每个库的块，也能发现崩溃上传留下的块；`incremental` 只删除历史裁剪刚刚变成孤儿的块，不做列举。
- 不带 `--mode` 时按配置的调度走：`gc.audit_interval_hours` 到期就把这一轮变成完整审计，并记录审计时间；带了 `--mode` 则本次不套用这个调度。
- 存活时间不足 `gc.min_block_age_secs` 的块永不删除；上传写入但一直没提交的块在 `gc.pending_ttl_hours` 之后被清掉。
- 最近一小时内有过写入的库会被跳过；读不出对象的库、以及引用了尚未写入对象的库只跳过该库。三者都在输出里单独计数。
- 输出会写明本次的扫描方式，以及各类别删了多少。
- 保护窗口与默认模式取自配置文件与 `NANOFILE_*` 环境变量；设置页保存的值不参与——它们配置的是后台作业，由作业在启动时读取。

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

- `install` 注册自动启动服务（开机即运行，无需登录），并移除本安装的"登录启动"项。默认账号是 `NT SERVICE\Nanofile`；见[Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service#开机启动管理员)。
- `uninstall` 停止并删除服务，回收授予它的目录权限。
- `status` 查询注册状态与运行账号；已注册时退出码 0。
- `run` 由 SCM 调用。手工执行会直接报错退出。

## 不是运维命令

二进制还接受一个隐藏的 `extract-worker` 子命令。服务器在每个沙盒请求上重新执行自己来启动它，不应手工运行。release 与 nightly 工作流用 `extract-worker --probe` 对构建产物和已发布镜像做沙盒冒烟测试。见[沙盒内部](/zh/dev/sandbox)。
