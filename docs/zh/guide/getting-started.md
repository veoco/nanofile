# 快速开始

Nanofile 的服务端是一个名为 `nanofile` 的二进制（不是 `server`）。它兼容 Seafile 的同步协议，官方客户端可以直接连接，同时自带网页界面。

## 环境要求

- 稳定的 Rust 工具链。workspace 使用 edition 2024。
- Node.js，用于构建前端资源。CI 使用 Node 24。
- 可选：`ffmpeg`，用于媒体缩略图。缺少它时媒体缩略图不可用，媒体测试会被跳过。

## 从源码构建

```bash
npm install                       # 前端打包工具（esbuild 必需）
cargo build --release -p server   # 产物是 target/release/nanofile
./target/release/nanofile
```

## 首次运行

既没有指定配置文件、环境里也没有任何 `NANOFILE_*` 变量时，这属于尚未初始化的安装，不会直接启动服务，而是进入**首次初始化**：

1. 起一个一次性页面，把地址打印出来，例如 `http://127.0.0.1:8082/setup/<token>`。桌面版还会自动用浏览器打开。
2. 在页面里创建第一个管理员，并决定监听地址、外部访问地址、数据目录，以及是否对文件块做静态加密。
3. 提交后写出 `~/.nanofile/config.toml`（仅属主可读写），并在同一个端口上启动服务。

之后每次启动都读这个文件；删掉它即可重新初始化。

## 自己管理配置文件

只要存在配置文件，就不会进入首次初始化。这是把配置留在源码树里、或做非交互部署的方式：

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # 文件里存有主密钥
./target/release/nanofile --config config.toml
```

服务默认监听 `http://localhost:8082`。

必须有管理员账号。首次初始化时设置页会创建；已有配置时用 CLI：

```bash
# 交互式输入密码
./target/release/nanofile adduser --email admin@example.com

# 或免交互
printf '%s\n' 'secret123' | ./target/release/nanofile adduser --email admin@example.com --password-stdin
```

`--regular` 创建普通用户账号。其余命令见[命令行参考](/zh/reference/cli)。

## 下一步

- [配置](/zh/guide/configuration) — 配置来源、优先级与重启规则。
- [Docker 部署](/zh/guide/docker) — 已发布的镜像。
- [安全](/zh/guide/security) — 反向代理、`site_url`、主密钥。
- [数据目录](/zh/guide/data-directory) — 数据存放在哪里。
