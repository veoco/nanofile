# Seafile 兼容性

Nanofile 实现 Seafile 的同步协议，以及官方桌面端与移动端客户端使用的接口。本页记录代码必须保持的兼容规则。它只记录由代码与测试证实的行为，不宣称支持尚未实现的功能。与 Seafile Web API 的逐接口对照见 [与 Seafile API 的兼容性](/zh/reference/api/compatibility)；本页覆盖那页装不下的协议规则。

## 提交 id

提交 id 的计算与 Seafile 的 `compute_commit_id()` 完全一致：SHA-1 依次覆盖

1. root id 的 40 位十六进制加一个 NUL，
2. creator id 的 40 位十六进制加一个 NUL，
3. creator name 加一个 NUL；名称为空时整段省略，
4. description 加一个 NUL，
5. 提交时间，8 字节大端。

NUL 终止符是哈希的一部分：每个字符串都按 C 字符串（`strlen + 1`）喂入，而不是裸字节切片。省略它们会得到任何官方客户端或服务端都不会为同一个逻辑提交算出的 id，于是通过网页 API 写入的资料库和桌面客户端写入的同一个资料库会在历史上产生分歧。

源码：`server/src/domain/commit.rs`；测试：`server/tests/commit_test.rs`。

## 目录顺序与 `fs_id`

目录对象的 `fs_id` 是其序列化形式的 SHA-1，因此 `dirents` 的顺序是对象身份的一部分。

官方客户端总是按**名称降序**写目录项（上游 `compare_dirents()` 是 `strcmp(b, a)`），并且读回已存储的 v1 对象时逐字节原样使用。以任何其他顺序存储的目录都会带上客户端无法复现的 id，下一次本地写入就会让两边对一个双方都没改过的目录产生分歧。

`nanofile verify-dir-order` 遍历资料库 HEAD 可达的每个目录，报告存储顺序不是规范顺序的对象。加 `--repair` 时，它为每个受影响的资料库追加一个普通提交，root 是当前树的规范形式：历史不重写、旧对象仍可读，客户端会把它当作普通更新。第二次运行不会再改动任何东西。

源码：`server/src/fs/core/dir_order.rs`；测试：`server/tests/dir_order_test.rs` 与 `server/tests/fs_id_compat.rs`。

## 收到的 FS 对象

默认情况下，收到的 FS 对象 id 必须等于 `sha1(uncompressed JSON)`：不一致会以 400 拒绝。官方服务端不做这个检查，但合法的 repo-v1 客户端都会发送匹配的 id，因此严格校验是安全的，并且能阻止客户端把内容与 id 不匹配的对象存下来。

策略由 `[sync] verify_fs_objects`（`NANOFILE_SYNC_VERIFY_FS_OBJECTS`）控制：`strict`（默认）、`log` 或 `off`。树的遍历受 `sync.max_tree_depth` 与 `sync.max_tree_visits` 约束，作为环与拒绝服务防护。

## 凭据与能力

每个请求被分类为三种凭据之一：

- **会话** —— 浏览器会话，或客户端登录后获得的账号级令牌。它**就是**账号，因此满足所有能力，也是密钥管理界面唯一接受的凭据。
- **API 密钥** —— 只携带被授予的能力，外加一个按资料库的上限，可以进一步收窄路由要求。授予 `file.write` 隐含 `file.read`；未分类的路由一律拒绝。
- **同步令牌** —— 构造上绑定一个资料库，API 界面从不接受；它会被拒绝，而不是当作账号凭据处理。

路由到能力的映射表在 `server/src/domain/capability.rs`；新增路由若未分类会直接失败。`server/tests/capability_coverage_test.rs` 检查映射表覆盖每个路由。

## 与上游的核对

本页的规则都对照过上游源码，不是凭记忆写的。核对用的版本：

| 仓库 | 版本 | 用来核对什么 |
| --- | --- | --- |
| `haiwen/seafile` | `af6539c` | commit id 计算、客户端守护进程的提交/对象写入 |
| `haiwen/seafile-server` | `a652460` | 同步 API 的服务端（C 的 `http-server.c` 与 Go 的 `fileserver/`）、仓库操作 |
| `haiwen/seahub` | `59e5b84` | `api2` / `api/v2.1` 的响应形状 |
| `haiwen/seafile-client` | `7b5a95a` | 桌面客户端的请求与解析 |
| `haiwen/seafile-ios` | `2aedcc3` | iOS 客户端的请求与解析 |
| `haiwen/seadroid` | `9d6d490` | Android 客户端的请求与解析 |
| `haiwen/seafdav` | `11295ad` | WebDAV 行为（wsgidav） |

下表行号来自这些版本；行号会漂移，commit id 不会。

| 行为 | 上游依据 |
| --- | --- |
| commit id 的输入顺序、NUL 终止符、空名称整段省略 | `seafile/common/commit-mgr.c:39-60`、`seafile-server/common/commit-mgr.c:41-60` |
| `creator_name` 非空才写入；读入时转小写 | `commit-mgr.c:606-609`、`:791-794` |
| 客户端先算 id，再把资料库元数据写进 commit | `seafile/daemon/repo-mgr.c:4567-4590` |
| 提交上传：解析失败/仓库不符 400，不校验哈希 | `seafile-server/fileserver/sync_api.go:941-984` |
| `recv-fs`：包格式错误 400，不校验内容 | `sync_api.go:453-517` |
| 文件/目录对象 id = `sha1(json_dumps(obj, JSON_SORT_KEYS))`；`modifier`/`size` 只出现在文件条目；降序排列 | `seafile-server/common/fs-mgr.c:381-392`、`:1795-1820`、`:1906-1911` |
| 同库移动是一个提交（先加后删，再提交） | `seafile-server/server/repo-op.c:3161-3245` |
| 旧版列表：字段、`dir_perm` 取真实权限、目录组+文件组各自按 `name.lower()` 排序 | `seahub/api2/views.py:2380-2462` |
| v2.1 列表：`{dirent_list, dir_id, user_perm}`、逐条 `permission`、`parent_dir`、`starred`、`modifier_email` | `seahub/api2/endpoints/dir.py:45-215`、`:343-344` |
| `/api/v2.1/search-file/`：`{"data": [{path, size, mtime(ISO), type: file/folder}]}`，无上限，文件名子串匹配 | `seahub/api2/endpoints/search_file.py` |
| `/api2/search/`：`{results, total, has_more}`，`per_page <= 100`，仅 Pro 版 | `seahub/api2/views.py:452-700` |
| 认证失败体是 `{"detail": …}`；功能错误才是 `{"error_msg": …}` | `seahub/api2/authentication.py:28-33`、`seahub/api2/utils.py:49-51` |
| 桌面端要求 `oid` 长 40、`dir_perm == "r"` 即只读、逐条 `permission == "r"` | `seafile-client/src/filebrowser/file-browser-requests.cpp:73-96`、`seaf-dirent.cpp:19-45` |
| 桌面端按服务端返回的顺序渲染列表 | `seafile-client/src/filebrowser/file-table.cpp:926-937` |
| 桌面端 `/api2/search/` 读取的字段 | `seafile-client/src/api/requests.cpp:939-980` |
| iOS v2.1 列表：`dirent_list`/`dir_id`/`user_perm`，逐条 `permission` 决定子目录可编辑；本地排序 | `seafile-ios/Pod/Classes/SeafDir.m:264-355`、`:160-170`、`:386-409` |
| iOS search-file 形状（`type == "folder"`、绝对 `path`）；`/api2/search/` 仅在 `seafile-pro`+`file-search` 时使用 | `seafile-ios/Pod/Classes/SeafConnection+Search.m:47-100`、`SeafConnection.m:1321-1338` |
| Android v2.1 列表模型；逐条 `permission` 生成操作菜单；`modifier_email` | `seadroid/.../ui/repo/RepoService.java:34-38`、`.../framework/db/entities/DirentModel.java:50-78`、`.../ui/repo/sheetaction/BottomSheetMenuManager.java:190-228` |
| WebDAV 的 `ETag` 是 FS 对象 id，GET 与 `getetag` 属性一致 | `seafdav/wsgidav/seafile_dav_provider.py:153-185`、`:637-640`、`dav_provider.py:747-749` |
| 条件请求先于 Range 求值；只支持单个区间；416 带 `Content-Range: bytes */size` | `seafdav/wsgidav/request_server.py:1477-1560`、`util.py:1485-1492` |
| `Depth: infinity` 可以用 403 + `<D:propfind-finite-depth/>` 拒绝（RFC 4918 §9.1） | `seafdav/wsgidav/request_server.py:282-296` |
| WebDAV 不提供加密资料库 | `seafdav/wsgidav/seafile_dav_provider.py:871-874` |

## 列表响应

`/api2/repos/{id}/dir/` 与 `/api/v2.1/repos/{id}/dir/` 报告调用者的**真实**权限，因为官方客户端据此决定哪些操作可用：

- 旧版列表的 `dir_perm` 头与每个条目的 `permission` 是 `"rw"` 或 `"r"`；
- v2.1 另外带 `user_perm`（同值）与 `dir_id`；
- nanofile 没有子目录共享，因此同一资料库里所有条目共用该值。

旧版列表按 seahub 的顺序返回（目录在前、文件在后，各自按小写名称排序），并支持 `?t=f`/`?t=d`。目录对象的存储顺序是它按名称降序的身份顺序，不是展示契约。

没有 head 提交的资料库（nanofile 会创建这种库；seahub 总会写一个 `Created library` 初始提交）列表返回 `oid`/`dir_id` = `EMPTY_SHA1` 和空列表。桌面端会拒绝 `oid` 不是 40 字符的列表，所以这是必需的，不是装饰。

列表键名使用 seahub 的拼写：`modifier_email`（不是 `modifier`），文件条目另带 `modifier_name`/`modifier_contact_email`。

## WebDAV 校验值

文件的 `ETag` 是加引号的 FS 对象 id —— 与 GET 和 PROPFIND 的 `getetag` 属性同值（seafdav 两处都报 `obj_id`）。`If-None-Match` 先于 `Range` 求值，弱比较接受 `W/` 前缀。`Depth: infinity`/`1` 的列表超过 `server.max_propfind_entries` 时返回 403 与 RFC 4918 的 `propfind-finite-depth` 体，告诉客户端改用更浅的 Depth；集合不再输出 `getcontentlength`。

## 刻意保留的偏差

以下与上游不同，是刻意的：

- **`_Internal` 在所有列表中隐藏**（普通、递归、WebDAV、`reloaddir`）。上游只在打包/索引的递归遍历里跳过它（仅根级，连同 `images`），因为 seahub 用它存 wiki/seadoc 的内部数据；nanofile 没有这棵树。从 seahub 迁移来的资料库会少看到一项。
- **`/api/v2.1/search-file/` 有上限**（共享的 200 条预算），上游无界遍历整棵树。两个移动客户端拿到的 `data` 会被截断。
- **`/api2/search/` 忽略 Pro 专属过滤参数**（`search_type`、`obj_type`、`time_from`、`size_from`、`search_path`、`with_permission`），并且在索引关闭时也可用（只匹配文件名）。
- **覆盖上传的额度按 `新 - 旧` 计**；上游上传路径按整个新 `Content-Length` 计，`update-branch` 处按 0。
- **不写 `Created library` 初始提交**：资料库可以没有 head，`commit/HEAD` 返回 `000…0` 哨兵，`get_commit` 合成它（上游总有一个真实提交）。
- **`dir/detail/` 的 `mtime` 是数字**，seahub 是 ISO 字符串（Android 的模型声明为 `long`，且从未调用该接口）。
- **未实现 `?oid=` 复用校验**（seahub 回 `"uptodate"`）：官方客户端不发它。
- **未实现 `If-Match`/`If-Unmodified-Since`/`If-Modified-Since` 与 `PROPPATCH`**。
- **WebDAV 对加密资料库返回 403**；seafdav 直接从列表里略过它（请求会变 404）。
- **`encoded_thumbnail_src` 恒为空**。iOS 与 Android 只把它当能力信号，并按服务端版本（低于 14.0，nanofile 即如此）自行拼旧的 `/api2/repos/{id}/thumbnail/` 地址。

## 保留的客户端怪癖

这些行为是刻意的，因为官方客户端依赖它们：

- 动态里的头像 URL 是绝对地址，因为 Android 客户端直接用 Glide 加载该字段。作者不存在时使用占位头像，URL 仍然格式正确。
- 资料库事件使用 `path="/"` 与空的 `name`，与 seahub 一致；nanofile 自己的资料库重命名事件在 seahub 里没有对应物，因此保留新名称。
- 无冲突的服务端合并使用上游描述 `Auto merge by system`。

## 测试

兼容性面由 `server/tests/` 下的 `commit_test.rs`、`fs_test.rs`、`fs_id_compat.rs`、`dir_order_test.rs`、`sync_test.rs`、`sync_merge_test.rs`、`client_compat_test.rs`、`api_v21_test.rs`、`webdav_test.rs`、`auth_test.rs` 与 `capability_coverage_test.rs` 覆盖。套件如何运行见[测试](/zh/dev/testing)。
