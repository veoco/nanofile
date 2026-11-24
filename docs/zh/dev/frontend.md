# 前端

网页由 Askama 服务端渲染，再用模块化 JavaScript 渐进增强。改前端后需要重新 `cargo build`，没有热重载。

## 模板与样式

- 模板在 `server/templates/`，由 `server/src/ui/` 渲染。
- Tailwind 输入是 `server/static/css/input.css`；生成的样式表是 `server/static/css/app.css`。
- favicon 以原始 SVG 内嵌并提供（源文件是 `server/static/img/favicon.svg`）。只有 `tray` 构建会在编译期把它栅格化，用于托盘图标与 Windows exe 图标。

## JavaScript

- 源模块在 `server/frontend/`（`core/`、`browser/`、`pages/`）。
- 入口是 `server/frontend/entries/*.js`。
- `server/build.rs` 用 esbuild 把每个入口打包成 `server/static/js/*.bundle.js`，并把结果嵌进二进制。
- esbuild 必需。Tailwind 可选；缺少它时构建回退到占位样式表，界面无样式。

bundle 被编译进二进制，因此只改前端文件、不跑 `cargo build` 不会生效。

auth 页还会加载单独的 `pow.bundle.js`；只有服务端渲染了 proof-of-work 元素时它才起作用。它是自托管挑战的浏览器端：以限时的小块循环求解 `SHA-256(seed || nonce)`，然后把结果填进隐藏的 nonce 字段。它的 SHA-256 与求解器在 `frontend/core/pow.js`，由 Node 的 `--test` 覆盖；没有渲染挑战时这个入口什么都不做。

## 国际化

界面翻译在 `server/locales/`（`en.toml`、`zh.toml`，以及邮件模板 `email_en.toml` / `email_zh.toml`），通过 `include_flate!` 以 DEFLATE 压缩编译进二进制。访客没有偏好时，`[ui] default_language` 决定回退语言。

## 测试

```bash
node --test "server/frontend/**/*.test.js"
```

浏览器端到端测试在 `e2e/`，通过 Playwright 运行；见[测试](/zh/dev/testing)。

## 相关

- [架构](/zh/dev/architecture) — 前端如何嵌入 server crate。
- [测试](/zh/dev/testing) — 完整套件与 CI 矩阵。
