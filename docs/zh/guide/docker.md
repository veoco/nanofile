# Docker 部署

镜像是一个 `scratch` 容器，仅含 `nanofile` 一个二进制，没有配置文件、数据目录，也没有 `HOME`。它以 uid/gid `1000:1000` 运行，挂载的数据卷需对该用户可写。

## 让容器不进入首次初始化

下面两条示例都设置了 `NANOFILE_*` 变量，因此视为已配置。若容器**完全不带**环境变量启动，则会进入首次初始化：

- 初始化页面的地址会打印到容器 stdout，用 `docker logs <名称>` 即可取到。
- 该地址是 `127.0.0.1`，且页面本身不带 TLS。远程主机请做端口转发（最简单的是 SSH 隧道），不要把它直接暴露出去。
- 镜像里没有 `HOME`，初始化无处写 `~/.nanofile/config.toml`。这种场景请自己传 `--config` 并设置 `HOME`。

## 主密钥

主密钥生成一次并长期保存。它用于加密会话、邮件与静态块。更换后所有会话登出、同步客户端失效、已加密块无法解密。

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

## 升级与权限

升级后，已有数据目录可能属于别的 uid。重启前先让它对 `1000:1000` 可写：

```bash
chown -R 1000:1000 data
```

`--user` 可以覆盖镜像里的数字用户，但数据目录仍必须对服务实际运行的那个 uid 可写。

## 下一步

- [数据目录](/zh/guide/data-directory) — 各个挂载目录里有什么。
- [配置](/zh/guide/configuration) — 全部 `NANOFILE_*` 变量。
- [安全](/zh/guide/security) — 主密钥与反向代理设置。
