# 贡献指南

## 构建与验证

```bash
npm ci
cargo build --bin nanofile
```

仓库的验证命令，按 CI 的顺序：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p server --features tray --bin nanofile -- -D warnings
cargo test -p server --features tray --bin nanofile
cargo machete
NANOFILE_TEST_FFMPEG_PATH="$(command -v ffmpeg)" cargo test --workspace --no-fail-fast
node --test "server/frontend/**/*.test.js"
cargo audit
```

浏览器套件见[测试](/zh/dev/testing)。CI 是权威；它还会跑单机无法覆盖的平台 lint 与交叉目标构建。

## 注释风格

注释的价值在于说出代码说不出的东西。具体来说：

- **保留**不变量、安全依据、`unsafe` 前置条件、并发与生命周期推理、错误/panic 契约、Seafile 客户端怪癖、迁移与线格式说明、`#[allow(...)]` 的理由以及 doctest。
- **删除**复述名字或控制流的注释、历史叙述（"used to"、"before this existed"）、测试的逐步旁白与装饰横幅。
- **压缩**模块文档到"目的 + 关键不变量 + 本站相关页面的链接"。多段设计说明放 `docs/dev/`。
- 不把本机路径、作者名或日期写进注释。

CI 会跑带 `-D warnings` 的 `cargo doc`，因此失效的文档内链接或 Markdown 解析错误的注释会让构建失败。删掉一个 doctest 时必须保留等价测试。

## 依赖规则

依赖图是构建契约的一部分：

- 不要打开 dev `reqwest` 的默认特性（它的 TLS 会拽入 `aws-lc-rs`）或 dev `image` 的默认格式（会拽入 `rav1e`/`ravif`）。
- TLS 只由 `lettre` 使用（SMTP，rustls/ring）。`sqlx`/`sea-orm` 使用 `runtime-tokio`，并且只连 SQLite。
- `ci.yml` 会在 `cargo machete` 发现未使用的直接依赖，或 `aws-lc-rs`、`aws-lc-sys`、`rav1e`、`ravif` 重新出现在依赖图里时失败。

## 文档

站点在 `docs/`，用 VitePress 构建。每个页面都有英文原文与 `docs/zh/` 下的简体中文镜像；在同一个提交里一起改。

```bash
npm ci --prefix docs
npm run --prefix docs dev       # http://localhost:5173/nanofile/
npm run --prefix docs build     # 死链会失败
npm run --prefix docs preview   # http://localhost:4173/nanofile/
```

- `base` 是 `/nanofile/`；fork 需要改 `docs/.vitepress/config.mts`。
- 站内链接写 `/guide/<章节>/<页面>`（英文）或 `/zh/guide/<章节>/<页面>`（中文）。指南按章节目录组织——`start`、`deploy`、`configure`、`admin`、`operate`——新页面放进对应章节目录。指向 `docs/` 之外的文件时用 GitHub URL，不要用相对路径。
- 正文里的 `<owner>` 这类尖括号占位符要放进反引号；代码块之外会被当成 HTML。
- `docs/public/favicon.svg` 是 `server/static/img/favicon.svg` 的副本。

## 提交信息

主题行用英文，保持祈使语气。一句主题加几条"做了什么"的要点即可。不要在提交信息里记录本地测试结果、数量或耗时；那些留在对话或 PR 里。
