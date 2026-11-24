---
description: 运行已发布镜像、挂载数据目录并使用 compose。
---
# Docker 部署

**适合谁：** 运行已发布镜像的运维人员。**前置条件：** 有 Docker，以及 uid/gid 1000 可写的数据卷。

镜像是一个 `scratch` 容器，仅含 `nanofile` 一个二进制，没有配置文件、数据目录，也没有 `HOME`。它以 uid/gid `1000:1000` 运行，挂载的数据卷需对该用户可写。

## 让容器不进入首次初始化

下面两条示例都设置了 `NANOFILE_*` 变量，因此视为已配置。若容器**完全不带**环境变量启动，则会进入首次初始化：

- 初始化页面的地址会打印到容器 stdout，用 `docker logs <名称>` 即可取到。
- 初始化页监听配置里的监听地址（默认 `0.0.0.0:8082`，即所有网卡），而打印出来的 URL 是 `127.0.0.1`，页面本身不带 TLS。远程主机请用防火墙限制端口，或用 SSH 隧道访问，不要把它直接暴露出去。
- 镜像里没有 `HOME`，工作目录又是 `/`，uid 1000 无法写入，初始化因此无处写 `~/.nanofile/config.toml`。给它一个可写的 HOME 与工作目录，例如 `-e HOME=/data -w /data`，配置就会写到 `/data/.nanofile/config.toml`。
- 不想走初始化时，挂载一个**已经存在**的配置文件并用 `--config` 传入它的路径（显式指定的配置文件必须存在，否则拒绝启动），再用下面的 `adduser` 建管理员。

变量跳过设置页时，在容器里创建第一个管理员：

```bash
docker exec -it nanofile /nanofile adduser --email admin@example.com
```

交互式密码提示需要终端；不带 `-it` 时进程会在读密码之前退出。脚本里改用标准输入喂密码：

```bash
printf '%s\n' 'secret123' | docker exec -i nanofile /nanofile adduser --email admin@example.com --password-stdin
```

或者挂载带 `[admin_init]` 的配置文件。生产环境把 `latest` 换成版本 tag；`latest` 会随每次发布移动。

## 主密钥

主密钥生成一次并长期保存。它派生 CSRF 签名密钥、同步令牌/TOTP/邮件队列的加密密钥、通知 JWT 密钥（未显式配置时）以及静态块密钥（未设置 `storage.encryption_key` 时）。会话不由它派生（会话 token 是随机值、库里只存 SHA-256 摘要），所以轮换不会让浏览器登出；但同步客户端会失效，上述数据在换回旧密钥前无法读出。

```bash
mkdir -p data
openssl rand -hex 32 > nanofile-secret
chmod 600 nanofile-secret
```

## 带配置文件运行

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -v "$PWD/config.toml:/etc/nanofile/config.toml:ro" \
  -v "$PWD/nanofile-secret:/run/secrets/nanofile-secret:ro" \
  -e NANOFILE_CONFIG=/etc/nanofile/config.toml \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_INDEX_INDEX_DIR=/data/index \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

配置文件可选，其余由内置默认值填充。第二条示例只设置那些必须放在镜像之外的值：

```bash
docker run -d --name nanofile \
  -p 8082:8082 \
  -v "$PWD/data:/data" \
  -e NANOFILE_DATABASE_URL='sqlite:/data/nanofile.db?mode=rwc' \
  -e NANOFILE_STORAGE_BLOCK_DIR=/data/blocks \
  -e NANOFILE_STORAGE_TEMP_DIR=/data/temp \
  -e NANOFILE_SERVER_SECRET_KEY="$(cat nanofile-secret)" \
  ghcr.io/veoco/nanofile:latest
```

`nanofile-secret` 为上方生成、跨启动复用的持久值。

## Compose 文件

```yaml
# docker-compose.yml
services:
  nanofile:
    image: ghcr.io/veoco/nanofile:latest
    restart: unless-stopped
    ports:
      - "8082:8082"
    volumes:
      - ./data:/data
      - ./config.toml:/etc/nanofile/config.toml:ro
      - ./nanofile-secret:/run/secrets/nanofile-secret:ro
    environment:
      NANOFILE_CONFIG: /etc/nanofile/config.toml
      NANOFILE_DATABASE_URL: sqlite:/data/nanofile.db?mode=rwc
      NANOFILE_STORAGE_BLOCK_DIR: /data/blocks
      NANOFILE_STORAGE_TEMP_DIR: /data/temp
      NANOFILE_SERVER_SECRET_KEY: ${NANOFILE_SECRET:?先在 .env 里设置 NANOFILE_SECRET}
```

`${NANOFILE_SECRET:?…}` 会在变量缺失时拒绝启动，而不是带着空主密钥起来。

## 验证

- `docker logs nanofile` 显示监听已启动，且没有启动错误。
- `curl -fsS http://127.0.0.1:8082/health` 返回 200。
- 登录、上传一个文件，用同一个数据卷重启容器，再下载它。

## 升级与权限

升级后，已有数据目录可能属于别的 uid。重启前先让它对 `1000:1000` 可写：

```bash
chown -R 1000:1000 data
```

`--user` 可以覆盖镜像里的数字用户，但数据目录仍必须对服务实际运行的那个 uid 可写。

## 下一步

- [数据目录](/zh/guide/configure/data-directory) — 各个挂载目录里有什么。
- [配置](/zh/guide/configure/configuration) — 全部 `NANOFILE_*` 变量。
- [安全](/zh/guide/admin/security) — 主密钥与反向代理设置。
- [备份、恢复与升级](/zh/guide/operate/backup-and-upgrade) — 镜像标签（`latest`、`vX.Y.Z`、`edge`）分别适合谁。
