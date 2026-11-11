---
description: 用 nginx 或 Caddy 给 Nanofile 加上 HTTPS，并让客户端 IP 正确传递。
---
# HTTPS 与反向代理

Nanofile 本身说 HTTP。反向代理负责终止 TLS、转发请求，并补上服务器识别真实客户端地址所需的转发头。本页讲代理配置；凭据与服务端规则见[安全](/zh/guide/admin/security)。

**适合谁：** 要把 Nanofile 暴露到可信局域网之外的人。**前置条件：** 一个 DNS 名称、一张 TLS 证书（或让 Caddy/Cloudflare 申请），以及监听在回环地址上的服务器。

## 前置条件

- 把 DNS 名称指向代理。
- 代理与 Nanofile 在同一台主机时，让 Nanofile 绑定 `127.0.0.1`，这样明文 HTTP 端口不能从外部直接访问。在 Docker 里，把容器放到私有网络，而不是把 8082 发布到公网。
- 决定对外地址。它必须是客户端实际使用的地址，不是服务器的内部地址。

## 设置 site_url

把 `server.site_url` 设为 HTTPS 地址：

```toml
[server]
site_url = "https://files.example.com"
```

`site_url` 决定会话 cookie 的 `Secure` 属性、是否启用 HSTS，以及生成分享链接时使用的地址。写错会表现为客户端能登录但同步不动，因为客户端会用这个地址去取块和上传。

首次初始化时不要暴露一次性设置页；先配好 `site_url` 与反向代理，或用 SSH 隧道访问设置页。见[安装与首次运行](/zh/guide/start/getting-started)。

## nginx

下面的配置假设 Nanofile 监听 `127.0.0.1:8082`、对外域名是 `files.example.com`：

```nginx
server {
    listen 443 ssl http2;
    server_name files.example.com;

    ssl_certificate     /etc/letsencrypt/live/files.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/files.example.com/privkey.pem;

    # 上传大小交给 Nanofile 自己限制；nginx 默认 1m 会挡住大文件。
    client_max_body_size 0;

    location / {
        proxy_pass http://127.0.0.1:8082;
        proxy_http_version 1.1;

        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 站内通知走 WebSocket，握手头必须透传。
        proxy_set_header Upgrade           $http_upgrade;
        proxy_set_header Connection        $connection_upgrade;

        # 大文件下载与长连接同步不能被中途掐断。
        proxy_read_timeout 3600s;
        proxy_buffering off;
    }
}
```

`$connection_upgrade` 需要在 `http` 块里先定义一次：

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
```

然后在 `server.trusted_proxies` 里写上代理地址：

```toml
[server]
site_url = "https://files.example.com"
trusted_proxies = ["loopback"]
```

## Caddy

Caddy 自己申请并续期证书：

```txt
files.example.com {
	reverse_proxy 127.0.0.1:8082 {
		# 让代理比服务器更早退掉空闲的上游连接。
		transport http {
			keepalive 10s
		}
	}
}
```

Caddy 会转发 `X-Forwarded-For` 与 `X-Forwarded-Proto`；照上面的方式设置 `server.site_url` 与 `server.trusted_proxies = ["loopback"]`。

`keepalive` 这一行不是可有可无的。Caddy 会复用上游连接，而服务器在等待下一个请求头超过 `server.header_read_timeout_secs`（默认 120 秒）时会退掉空闲的那条。请求若正好写进一条刚被服务器关掉的连接，客户端收到的就是 502——只在空闲一段时间后的第一个请求上出现，其余一切看起来都正常。10 秒短于任何轮询间隔；要提高吞吐可以调大，但别超过服务器的截止时间。上面的 nginx 配方发往上游的是 `Connection: close`，不存在空闲复用，因此不需要这一项。

## Cloudflare Tunnel

经 Cloudflare Tunnel（`cloudflared`）暴露时，源站看到的对端是回环或私有地址。设置：

```toml
[server]
site_url = "https://files.example.com"
trusted_proxies = ["private"]
```

把源站绑定到 `127.0.0.1`，或在 Docker 网络中隔离。Cloudflare 会通过 `CF-Connecting-IP` 提供真实访客 IP，其优先级高于 `X-Forwarded-For`。

只有在源站无法被其他来源访问时才可信任 `private`，否则同网段设备可伪造该头。

## 受信代理

`trusted_proxies` 决定哪些对端地址提供的 `CF-Connecting-IP` 或 `X-Forwarded-For` 会被采信。未设置时这些头一律被忽略，所有客户端都会塌缩成代理地址，按 IP 的限流退化为同一个桶。

每项可填具体 IP、CIDR 网段，或关键字：

- `loopback` —— `127.0.0.1` 与 `::1`；
- `private` —— RFC 1918 / 私有网段；
- `link-local`、`unique-local` —— 对应的 IPv6 网段。

列表越窄越好。范围过宽等于信任所有能从这些地址访问服务器的机器。

## 验证

- `curl -I https://files.example.com/` 返回预期的跳转或页面；启用 HSTS 时响应带 `strict-transport-security`。
- 通过 HTTPS 登录；会话 cookie 带 `Secure` 标志。
- 创建一个分享链接；它以 `https://files.example.com` 开头。
- 打开网页界面，等待站内通知连接建立；它不应循环重连。
- 上传一个大于 1 MiB 的文件；代理不应返回 413。
- 看服务器日志里的客户端地址；`trusted_proxies` 正确时它是访客地址，不是代理地址。

## 排障

- **502/504** —— 代理连不上 `127.0.0.1:8082`；检查 `server.addr`、`server.port` 和服务器是否在运行。
- **偶发 502、过一会儿自己好** —— 请求落在了服务器刚退掉的空闲上游连接上；见上面 Caddy 的 `keepalive` 说明，或把 `server.header_read_timeout_secs` 调到大于客户端的轮询间隔。
- **重定向循环** —— 代理发的是 `X-Forwarded-Proto: http`，或 `site_url` 写的是 `https` 而代理转发的是明文 HTTP；修正头或地址。
- **WebSocket 断开** —— 缺少 `Upgrade`/`Connection` 头，或代理把它们剥掉了。
- **上传到固定大小就失败** —— 代理的请求体上限小于文件；nginx 里设 `client_max_body_size 0`，其他代理找等价设置。
- **所有人都是同一个 IP** —— `trusted_proxies` 为空，或没有包含代理地址；见上。
- **客户端拒绝证书** —— 用受信任的证书；官方客户端不接受自签名证书。

## 相关

- [安全](/zh/guide/admin/security) —— 主密钥、绑定地址、PoW 与凭据规则。
- [配置](/zh/guide/configure/configuration) —— `site_url` 与 `trusted_proxies` 在设置里的位置。
- [后台运行与自启动](/zh/guide/deploy/running-in-background) —— 让服务器与代理长期运行。
- [故障排查](/zh/guide/operate/troubleshooting) —— 客户端连不上时。
