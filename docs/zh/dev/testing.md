# 测试

## Rust

```bash
cargo test --workspace
```

它会跑单元测试、`server/tests/` 下的集成测试与文档测试。workspace 套件是主门禁。`server/tests/docs_api_reference_test.rs` 把 API 参考与路由表、被链接的显式锚点钉在一起，见 [API 参考](/zh/reference/api/)。

可选的 tray feature 默认不编译，因此它的单元测试要显式开启：

```bash
cargo test -p server --features tray --bin nanofile
```

## 前端

```bash
node --test "server/frontend/**/*.test.js"
```

## 浏览器端到端

e2e 在真实浏览器里驱动一个真实服务器。先构建 debug 二进制，因为测试会启动它：

```bash
npm ci
cargo build --bin nanofile
npm ci --prefix e2e
npx --prefix e2e playwright install chromium
npm run --prefix e2e test
```

`e2e/global-setup.ts` 启动服务器，`e2e/global-teardown.ts` 停止它。全新机器上浏览器缺系统库时，再跑 `npx --prefix e2e playwright install-deps chromium`。

## 媒体测试与 ffmpeg

媒体测试需要真实的 `ffmpeg`。用 `NANOFILE_TEST_FFMPEG_PATH` 指给它：

```bash
NANOFILE_TEST_FFMPEG_PATH="$(command -v ffmpeg)" cargo test --workspace --no-fail-fast
```

没有 ffmpeg 时媒体测试会被跳过。CI 在每个 runner 上都安装 ffmpeg，因此媒体断言是在 CI 上真正测过的，而不是被静默跳过。

## 沙盒 probe

`server/tests/sandbox_probe_test.rs` 断言每个沙盒 profile 的类型化报告、平台机制 token、解析器的产物与整条媒体流水线。它检查什么见[沙盒内部](/zh/dev/sandbox)。

## CI 跑什么

| Job | 平台 | 内容 |
|---|---|---|
| `lint` | Linux | `cargo fmt --check`、clippy、rustdoc、依赖守卫、审计 |
| `lint-platforms` | macOS、Windows | 针对平台 `cfg` 代码的 clippy 与 rustdoc |
| `tray-check` | Linux、macOS、Windows | `tray` feature 的 clippy、rustdoc 与单元测试 |
| `tests` | Linux x86_64/arm64、macOS arm64、Windows amd64 | 完整 Rust 套件 |
| `cross-check` | Linux | 编译并 lint 四个没有 runner 的目标 |
| `frontend-test` | Linux | `node --test` |
| `e2e` | Linux | Playwright |
| `audit` | Linux | `cargo audit` |

完整定义见 [`ci.yml`](https://github.com/veoco/nanofile/blob/main/.github/workflows/ci.yml)。工作流整体布局见 [CI 与发布](/zh/dev/ci-releases)。
