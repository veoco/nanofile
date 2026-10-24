# 通知 WebSocket

官方客户端打开的 WebSocket，用来得知资料库发生变化，而不用轮询。它遵循 Seafile 的 notification-server 协议；端点是 `/notification`，认证发生在**每个订阅**上，而不是握手上。

源码：`server/src/notification/`；测试：`server/tests/notification_test.rs`。

## 连接 {#get-notification}

`GET /notification`（WebSocket 升级）

握手不带凭据，与 Seafile 一致：在客户端订阅之前，服务器不知道它关心哪些资料库。升级前先做 DoS 控制：`notification.max_connections`（默认 1024）与 `notification.max_connections_per_ip`（默认 64）限制连接数，超限请求返回 `503`。

连接在 `notification.subscribe_timeout_secs`（默认 60）内没有订阅会被关闭。

## 消息

每帧都是带 `type` 与 `content` 的 JSON 对象：

```json
{ "type": "subscribe", "content": { "repos": [{ "id": "…", "jwt_token": "…" }] } }
```

### 客户端 → 服务端

| `type` | `content` |
| --- | --- |
| `subscribe` | `{"repos": [{"id": "<repo_id>", "jwt_token": "<token>"}]}` |
| `unsubscribe` | `{"repos": [{"id": "<repo_id>"}]}` |

JWT 来自 [`GET /seafhttp/repo/{repo_id}/jwt-token`](/zh/reference/api/sync#get-seafhttp-repo-repo-id-jwt-token)，HS256、有效期 3 天，声明是 `repo_id`、`username`、`exp`。无效、过期或库不符的令牌不会关闭连接：服务器为该资料库发送 `jwt-expired`，客户端可换新令牌重新订阅。

### 服务端 → 客户端

| `type` | `content` |
| --- | --- |
| `repo-update` | `{"repo_id": "…", "commit_id": "…"}` —— HEAD 变了。 |
| `file-lock-changed` | `{"repo_id": "…", "path": "/a.txt", "change_event": "locked"\|"unlocked", "lock_user": "…"}` |
| `comment-update` | `{"repo_id": "…", "type": "…", "file_uuid": "…", "file_path": "…"}` —— 预留；Nanofile 没有评论。 |
| `jwt-expired` | `{"repo_id": "…"}` —— 换令牌后重新订阅。 |

### 保活

当 `notification.ping_interval`（默认 30）与 `notification.client_timeout`（默认 90）都非零时，服务器发送 WebSocket ping，并在 `client_timeout` 秒内没收到 pong 时关闭连接。客户端也可以自己发 ping，axum 会应答。

## 发布事件 {#post-notification-events}

`POST /notification/events`

为外部发布者（转换或 Office 服务）注入事件。请求体与客户端收到的是同一外层结构：

```json
{ "type": "repo-update", "content": { "repo_id": "…", "commit_id": "…" } }
```

需要 `Authorization: Bearer <jwt>`。令牌必须用通知**事件**子密钥做 HS256 签名；`/seafhttp/repo/{id}/jwt-token` 的订阅 JWT 会被拒绝。开启 `notification.accept_legacy_event_tokens` 后，用原始 `notification.private_key` 签名的令牌也接受（供旧式发布者）。

**响应 `200`** —— `{"ret": "ok"}`。未授权返回 `401`；信封格式错误返回 `400`；未配置通知服务返回 `404`。

## Ping {#get-notification-ping}

`GET /notification/ping`

**响应 `200`** —— `{"ret": "pong"}`。公开健康探针。
