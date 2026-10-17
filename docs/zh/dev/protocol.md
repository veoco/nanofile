# Seafile 兼容性

Nanofile 实现 Seafile 的同步协议，以及官方桌面端与移动端客户端使用的接口。本页记录代码必须保持的兼容规则。它只记录由代码与测试证实的行为，不宣称支持尚未实现的功能。

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

## 保留的客户端怪癖

这些行为是刻意的，因为官方客户端依赖它们：

- 动态里的头像 URL 是绝对地址，因为 Android 客户端直接用 Glide 加载该字段。作者不存在时使用占位头像，URL 仍然格式正确。
- 资料库事件使用 `path="/"` 与空的 `name`，与 seahub 一致；nanofile 自己的资料库重命名事件在 seahub 里没有对应物，因此保留新名称。
- 无冲突的服务端合并使用上游描述 `Auto merge by system`。

## 测试

兼容性面由 `server/tests/` 下的 `commit_test.rs`、`fs_test.rs`、`fs_id_compat.rs`、`dir_order_test.rs`、`sync_test.rs`、`sync_merge_test.rs`、`client_compat_test.rs` 与 `capability_coverage_test.rs` 覆盖。套件如何运行见[测试](/zh/dev/testing)。
