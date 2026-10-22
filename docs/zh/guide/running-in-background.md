# 后台运行与自启动

托盘版在桌面上手动启动就够了；放到服务器或无人值守的机器上，就要让它跟系统一起起来、崩了能自己回来。Windows 的服务与托盘见[桌面托盘与 Windows 服务](/zh/guide/desktop-and-service)，本页覆盖 Linux、macOS 与容器。

## 共用前提

不管用哪种方式，下面几条都成立：

- **相对路径以二进制所在目录为基准**（不是工作目录），因此把二进制、配置与数据目录放在一起最省心；想分开就用绝对路径，见[数据目录](/zh/guide/data-directory)。
- **显式指定的配置文件不存在会拒绝启动**（`--config` 或 `NANOFILE_CONFIG`），不会悄悄用内置默认值——后台服务因此不会"起来了但连错数据库"。
- **release 构建没有主密钥会拒绝启动**，所以要让服务进程能读到 `server.secret_key`（配置文件，或 `NANOFILE_SERVER_SECRET_KEY`）。
- **不要用 root 跑。** 建一个专用用户，让它对配置与数据目录可读写即可。
- **日志默认去 stdout**（无头运行），因此交给 journald 或 launchd 收集就行；想写文件用 `[logging]`。

## Linux：systemd

```ini
# /etc/systemd/system/nanofile.service
[Unit]
Description=Nanofile
After=network.target

[Service]
Type=simple
User=nanofile
Group=nanofile
WorkingDirectory=/opt/nanofile
ExecStart=/opt/nanofile/nanofile --config /opt/nanofile/config.toml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now nanofile
sudo systemctl status nanofile
journalctl -u nanofile -f        # 看日志
```

- 二进制放 `/opt/nanofile/`，配置与数据目录对它可读可写：`sudo chown -R nanofile:nanofile /opt/nanofile`。
- 崩溃与异常退出由 `Restart=always` 兜住；主动 `systemctl stop` 不会重启。
- 升级就是换掉二进制再 `sudo systemctl restart nanofile`，步骤见[备份、恢复与升级](/zh/guide/backup-and-upgrade)。

**加固要一步一步来。** systemd 的隔离选项（`ProtectSystem=strict`、`PrivateTmp`、`ProtectHome` 等）会改变进程能看到的文件系统，而 Nanofile 的解析子进程本身就在建立自己的限制。先用上面的最小单元跑通，再逐项加固并重启验证，不要把一整套硬化选项一次加上。

## macOS：launchd

桌面使用直接跑 `tray` 变体即可；要让它在后台随系统启动，用一个 LaunchDaemon（下面是模板，**未纳入本项目的 CI 验证**，路径与用户按实际安装替换）：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!-- /Library/LaunchDaemons/io.github.veoco.nanofile.plist -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>io.github.veoco.nanofile</string>
    <key>ProgramArguments</key>
    <array>
        <string>/opt/nanofile/nanofile</string>
        <string>--config</string>
        <string>/opt/nanofile/config.toml</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/opt/nanofile</string>
    <key>UserName</key>
    <string>nanofile</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/nanofile.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/nanofile.log</string>
</dict>
</plist>
```

```bash
sudo launchctl load -w /Library/LaunchDaemons/io.github.veoco.nanofile.plist
```

只想在登录后随用户启动，就放到 `~/Library/LaunchAgents/` 并用 `launchctl load -w` 加载（去掉 `UserName`）。

## 容器：重启策略

Docker 自己就能看住进程：

```bash
docker run -d --name nanofile --restart=unless-stopped \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

`unless-stopped` 会在崩溃与 Docker 重启后拉起容器，但不会推翻你手动 `docker stop`。compose 里是 `restart: unless-stopped`。详见[Docker 部署](/zh/guide/docker)。

镜像里没有 `HOME`、也没有 shell，因此**后台运行所需的一切都要通过 `NANOFILE_*` 变量或挂载的配置文件给出**；首次初始化在容器里也无处写 `~/.nanofile/config.toml`。

## 相关

- [桌面托盘与 Windows 服务](/zh/guide/desktop-and-service) — Windows 的托盘与 SCM 服务。
- [备份、恢复与升级](/zh/guide/backup-and-upgrade) — 停服务、换二进制、换镜像。
- [数据目录](/zh/guide/data-directory) — 这些东西放在哪。
- [配置](/zh/guide/configuration) — 配置文件查找顺序与 `NANOFILE_*`。
