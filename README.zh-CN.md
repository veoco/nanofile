# Nanofile

[English](README.md) | [简体中文](README.zh-CN.md)

Nanofile 是一个可自行部署的文件同步与分享服务。它兼容 Seafile 的同步协议与接口，官方桌面端、移动端客户端可直接连接；同时自带网页界面，无需另装 seaf-server + seahub。

**文档：** <https://veoco.github.io/nanofile/zh/> · [English](https://veoco.github.io/nanofile/)

## 功能

- **文件同步** —— 官方 Seafile 桌面端与移动端客户端自动同步资料库。
- **网页端** —— 浏览、预览、下载、收藏，以及动态与回收站。
- **分享** —— 分享链接支持密码、过期时间与查看次数；支持匿名上传链接。
- **登录安全** —— 两步验证（TOTP）、受信设备、登录失败锁定与 API 密钥。
- **管理后台** —— 用户、配额、邮件、系统设置与后台任务。
- **受限解析** —— 文档、图片、EXIF 与媒体解析都在有界子进程中运行。

## 快速开始

环境要求：稳定的 Rust 工具链（edition 2024）与 Node.js（CI 使用 24）。`ffmpeg` 可选，用于媒体缩略图。

```bash
npm install                       # 前端打包工具（esbuild 必需）
cargo build --release -p server   # 产物是 target/release/nanofile
./target/release/nanofile
```

既没有配置文件、环境里也没有任何 `NANOFILE_*` 变量时，会进入首次初始化：起一个一次性页面 `http://127.0.0.1:8082/setup/<token>`，创建第一个管理员，写出 `~/.nanofile/config.toml`，并在同一个端口上启动服务。删掉该文件即可重新初始化。

如果想自己管理配置，只要存在配置文件就不会进入初始化：

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # 文件里存有主密钥
./target/release/nanofile --config config.toml
```

服务监听 <http://localhost:8082>。创建管理员（首次初始化时设置页会替你做）：

```bash
./target/release/nanofile adduser --email admin@example.com
```

`--regular` 创建普通用户。更多见[快速开始](https://veoco.github.io/nanofile/zh/guide/getting-started)。

## Docker

镜像为 `scratch` 容器，仅含二进制，以 uid/gid `1000:1000` 运行，挂载的数据卷需对该用户可写。

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret

docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

主密钥生成一次并长期保存：更换后所有会话登出、同步客户端失效、已加密块无法解密。配置文件方式、容器内首次初始化的注意事项与升级权限见 [Docker 部署](https://veoco.github.io/nanofile/zh/guide/docker)。

## 文档

| | |
|---|---|
| [快速开始](https://veoco.github.io/nanofile/zh/guide/getting-started) | 构建、首次运行、管理员 |
| [配置](https://veoco.github.io/nanofile/zh/guide/configuration) | 配置文件、`NANOFILE_*`、设置页 |
| [沙盒](https://veoco.github.io/nanofile/zh/guide/sandbox) | 约束了什么、没约束什么 |
| [安全](https://veoco.github.io/nanofile/zh/guide/security) | 反向代理、TLS、主密钥 |
| [命令行](https://veoco.github.io/nanofile/zh/reference/cli) | 全部运维命令 |
| [开发](https://veoco.github.io/nanofile/zh/dev/architecture) | 架构、测试、CI |

## 开发

项目是包含四个 crate 的 Cargo workspace：`base`（领域类型）、`infra`（数据库、存储、加密、配置）、`server`（HTTP、同步协议、WebDAV、网页界面）与 `migration`。依赖方向 `base → infra → server`。

```bash
cargo test --workspace
node --test "server/frontend/**/*.test.js"
```

浏览器套件、平台矩阵与依赖规则见[测试](https://veoco.github.io/nanofile/zh/dev/testing)与[贡献指南](https://veoco.github.io/nanofile/zh/dev/contributing)。

## 许可证

MIT
