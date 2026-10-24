# Notification WebSocket

The WebSocket the official clients open to learn that a library changed, instead
of polling. It follows Seafile's notification-server protocol; the endpoint is
`/notification` and authentication happens **per subscription**, not at the
handshake.

Source: `server/src/notification/`; tests:
`server/tests/notification_test.rs`.

## Connect {#get-notification}

`GET /notification` (WebSocket upgrade)

The handshake carries no credential, matching Seafile: the server cannot know
which libraries the client cares about until it subscribes. DoS controls apply
before the upgrade: `notification.max_connections` (default 1024) and
`notification.max_connections_per_ip` (default 64) count connections, and an
over-limit request gets `503`.

A connection that has not subscribed within
`notification.subscribe_timeout_secs` (default 60) is closed.

## Messages

Every frame is a JSON object with a `type` and a `content`:

```json
{ "type": "subscribe", "content": { "repos": [{ "id": "…", "jwt_token": "…" }] } }
```

### Client → server

| `type` | `content` |
| --- | --- |
| `subscribe` | `{"repos": [{"id": "<repo_id>", "jwt_token": "<token>"}]}` |
| `unsubscribe` | `{"repos": [{"id": "<repo_id>"}]}` |

The JWT comes from
[`GET /seafhttp/repo/{repo_id}/jwt-token`](/reference/api/sync#get-seafhttp-repo-repo-id-jwt-token).
It is HS256, lives three days, and its claims are `repo_id`, `username` and
`exp`. An invalid, expired or wrong-library token does not close the connection:
the server sends `jwt-expired` for that library so the client can fetch a new
token and resubscribe.

### Server → client

| `type` | `content` |
| --- | --- |
| `repo-update` | `{"repo_id": "…", "commit_id": "…"}` — HEAD moved. |
| `file-lock-changed` | `{"repo_id": "…", "path": "/a.txt", "change_event": "locked"\|"unlocked", "lock_user": "…"}` |
| `comment-update` | `{"repo_id": "…", "type": "…", "file_uuid": "…", "file_path": "…"}` — reserved; Nanofile has no comments. |
| `jwt-expired` | `{"repo_id": "…"}` — re-fetch the token and resubscribe. |

### Keepalive

When `notification.ping_interval` (default 30) and `notification.client_timeout`
(default 90) are both non-zero, the server sends WebSocket pings and closes the
connection after `client_timeout` seconds without a pong. The client may also
send its own pings; axum answers them.

## Publish an event {#post-notification-events}

`POST /notification/events`

Injects an event for an external publisher (a conversion or office service). The
body is the same envelope the clients receive:

```json
{ "type": "repo-update", "content": { "repo_id": "…", "commit_id": "…" } }
```

`Authorization: Bearer <jwt>` is required. The token must be HS256-signed with
the notification **event** subkey; a subscription JWT from
`/seafhttp/repo/{id}/jwt-token` is rejected. With
`notification.accept_legacy_event_tokens` enabled, a token signed with the raw
`notification.private_key` is also accepted (for pre-subkey publishers).

**Response `200`** — `{"ret": "ok"}`. Unauthorized requests are `401`; a
malformed envelope is `400`; no notification service configured is `404`.

## Ping {#get-notification-ping}

`GET /notification/ping`

**Response `200`** — `{"ret": "pong"}`. Unauthenticated health probe.
