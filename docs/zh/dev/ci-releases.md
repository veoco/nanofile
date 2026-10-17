# CI 与发布

三个工作流负责构建与发布，另一个负责部署本站。

## `ci.yml`

在每次 push/PR 到 `main`、`master` 或 `develop` 时运行，也可以手动"Run workflow"。路径过滤器决定哪些 job 运行：

- **`lint`**（Linux）—— `cargo fmt --check`、整个 workspace 的 clippy、依赖守卫、rustdoc 与 `cargo audit`。
- **`lint-platforms`**（macOS、Windows）—— 针对 Linux runner 永远不会编译的 `cfg` 代码跑 clippy 与 rustdoc。
- **`tray-check`**（Linux、macOS、Windows）—— 可选 tray feature 的 clippy、rustdoc 与单元测试。
- **`tests`**（Linux x86_64 与 arm64、macOS arm64、Windows amd64）—— 完整 Rust 套件。
- **`cross-check`**（Linux）—— 编译并 lint 四个没有 runner 的 Linux 目标（`x86_64-unknown-linux-musl`、`aarch64-unknown-linux-musl`、`loongarch64-unknown-linux-gnu`、`loongarch64-unknown-linux-musl`）。
- **`frontend-test`** —— `node --test`。
- **`e2e`** —— 用 debug 二进制跑 Playwright。
- **`audit`** —— `cargo audit`。

强推（force-push）或手动触发会忽略过滤器跑全部 job，因为历史被改写后，与上一个提交的 diff 不可信。

### 固定 action

Action 固定到完整 commit SHA，tag 写在行尾注释里，因此被移动或劫持的 tag 无法向 CI 注入代码。Dependabot 刻意未启用：刷新 pin 时解析新 tag，并更新每一处出现。

### 依赖守卫

`lint` job 会在以下情况失败：

- 有未使用的直接依赖（`cargo machete`），或
- `aws-lc-rs`、`aws-lc-sys`、`rav1e`、`ravif` 重新进入依赖图。它们会通过 `reqwest` 的 TLS 与 `image` 的格式默认特性被拽回来，因此 `server/Cargo.toml` 的 dev-dependencies 关掉了这些默认特性。

`cargo audit` 忽略 `RUSTSEC-2023-0071`（`rsa` 里的 Marvin 攻击，由 `jsonwebtoken` 的 rust_crypto 特性引入）。Nanofile 只签发与校验 HS256 令牌，该公告在这里不可达；等 `jsonwebtoken` 支持算法级特性时再重新评估这个忽略。

## `nightly.yml`

每天 10:00 UTC 运行。定时运行先检查最新提交是否来自今天；手动运行跳过该检查。它构建多架构镜像并发布为 `ghcr.io/veoco/nanofile:edge`，同时对每个原生构建产物与已推送镜像的沙盒做冒烟测试（`extract-worker --probe`）。

## `release.yml`

在 `v*.*.*` tag 上运行，或手动指定版本。它构建每个平台与架构，对发布产物跑同一份沙盒 probe，构建多架构容器镜像并发布：

- 带版本号的镜像（`v<version>`、`v<major>.<minor>`，默认分支上还有 `latest`），
- 带归档与发布说明的 GitHub Release。

发布归档包含 README。

## `docs.yml`

每次 `docs/` 变更时构建本站，并在 `main` 上部署到 GitHub Pages。PR 只构建不部署，因此死链会让 PR 失败。本地命令见[贡献指南](/zh/dev/contributing)。

## 相关

- [测试](/zh/dev/testing) — 每个 job 跑什么。
- [贡献指南](/zh/dev/contributing) — 本地等价命令。
