# 架构

Nanofile 是一个包含四个 crate 的 Cargo workspace。依赖方向是单向的：`base → infra → server`；`migration` 由 `server` 使用。

| Crate | 职责 |
|---|---|
| `base` | 纯领域类型、`AppError`、路径规范化。默认不依赖 axum 或数据库，除非启用 `with-axum` / `with-sea-orm` feature。 |
| `infra` | 数据库实体与连接、配置、加密、块存储、序列化、权限、限流、搜索索引、设置目录及其事件。 |
| `server` | HTTP handler 与路由、service、`AppState`、Seafile 同步协议、WebDAV、网页界面、沙盒、任务与可选托盘。 |
| `migration` | SeaORM 数据库迁移。 |

## `server` 内部

| 模块 | 职责 |
|---|---|
| `handler` | HTTP 端点：同步 API、REST API、网页、分享、上传。 |
| `service` | 业务逻辑：认证、同步、文件、分享、动态、邮件、搜索、通知。 |
| `repository` | 账号、令牌、提交、文件与分享的数据库访问。 |
| `domain` | 共享领域类型：凭据、能力、提交、文件系统对象。 |
| `fs` | Seafile 文件系统对象：树操作、提交、合并、回收站、垃圾回收、块校验与迁移。 |
| `ui` | Askama 渲染的网页，包括管理后台。 |
| `middleware` | 认证与能力、CSP、CSRF、请求上限。 |
| `webdav` | 建立在同一套资料库与文件服务之上的 WebDAV 接口。 |
| `indexer` | 全文索引（Tantivy）与喂给它的沙盒提取器。 |
| `sandbox` | 解析用户内容的受限子进程。 |
| `tasks` | 后台作业的注册表、队列、执行器与运行历史。 |
| `settings` | 运行时配置、设置分层与管理端设置服务。 |
| `startup`、`tray` | 自动启动注册与可选的桌面托盘。 |
| `restart`、`serve`、`app`、`routes` | 进程生命周期、监听器、路由与应用状态。 |

## 请求流

请求先经过 `middleware`：凭据被分类为会话、API 密钥或同步令牌，路由所需的能力在**一张表**里查找。然后进入 `handler`，由它调用 `service`；service 通过 `repository` 与 `fs` 访问数据库和块存储。`infra` 提供配置、存储与加密。

能力表让鉴权保持统一：会话满足所有能力，密钥只携带被授予的能力，未分类的路由一律拒绝而不是放行。同步侧见[Seafile 兼容性](/zh/dev/protocol)。

## 后台任务

`tasks` 是一个作业注册表、一个有界运行存储和一个执行器。工作从两条路进入：

- `TaskSystem::submit` 调度之后会按 id 查询的工作。它产生可轮询的作业运行，可取消的作业还能停止。
- `TaskSystem::run_inline` 在同样的并发限制与超时下运行请求内工作，但不留记录。

常驻事件监听没有所有者、进度或终态，因此它是服务而不是作业。作业自己声明是每次运行都记录（`EveryRun`），还是只记录有内容可报的运行（`Notable`）；手动触发的运行总是记录，失败也总是记录。

## 设置

`infra::settings::catalog` 是 `Config` 字段的唯一目录。每个条目写明它的 `NANOFILE_*` 变量、如何解析，以及修改后何时生效。环境变量层就是按这张表应用的，因此新增设置不可能漏接变量，管理界面也不可能宣传服务器不读的变量。见[配置](/zh/guide/configuration)。

## 构建与前端

`server/build.rs` 编译 Tailwind 样式表，并用 esbuild 打包前端入口。改前端后需要重新 `cargo build`，没有热重载。环境要求、`tray` feature 与交叉编译见[构建](/zh/dev/build)，前端约定见[前端](/zh/dev/frontend)。

## 相关

- [构建](/zh/dev/build) — 从源码编译：工具链、前端资源、交叉编译目标。
- [Seafile 兼容性](/zh/dev/protocol) — 提交 id、`fs_id`、能力。
- [沙盒内部](/zh/dev/sandbox) — 受限 worker。
- [测试](/zh/dev/testing) — 测试套件与平台矩阵。
- [CI 与发布](/zh/dev/ci-releases) — 工作流。
