# 安装与首次运行

Nanofile 的服务端是一个名为 `nanofile` 的二进制（不是 `server`）。它兼容 Seafile 的同步协议，官方客户端可以直接连接，同时自带网页界面。

## 安装

### 下载发布版归档

每个版本都会在 GitHub Releases 上发布各平台的归档，名字是 `nanofile-v<版本>-<后缀>.tar.gz`（Windows 是 `.zip`）。归档里有 `nanofile`（Windows 是 `nanofile.exe`）、`README.md`、`LICENSE` 与 `config.toml.example`。

| 后缀 | 适用 |
|---|---|
| `linux-amd64` | Linux x86_64，链接系统的 glibc |
| `linux-amd64-musl` | Linux x86_64，静态链接，不依赖主机 glibc |
| `linux-arm64`、`linux-arm64-musl` | Linux ARM64（同上两种链接方式） |
| `linux-loong64`、`linux-loong64-musl` | Linux LoongArch64 |
| `macos-arm64` | macOS Apple Silicon |
| `windows-amd64` | Windows x86_64（`.zip`） |
| `linux-amd64-tray`、`macos-arm64-tray`、`windows-amd64-tray` | 同上平台，但带系统托盘菜单 |

**选哪个：**

- 服务器或无头部署用**不带 tray** 的变体；想在桌面上用托盘菜单控制服务，选 `-tray` 变体（见[桌面托盘与 Windows 服务](/zh/guide/deploy/desktop-and-service)）。
- 不确定或主机 glibc 较老时选 `musl` 变体：它不依赖主机的 C 库。

```bash
tar xzf nanofile-v1.2.3-linux-amd64.tar.gz
cd nanofile-v1.2.3-linux-amd64
chmod +x nanofile
./nanofile
```

首次运行下载的二进制时，两个平台会先拦一下：macOS 因未签名会被 Gatekeeper 拦截（在「系统设置 → 隐私与安全性」里允许，或 `xattr -d com.apple.quarantine nanofile`）；Windows 可能弹出 SmartScreen 提示，确认「仍要运行」即可。

### Docker

已发布的镜像见[Docker 部署](/zh/guide/deploy/docker)：

```bash
docker run -d --name nanofile -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_SERVER_SECRET_KEY="$(openssl rand -hex 32)" \
  ghcr.io/veoco/nanofile:latest
```

### 从源码编译

想自己编译而不是用发布版，见开发部分的[构建](/zh/dev/build)。之后的操作与本页相同。

## 首次运行

既没有指定配置文件、环境里也没有任何 `NANOFILE_*` 变量时，这属于尚未初始化的安装，不会直接启动服务，而是进入**首次初始化**：

1. 起一个一次性页面，把地址打印出来，例如 `http://127.0.0.1:8082/setup/<token>`。桌面版还会自动用浏览器打开。
2. 在页面里创建第一个管理员，并决定监听地址、外部访问地址、数据目录，以及是否对文件块做静态加密。
3. 提交后写出 `~/.nanofile/config.toml`（仅属主可读写），并在同一个端口上启动服务。

之后每次启动都读这个文件；删掉它即可重新初始化。

页面地址是 `127.0.0.1` 且不带 TLS。在远程主机上部署时，不要把它直接暴露出去——先按[安全](/zh/guide/admin/security)配好 `site_url` 与反向代理，或用 SSH 隧道访问。

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

- [网页界面](/zh/guide/start/web-interface) — 登录后能做什么。
- [客户端接入](/zh/guide/start/clients) — 官方 Seafile 客户端与 WebDAV。
- [配置](/zh/guide/configure/configuration) — 配置来源、优先级与重启规则。
- [安全](/zh/guide/admin/security) — 反向代理、`site_url`、主密钥。
- [数据目录](/zh/guide/configure/data-directory) — 数据存放在哪里。
