# 前端

网页由 Askama 服务端渲染，再用模块化 JavaScript 渐进增强。改前端后需要重新 `cargo build`，没有热重载。

## 模板与样式

- 模板在 `server/templates/`，由 `server/src/ui/` 渲染。
- Tailwind 输入是 `server/static/css/input.css`；生成的样式表是 `server/static/css/app.css`。
- favicon 与生成的托盘图标来自 `server/static/img/favicon.svg`，在编译期栅格化。

## JavaScript

- 源模块在 `server/frontend/`（`core/`、`browser/`、`pages/`）。
- 入口是 `server/frontend/entries/*.js`。
- `server/build.rs` 用 esbuild 把每个入口打包成 `server/static/js/*.bundle.js`，并把结果嵌进二进制。
- esbuild 必需。Tailwind 可选；缺少它时构建回退到占位样式表，界面无样式。

bundle 被编译进二进制，因此只改前端文件、不跑 `cargo build` 不会生效。

## 国际化

界面翻译在 `server/locales/`（`en.toml`、`zh.toml`，以及邮件模板 `email_en.toml` / `email_zh.toml`），通过 `include_str!` 编译进来。访客没有偏好时，`[ui] default_language` 决定回退语言。

## 测试

```bash
node --test "server/frontend/**/*.test.js"
```

浏览器端到端测试在 `e2e/`，通过 Playwright 运行；见[测试](/zh/dev/testing)。

## 相关

- [架构](/zh/dev/architecture) — 前端如何嵌入 server crate。
- [测试](/zh/dev/testing) — 完整套件与 CI 矩阵。
