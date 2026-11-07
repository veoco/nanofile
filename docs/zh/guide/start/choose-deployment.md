---
description: 在桌面托盘、无头服务器服务和 Docker 之间做选择。
---
# 选择部署方式

**适合谁：** 第一次安装、正在选择运行方式的人。

Nanofile 只有一个二进制，但有三种常见运行方式。按机器类型和无人值守程度选一种。

## 三种形态

| 形态 | 适合 | 从这里开始 |
|---|---|---|
| **桌面托盘** | 笔记本或桌面，希望有托盘菜单和登录启动 | 从[安装与首次运行](/zh/guide/start/getting-started)下载 `-tray` 归档 |
| **无头服务器** | Linux 或 macOS 主机，开机自启、不需要桌面 | [后台运行与自启动](/zh/guide/deploy/running-in-background) |
| **Docker** | 容器主机、compose 文件或隔离部署 | [Docker 部署](/zh/guide/deploy/docker) |

Windows 是特例：每个 Windows 构建都有服务命令，`-tray` 构建多一个托盘菜单。见
[Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service)。

## 桌面托盘

当 Nanofile 跑在你正在用的机器上，并且你想用托盘图标打开网页、打开配置文件、登录启动或退出时，用 `-tray` 归档。Windows 上托盘还可以安装一个开机即启动、不需要登录的服务。

- **适合：** 桌面会话里的家庭服务器、带桌面的 NAS、评估 Nanofile。
- **不适合：** 没有桌面会话的无头主机，或必须在任何人登录前启动的服务器。那种情况用服务。
- **注意：** Linux 上托盘是会话总线上的 StatusNotifierItem 服务，桌面必须提供宿主（KDE 原生支持，GNOME 需装 AppIndicator 扩展）。

## 无头服务器

在服务管理器下运行普通归档。服务器默认把日志写到 stdout，因此 journald 或 launchd 可以直接收集。

- **Linux：** systemd unit、`Restart=always`、专用用户。
- **macOS：** 开机启动用 LaunchDaemon，用户会话用 LaunchAgent。
- **Windows：** Service Control Manager（`nanofile service …`）。
- **适合：** 服务器、虚拟机、没有桌面的 NAS，或任何无人值守的机器。
- **不适合：** 不能安装服务、也不能保持进程运行的主机。

## Docker

已发布的镜像是只含二进制的 `scratch` 容器，没有 shell、没有 `HOME`、没有包管理器；它需要的一切都必须通过 `NANOFILE_*` 变量或挂载的配置文件传入。

- **适合：** 容器主机、compose、可复现部署和干净的升级路径。
- **不适合：** 不能挂载可写数据卷，或不能运行容器的主机。
- **注意：** 镜像以 uid/gid `1000:1000` 运行；数据卷必须对该用户可写。

## 每种形态都需要什么

- 服务用户可读写的目录。默认是二进制旁的 `data/`；首次初始化会选 `~/.nanofile/data`。
- `server.secret_key` 或 `NANOFILE_SERVER_SECRET_KEY` 里的主密钥。
- 备份计划；见[备份、恢复与升级](/zh/guide/operate/backup-and-upgrade)。
- 如果服务器能从公网访问，通过反向代理走 HTTPS；见 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy)。
- 可选：`ffmpeg`，用于媒体缩略图，以及在做人脸检测和 OCR 前解码 HEIC/HEIF/AVIF。

## 一分钟决定

- **“我只想在这台笔记本上试试。”** 用 `-tray` 归档和首次初始化。以后可以再搬到服务里。
- **“它要在 Linux 或 macOS 服务器上开机自启。”** 用普通归档加 systemd unit 或 LaunchDaemon。
- **“它跑在容器里。”** 用 Docker 镜像和 compose 文件。
- **“这是一台没有用户登录的 Windows 机器。”** 安装 Windows 服务；托盘构建可以从菜单里做。

## 下一步

- [安装与首次运行](/zh/guide/start/getting-started) —— 端到端教程。
- [Docker 部署](/zh/guide/deploy/docker) —— 镜像、数据卷与 compose。
- [后台运行与自启动](/zh/guide/deploy/running-in-background) —— systemd 与 launchd。
- [Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service) —— Windows 服务与托盘菜单。
