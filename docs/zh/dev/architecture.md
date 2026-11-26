# 架构

Nanofile 是一个包含五个 crate 的 Cargo workspace。依赖方向是单向的：`base → infra → server`；`migration` 与 `inference` 由 `server` 使用，而 `inference` 只依赖两个数值内核库。

| Crate | 职责 |
|---|---|
| `base` | 纯类型、`AppError`、路径规范化、MIME 与 User-Agent 助手。默认不依赖 axum 或数据库，除非启用 `with-axum` / `with-sea-orm` feature。 |
| `infra` | 数据库实体与连接、配置、加密、块存储、序列化、限流、设置目录与运行时快照、事件。 |
| `server` | HTTP handler 与页面、service、`AppState`、Seafile 同步协议、WebDAV、网页界面、文件系统引擎、沙盒、任务与可选托盘。 |
| `inference` | 内嵌模型的定计划推理运行时：算子执行器、f32 内核与生成好的计划。无 ONNX 解析器，不依赖 server。 |
| `migration` | SeaORM 数据库迁移。 |

## `server` 内部

| 模块 | 职责 |
|---|---|
| `handler` | HTTP 端点：同步 API、REST API、上传、下载。对 service 的薄适配；协议/页面适配器可以直接使用文件系统引擎。 |
| `ui` | Askama 渲染的网页及其表单处理，包括管理后台。 |
| `http` | `handler`、`ui`、`webdav` 共享的表现层支持：文件下载响应构造（Range、ETag、Content-Disposition）与有界 multipart 读取。 |
| `service` | 业务逻辑与用例：认证、同步、文件、分享、动态、邮件、搜索、通知、权限、上传状态。负责一个用例的事务边界。 |
| `repository` | 账号、令牌、提交、文件与分享的数据库访问：trait 接口、SeaORM 实现、缓存装饰器、窄事务聚合，以及活动日志写入。 |
| `domain` | 共享授权模型：能力、凭据、资料库范围、令牌过期、提交/fs_id 计算。不持有数据库或 HTTP 句柄。 |
| `fs` | Seafile 文件系统引擎：树操作、提交、合并、回收站、垃圾回收、块校验与迁移；也拥有跨提交/文件对象的多表事务操作。 |
| `middleware` | 认证与能力执行、CSP、CSRF、请求上限、路由审计。 |
| `webdav` | 建立在同一套资料库与文件服务之上的 WebDAV 接口。 |
| `indexer` | 全文索引（Tantivy）与喂给它的沙盒提取器。 |
| `ocr` | 全文索引所用的 PP-OCRv6 tiny 推理、页面与文本行方向分类器及 DB/CTC 后处理。 |
| `sandbox` | 解析用户内容的受限子进程。 |
| `tasks` | 后台作业的注册表、队列、执行器与运行历史。作业体调用 service；service 通过 `service::index::IndexScheduler` 提交工作。 |
| `settings` | 分层设置服务：已保存的行、生效值与管理端保存路径。运行时快照（`RuntimeConfig`）位于 `infra::settings`。 |
| `startup`、`tray` | 自动启动注册与可选的桌面托盘。 |
| `restart`、`serve`、`app`、`routes` | 进程生命周期、监听器、路由与应用状态。 |

## 领域模型

`domain` 放的是授权模型而不是持久化实体：`Capability`/`CapabilitySet`（路由表按方法字符串分类）、`Credential`（会话、API 密钥或同步令牌及其资料库范围）、`RepoScope`、令牌过期规则、设备与会话来源取值，以及仓储返回的 API-key 视图（`ApiKeyLookup`、`ApiKeyBinding`）。它可以引用 `infra::entity` 的数据形状，但不持有数据库连接或 HTTP 类型；需要成员行的检查位于 `service::permission`。

## 依赖规则

分层是目录约定，不是编译器错误。以下方向是错的，由 CI 里的 `tools/check-layering.mjs` 检查：

| 模块 | 不得引用 |
|---|---|
| `repository` | `service`、`handler`、`ui`、`webdav`、`middleware`、`tasks`、`sandbox`、`app`、`routes`、`fs` |
| `domain` | `repository`、`service`、`handler`、`ui`、`webdav`、`middleware`、`tasks`、`sandbox`、`app`、`routes` |
| `fs` | `service`、`handler`、`ui`、`webdav`、`middleware`、`sandbox`、`app`、`routes` |
| `sandbox` | `service`、`handler`、`ui`、`webdav`、`middleware`、`tasks`、`app`、`routes` |
| `service` | `handler`、`ui`、`webdav`、`middleware`、`settings`、`app`、`routes` |
| `tasks` | `handler`、`ui`、`webdav`、`middleware`、`app`、`routes` |
| `handler` | `repository`、`sandbox`、`app`、`routes` |
| `middleware` | `handler`、`ui`、`webdav`、`tasks`、`sandbox`、`app`、`routes` |
| `ui` | `handler`、`repository`、`tasks`、`sandbox`、`fs`、`app`、`routes` |
| `webdav` | `handler`、`ui`、`repository`、`app`、`routes` |
| `settings` | `handler`、`ui`、`webdav`、`middleware`、`app`、`routes` |

未列出的方向允许（例如 `service → repository`、`handler → service`、`repository → domain`）。以下四个例外是刻意保留的，理由写在守卫脚本里：

- `fs/core/gc.rs` 接收任务的 `JobContext` 用于进度与取消：GC 本身就是一个 job。
- `handler` 的协议/页面适配器（`handler/sync/`、`handler/web/{share_view,upload,zip_download,download}.rs`）直接使用块与树函数，而不是绕一层 service。
- `handler/async_batch.rs` 与 `handler/reindex.rs` 提交并读取后台作业。
- `service/{face_tags,index,mail}` 是作业体与索引调度器；任务系统是 service 层的执行器。

`node tools/check-layering.mjs --report` 会打印已知债务基线；新违规即使旧问题还没清完也会让门禁失败。基线按「方向 + 文件」登记：`ui → repository`（管理页与页面处理直接读取要渲染的行）、`ui → tasks`（`ui/admintasks.rs`）、`ui → fs`（文件页与回收站页）、`ui → sandbox`（设置页的沙箱探针）以及 `webdav → repository`（树辅助函数）。基线之外的文件会失败，因此债务无法增长；`--report` 还会列出不再命中的基线项，可以直接删除。

## 请求流

请求先经过 `middleware`：凭据被分类为会话、API 密钥或同步令牌，路由所需的能力在**一张表**里查找。然后进入 `handler`，由它调用 `service`；service 通过 `repository` 与 `fs` 访问数据库和块存储。`infra` 提供配置、存储与加密。

能力表让鉴权保持统一：会话满足所有能力，密钥只携带被授予的能力，未分类的路由一律拒绝而不是放行。同步侧见[Seafile 兼容性](/zh/dev/protocol)。

协议适配器是唯一被允许的“第二入口”：同步协议、WebDAV 与匿名分享/上传页面可以直接调用 `fs` 引擎的树与块函数，因为这些 handler 本身就是协议。业务写仍属于 service；handler 不直接碰 repository。

## 持久化与事务

`repository` 是 CRUD 边界：每个聚合一个 trait、一个 SeaORM 实现、认证缓存的装饰器，以及活动日志写入。但它不是唯一的数据库路径：`fs` 在跨表操作（树更新 + 提交 + HEAD、归档 + 删除、回收站恢复）中也会直接读写实体；`tasks::setup`/`indexer` 为自己的簿记使用连接。

事务边界遵循一条规则：**多表不变量由拥有事务的单个 repository 方法写入**，而不是由 service 组合多次 repository 调用。例子有 `history_prune`（一次清理的 commit 与文件对象行）、`provisioning`（资料库行 + 所有者成员关系）与 `head_commit`（提交行 + HEAD 的 CAS）。没有通用 Unit of Work；必须原子的流程就得到一个窄方法。允许滞后的工作（活动日志、索引调度）在事务之后尽力而为。

## 后台任务

`tasks` 是一个作业注册表、一个有界运行存储和一个执行器。工作从两条路进入：

- `TaskSystem::submit` 调度之后会按 id 查询的工作。它产生可轮询的作业运行，可取消的作业还能停止。
- `TaskSystem::run_inline` 在作业的并发限制下运行请求内工作，但不留记录，也不继承作业的超时。

常驻事件监听没有所有者、进度或终态，因此它是服务而不是作业。作业自己声明是每次运行都记录（`EveryRun`），还是只记录有内容可报的运行（`Notable`）；手动触发的运行总是记录，失败也总是记录。

## 设置

`infra::settings::catalog` 是 `Config` 字段的唯一目录。每个条目写明它的 `NANOFILE_*` 变量、如何解析，以及修改后何时生效。环境变量层就是按这张表应用的，因此新增设置不可能漏接变量，管理界面也不可能宣传服务器不读的变量。可替换的快照本身（`RuntimeConfig`）位于 `infra::settings`；`server::settings` 负责分层与持久化保存。见[配置](/zh/guide/configure/configuration)。

## 构建与前端

`server/build.rs` 编译 Tailwind 样式表，并用 esbuild 打包前端入口。改前端后需要重新 `cargo build`，没有热重载。环境要求、`tray` feature 与交叉编译见[构建](/zh/dev/build)，前端约定见[前端](/zh/dev/frontend)。

## 相关

- [构建](/zh/dev/build) — 从源码编译：工具链、前端资源、交叉编译目标。
- [Seafile 兼容性](/zh/dev/protocol) — 提交 id、`fs_id`、能力。
- [沙盒内部](/zh/dev/sandbox) — 受限 worker。
- [测试](/zh/dev/testing) — 测试套件与平台矩阵。
- [CI 与发布](/zh/dev/ci-releases) — 工作流。
