---
description: Nanofile 指南：安装、使用与长期运维。
---
# Nanofile 指南

**适合谁：** 安装、使用或运维 Nanofile 的人。

Nanofile 是一个二进制：既实现 Seafile 同步协议，又提供自己的网页界面。本指南从首次运行讲到长期运维——安装、接入客户端、配置、管理账号，以及保持它健康运行。

指南跟随最新发布。`edge` 镜像可能包含尚未发布的改动；依赖新行为前先看发布说明。

## 从这里开始

1. **[什么是 Nanofile？](/zh/guide/start/what-is-nanofile)** —— 它做什么、与 Seafile 兼容到什么程度，以及它不做什么。
2. **[选择部署方式](/zh/guide/start/choose-deployment)** —— 桌面托盘、无头服务器或 Docker。
3. **[安装与首次运行](/zh/guide/start/getting-started)** —— 端到端教程：安装、创建第一个管理员、上传文件、分享并连接客户端。

## 常见任务

| 我想… | 去哪里 |
|---|---|
| 在这台机器上试试 | [安装与首次运行](/zh/guide/start/getting-started) |
| 连接 Seafile 桌面端或移动端 | [客户端接入](/zh/guide/use/clients) |
| 上传、下载、整理或分享文件 | [文件与分享](/zh/guide/use/files-and-sharing) |
| 开启两步验证或管理会话 | [账号安全](/zh/guide/use/account-security) |
| 放在 HTTPS 后面 | [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy) |
| 开机自启 | [后台运行与自启动](/zh/guide/deploy/running-in-background) 或 [Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service) |
| 备份、恢复或升级 | [备份、恢复与升级](/zh/guide/operate/backup-and-upgrade) |
| 开启人物标签或 OCR | [自动人物标签](/zh/guide/use/people-tags) 或 [图片与 PDF 文字识别](/zh/guide/use/ocr) |
| 排查问题 | [故障排查](/zh/guide/operate/troubleshooting) |

## 本指南的结构

- **上手** —— 评估、选择形态、安装。
- **使用 Nanofile** —— 网页界面、文件与分享、客户端、账号安全，以及可选的媒体功能。
- **部署** —— Docker、系统服务、Windows 服务与 HTTPS 反向代理。
- **配置** —— 配置文件、环境变量、设置页、数据目录与邮件。
- **管理与安全** —— 用户、配额、分享策略、主密钥与沙盒。
- **运维** —— 备份、升级、维护、监控与故障排查。

## 参考与开发

- [API 参考](/zh/reference/api/) —— 全部 HTTP 接口，以及与 Seafile 的对比。
- [命令行](/zh/reference/cli) —— 全部运维命令。
- [开发](/zh/dev/architecture) —— 架构、构建、测试与 CI。
