---
description: 安装 Nanofile、创建第一个管理员、上传并分享文件、连接客户端。
---
# 安装与首次运行

本教程把一台新机器带到可用的 Nanofile 服务器：一个管理员、第一个资料库、一个上传的文件、一条分享链接，以及一个连上的客户端。发布归档和 Docker 镜像二选一，选一条走完。

**时间：** 约 15 分钟。**适合谁：** 第一次自托管 Nanofile 的人。

## 最终会得到什么

- Nanofile 监听在 `http://localhost:8082`。
- 一个管理员账号和一个配置文件。
- 一个资料库，里面有一个上传的文件。
- 一条能在隐私窗口打开的分享链接。
- 已登录并同步该资料库的官方 Seafile 客户端。

## 前置条件

- 一台 Linux、macOS 或 Windows 主机、虚拟机或容器主机。生产环境优先 Linux；发布归档覆盖这三个操作系统。
- 一个存放数据库、块、缩略图和索引的目录。默认是二进制旁的 `data/`。
- 可选：`ffmpeg`，用于媒体缩略图和 HEIC/HEIF/AVIF 解码。
- 如果服务器会从公网访问，还需要 DNS 名称和 TLS 证书。可以先做本地测试再加 HTTPS；见 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy)。

## 安装

### 发布归档

每个版本都会在 GitHub Releases 上发布各平台的归档，名字是
`nanofile-v<版本>-<后缀>.tar.gz`（Windows 是 `.zip`）。归档里有 `nanofile`
（Windows 是 `nanofile.exe`）、`README.md`、`LICENSE` 与 `config.toml.example`。

| 后缀 | 适用 |
|---|---|
| `linux-amd64` | Linux x86_64，链接系统的 glibc |
| `linux-amd64-musl` | Linux x86_64，静态链接，不依赖主机 glibc |
| `linux-arm64`、`linux-arm64-musl` | Linux ARM64（同上两种链接方式） |
| `linux-loong64`、`linux-loong64-musl` | Linux LoongArch64 |
| `macos-arm64` | macOS Apple Silicon |
| `windows-amd64` | Windows x86_64（`.zip`） |
| `linux-amd64-tray`、`macos-arm64-tray`、`windows-amd64-tray` | 同上平台，但带系统托盘菜单 |

服务器用**不带 tray** 的变体；想从桌面托盘控制安装，选 `-tray` 变体（见
[Windows 服务与桌面托盘](/zh/guide/deploy/desktop-and-service)）。不确定或主机 glibc 较老时，选 `musl` 变体。

```bash
tar xzf nanofile-v1.2.3-linux-amd64.tar.gz
cd nanofile-v1.2.3-linux-amd64
chmod +x nanofile
./nanofile
```

首次运行下载的二进制时，两个平台会先拦一下：macOS 因未签名会被 Gatekeeper 拦截（在「系统设置 → 隐私与安全性」里允许，或 `xattr -d com.apple.quarantine nanofile`）；Windows 可能弹出 SmartScreen 提示，确认「仍要运行」即可。

### Docker

已发布的镜像是 `scratch` 容器，二进制在 `/nanofile`。它以 uid/gid `1000:1000` 运行，因此数据卷必须对该用户可写：

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret

docker run -d --name nanofile -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

这些变量视为已配置，因此容器不会进入一次性设置页。在容器里创建第一个管理员：

```bash
docker exec nanofile /nanofile adduser --email admin@example.com
```

生产部署请固定版本 tag，不要用 `latest`；配置文件方式、容器内首次初始化的注意事项与升级权限见 [Docker 部署](/zh/guide/deploy/docker)。

### 从源码编译

想自己编译而不是用发布版，见[构建](/zh/dev/build)。之后的操作与本页相同。

## 首次运行

既没有指定配置文件、环境里也没有任何 `NANOFILE_*` 变量时，这属于尚未初始化的安装，不会直接启动服务，而是进入**首次初始化**：

1. 起一个一次性页面，把地址打印出来，例如 `http://127.0.0.1:8082/setup/<token>`。桌面版还会自动用浏览器打开。
2. 在页面里创建第一个管理员，并决定监听地址、外部访问地址、数据目录，以及是否对文件块做静态加密。
3. 提交后写出 `~/.nanofile/config.toml`（仅属主可读写），并在同一个端口上启动服务。

之后每次启动都读这个文件；删掉它即可重新初始化。

页面地址是 `127.0.0.1` 且不带 TLS。在远程主机上部署时，不要把它直接暴露出去——先按 [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy)配好 `site_url` 与反向代理，或用 SSH 隧道访问。

**Docker：** 上面的命令已经设置了 `NANOFILE_*`，因此没有设置页。用 Docker 一节的 `docker exec … adduser` 创建管理员，然后打开 `http://localhost:8082`。

## 创建资料库并上传文件

1. 打开 `http://localhost:8082`，用管理员账号登录。
2. 在**资料库**页点**新建资料库**，起个名字并创建。
3. 打开资料库，点**上传文件**，选一个小文件，等进度面板走完。
4. 文件出现在列表里；双击它可以检查预览或下载。

如果看不到上传按钮，说明该账号对这个资料库只有只读权限。资料库的创建者有读写权限；其他用户要通过分享加入，本教程不展开。

## 分享文件

1. 打开文件的 **⋯** 菜单，选**分享链接**。
2. 需要的话设置密码和过期时间；分享链接还支持查看次数。
3. 复制链接，在隐私窗口打开，输入密码并下载文件。

这是公开分享的路径。**上传链接**同理，但让收件人向资料库上传。完整选项见[文件与分享](/zh/guide/use/files-and-sharing)。

## 连接客户端

官方 Seafile 桌面端与移动端可以直接连接 Nanofile。

1. 安装对应平台的官方 Seafile 客户端。
2. 服务器地址填 `server.site_url` 里的地址。本地测试就是你打开的地址，例如 `http://localhost:8082`。
3. 用管理员邮箱和密码登录。开启两步验证时输入 6 位动态口令，并可以信任该设备。
4. 添加刚创建的资料库，让客户端同步。
5. 往同步文件夹里放一个文件；它应该出现在网页界面里。

客户端会用同一份服务器地址去取块和上传，所以地址写错可能表现为能登录但同步不动。WebDAV 与 API 密钥是另外两种接入方式；见[客户端接入](/zh/guide/use/clients)。

## 验证

- `curl -fsS http://127.0.0.1:8082/health` 返回 200，响应体为空。
- 文件能从网页界面下载。
- 分享链接能在隐私窗口打开，密码有效。
- 客户端显示资料库已同步，两边任一侧新增的文件都会出现在另一侧。
- 服务端日志没有反复出现的错误。

## 自己管理配置文件

只要存在配置文件，就不会进入首次初始化。这是把配置留在源码树里、或做非交互部署的方式：

```bash
cp config.toml.example config.toml
chmod 600 config.toml             # 文件里存有主密钥
./nanofile --config config.toml
```

服务默认监听 `http://localhost:8082`。

必须有管理员账号。首次初始化时设置页会创建；已有配置时用 CLI：

```bash
# 交互式输入密码
./nanofile adduser --email admin@example.com

# 或免交互
printf '%s\n' 'secret123' | ./nanofile adduser --email admin@example.com --password-stdin
```

`--regular` 创建普通用户账号。也可以在配置文件里用 `[admin_init]`，让首次启动自动建管理员。其余命令见[命令行参考](/zh/reference/cli)。

## 下一步

- [HTTPS 与反向代理](/zh/guide/deploy/https-and-reverse-proxy) —— 暴露到公网前先套上 TLS。
- [后台运行与自启动](/zh/guide/deploy/running-in-background) —— systemd 或 launchd。
- [备份、恢复与升级](/zh/guide/operate/backup-and-upgrade) —— 需要之前先备份。
- [用户与分享](/zh/guide/admin/users-and-sharing) —— 账号、配额与分享策略。
- [自动人物标签](/zh/guide/use/people-tags) 与[图片与 PDF 文字识别](/zh/guide/use/ocr) —— 可选的媒体功能。
