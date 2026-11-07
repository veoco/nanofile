# 构建

本页是从源码编译 Nanofile 的地方。只想把服务跑起来，不需要编译：见指南的[安装与首次运行](/zh/guide/start/getting-started)。

## 环境要求

- **稳定的 Rust 工具链。** workspace 使用 edition 2024。
- **Node.js 与 npm。** 只为提供 `esbuild` 与 Tailwind，见下节。CI 使用 Node 24。
- **可选：`ffmpeg`。** 只影响媒体缩略图；缺少它时媒体缩略图不可用，媒体测试会被跳过。
- **仅 `tray` 构建：** Windows 需要资源编译器；Linux 不需要额外系统包。见下面「可选 feature：`tray`」一节。

## 前端资源怎么进二进制

`server/build.rs` 在每次构建前做三件事：

1. **Tailwind CSS**（可选）—— 把 `static/css/input.css` 编译成 `static/css/app.css`。找不到 Tailwind 时写入一张占位样式表，构建照常通过，但界面没有样式。
2. **esbuild**（必需）—— 把 `frontend/entries/` 下的入口打包成 `static/js/*.bundle.js`。找不到 esbuild 时构建**直接失败**（内联在模板里的脚本依赖这些产物）。
3. **托盘图标**（仅 `tray` feature）—— 从 `static/img/favicon.svg` 光栅化出各尺寸图标，在 Windows 上再嵌入 exe 资源。

两者都先在 crate 根目录与 workspace 根的 `node_modules/.bin/` 里找，再退回 `PATH`。因此下面这条 `npm ci` 只是为了把这两个工具放进去。

改完前端必须重新 `cargo build`，没有热重载。前端本身的约定见[前端](/zh/dev/frontend)。

## 基本构建

```bash
npm ci                            # esbuild 与 Tailwind
cargo build --release -p server   # 产物是 target/release/nanofile
./target/release/nanofile
```

日常开发用 debug 构建更快：

```bash
cargo build --bin nanofile        # 产物是 target/debug/nanofile
```

两种 profile 在启动行为上有差别：release 构建在没有 `server.secret_key` 时**拒绝启动**，debug 构建会临时生成一个，因此会话无法跨重启保留。见[安全](/zh/guide/admin/security)。

## 二进制体积与性能探针

`[profile.release]` 已经启用 fat LTO、单 codegen unit 与 `strip`。刻意不使用 `panic = "abort"`，因为任务执行器与索引提取依赖捕获 panic。依赖变更导致体积上涨时，用下面的命令看各 crate 的占比：

```bash
cargo bloat --release --crates -n 60 -p server --bin nanofile
```

release 二进制把 Web 资源与语言包用 DEFLATE（`include-flate`）内嵌，内容分析器是无词典的 CJK bigram，因此不携带词表，也不链接 C 压缩库。`include-flate` 的传递特性必须保持只有 `deflate`：打开 `zstd` 会把 `zstd-sys` 编进每次构建，CI 会拒绝。

热路径探针在 `server/tests/perf_probe.rs`，带 `#[ignore]`，因此从不在 CI 里跑；性能改动前后各跑一次并对比 `perf ...` 行（Linux 会打印进程 `VmRSS`）：

```bash
cargo test -p server --release --test perf_probe -- --ignored --nocapture
PERF_FILES=500 cargo test -p server --release --test perf_probe -- --ignored --nocapture
```

## 可选 feature：`tray`

`--features tray` 构建出带系统托盘菜单的二进制：

```bash
cargo build --release -p server --features tray
```

- **Linux** 不需要额外系统包：托盘图标是会话总线上的 StatusNotifierItem 服务，需要桌面提供宿主（KDE 原生支持，GNOME 需装 AppIndicator 扩展）。
- **Windows** 用 `winresource` 嵌入图标与版本资源，需要 `rc.exe` 或 `llvm-rc`。
- **macOS** 无额外系统依赖；托盘图标用模板图，由菜单栏自行反色。

发布产物里只有三个原生 runner 平台带 tray 变体（Linux amd64、macOS arm64、Windows amd64），交叉编译的行只构建无头二进制。托盘本身的行为在[Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service)。

## 嵌入推理（人物标签与 OCR）

所有构建都会编译 YuNet 检测器、PP-OCRv6 tiny 的 OCR 图、PP-LCNet 页面与文本行
方向分类器和自动人物标签流水线，没有需要开启的 Cargo feature。纯 Rust 的
`tract` CPU runtime 与 ONNX 模型（合计约 14.3 MB）始终在二进制里，而两个
扫描器仍要等 `[face_tags] enabled` 或 `[ocr] enabled` 打开后才运行。正常构建即可：

```bash
cargo build --release -p server
```

模型就是上游 ONNX 原件：YuNet 为 MIT，PaddleOCR 的模型为 Apache-2.0，已签入
`server/models/`，同目录有模型卡、许可证和 SHA-256。`tools/models/fetch_models.py`
下载固定 revision、校验哈希并抽取识别字典。构建期和运行期都没有权重转换、
ONNX Runtime、Python 或 BLAS。CI 默认的 `cargo clippy`/`cargo test` 与
release/nightly 构建都会包含它；发布探针会跑
`extract-worker --probe --profile faces` 和 `--profile ocr`。

两个功能都与 `tray` 互相独立；发布版托盘变体使用 `--features tray`。

## 交叉编译

Linux 目标用 `cargo-zigbuild` 链接：

```bash
cargo zigbuild --release --target aarch64-unknown-linux-musl
```

| 目标 | 用途 |
|---|---|
| `x86_64-unknown-linux-gnu` / `-musl` | Linux amd64 |
| `aarch64-unknown-linux-gnu` / `-musl` | Linux arm64 |
| `loongarch64-unknown-linux-gnu` / `-musl` | Linux LoongArch64 |
| `x86_64-pc-windows-msvc` | Windows amd64 |
| `aarch64-apple-darwin` | macOS Apple Silicon |

工具链是固定的：`cargo-zigbuild@0.23.4` 配 zig `0.16.0`，且归档按 SHA-256 校验后才解压——被替换的 CDN 或 tarball 无法把工具链塞进构建。zig 0.15+ 把产出二进制的基线抬到了 glibc 2.31 / Linux 5.10，要让更老的主机继续可用就加后缀，例如 `--target aarch64-unknown-linux-gnu.2.28`。

musl 产物是静态链接的，容器镜像（`FROM scratch`）用的就是它们。

## 验证

改动提交前跑的命令，以及 CI 的等价步骤，见[贡献指南](/zh/dev/contributing)与[测试](/zh/dev/testing)。

## 相关

- [架构](/zh/dev/architecture) — crate 布局与构建脚本在其中的位置。
- [前端](/zh/dev/frontend) — 模板、样式与 JavaScript 约定。
- [测试](/zh/dev/testing) — Rust、前端与浏览器套件。
- [CI 与发布](/zh/dev/ci-releases) — 各平台构建、沙盒冒烟测试与发布产物。
- [安装与首次运行](/zh/guide/start/getting-started) — 不编译，直接运行。
