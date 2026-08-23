use axum::extract::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, extract::State};
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use super::events::{JwtExpiredEvent, NotificationMessage, SubscribeRequest, UnsubscribeRequest};
use super::manager::validate_notification_jwt;
use crate::AppState;
use base::error::AppError;

/// GET /notification/ — WebSocket upgrade endpoint.
///
/// The client connects via WebSocket and then sends subscribe/unsubscribe
/// messages to register for repo notifications.
///
/// The upgrade handshake itself carries no credentials — matching the official
/// Seafile notification-server protocol, where auth happens per-repo via the
/// JWT in the `subscribe` message. Instead, connection-level DoS defenses are
/// applied here: global and per-IP connection caps are checked before the
/// upgrade is committed (a 503 is returned when a cap is reached), and an
/// unauthenticated connection is dropped after `subscribe_timeout_secs`.
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Response {
    // The effective client IP honors X-Forwarded-For only when the TCP peer is
    // a trusted proxy, so per-IP caps cannot be bypassed by spoofing the header.
    let peer_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );

    // Enforce connection caps before the upgrade is committed. `on_upgrade` is
    // never called on this path, so the `OnUpgrade` future is dropped and no
    // WebSocket is established — the client receives a plain 503.
    if let Some(mgr) = &state.notification_manager {
        let over_global =
            mgr.max_connections() > 0 && mgr.connection_count() as u64 >= mgr.max_connections();
        let over_ip = mgr.max_connections_per_ip() > 0
            && mgr.connection_count_by_ip(&peer_ip) as u64 >= mgr.max_connections_per_ip();
        if over_global || over_ip {
            tracing::warn!(
                "Rejecting WebSocket connection from {peer_ip}: connection limit reached"
            );
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_ws_socket(socket, peer_ip, state))
}

/// Handle an upgraded WebSocket connection.
async fn handle_ws_socket(socket: WebSocket, peer_ip: String, state: Arc<AppState>) {
    let notif_mgr = match &state.notification_manager {
        Some(mgr) => mgr.clone(),
        None => return,
    };

    let private_key = state.config.notification.private_key.clone();
    let ping_interval = state.config.notification.ping_interval;
    let client_timeout = state.config.notification.client_timeout;
    let subscribe_timeout_secs = state.config.notification.subscribe_timeout_secs;
    let keepalive_enabled = ping_interval > 0 && client_timeout > 0;
    let subscribe_timeout = std::time::Duration::from_secs(subscribe_timeout_secs);

    // Shared timestamp (nanos since UNIX epoch) of the last received Pong.
    let last_pong = Arc::new(AtomicI64::new(now_nanos()));

    // Split the socket into independent read/write halves. This is critical:
    // the read half blocks waiting for the next client frame, and must not
    // contend with (and starve) the write half that delivers notifications.
    let (mut sink, mut stream) = socket.split();

    // Messages are pre-serialized bytes; the bounded channel caps how much a
    // slow client can buffer before notifications start being dropped.
    let (tx, mut rx) = mpsc::channel::<Arc<[u8]>>(64);

    // Register the client. If the connection caps were hit in the race window
    // between the pre-upgrade check and this call, close the connection.
    let (client_id, client_state) = match notif_mgr.register_client(tx, peer_ip) {
        Some(pair) => pair,
        None => {
            tracing::debug!("WebSocket connection rejected during client registration");
            return;
        }
    };

    // Task: read messages from the WebSocket.
    let read_mgr = notif_mgr.clone();
    let read_id = client_id;
    let read_key = private_key.clone();
    let read_pong = last_pong.clone();
    let read_state = client_state.clone();

    let read_task = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + subscribe_timeout;
        loop {
            // An unauthenticated connection must present a valid `subscribe`
            // (proving repo access via JWT) before the deadline. The deadline
            // is absolute, so a client sending garbage frames every few seconds
            // cannot reset the timer and hold the connection open forever.
            let next = if subscribe_timeout_secs > 0 && !read_state.is_authenticated() {
                match tokio::time::timeout_at(deadline, stream.next()).await {
                    Ok(inner) => inner,
                    Err(_) => {
                        tracing::debug!(
                            "WebSocket closed: no valid subscribe within {}s",
                            subscribe_timeout_secs
                        );
                        break;
                    }
                }
            } else {
                stream.next().await
            };

            let msg = match next {
                Some(Ok(m)) => m,
                _ => break,
            };
            if handle_read_msg(&read_mgr, read_id, &read_key, &read_pong, msg).await {
                break;
            }
        }

        read_mgr.unregister_client(read_id).await;
    });

    // Task: forward events from the notification manager channel to the
    // WebSocket. When keepalive is enabled this task also drives the
    // server→client ping/pong watchdog.
    let write_mgr = notif_mgr.clone();
    let write_id = client_id;
    let write_pong = last_pong.clone();

    let write_task = tokio::spawn(async move {
        if keepalive_enabled {
            let interval = std::time::Duration::from_secs(ping_interval);
            let timeout_ns = std::time::Duration::from_secs(client_timeout).as_nanos() as i64;
            let mut ticker =
                tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);

            loop {
                tokio::select! {
                    bytes = rx.recv() => {
                        match bytes {
                            Some(bytes) => {
                                let text =
                                    String::from_utf8((*bytes).to_vec()).unwrap_or_default();
                                if sink.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = ticker.tick() => {
                        let last = write_pong.load(Ordering::Acquire);
                        let elapsed = now_nanos() - last;
                        if elapsed > timeout_ns {
                            tracing::debug!(
                                "WebSocket client timed out after {}s without pong",
                                elapsed / 1_000_000_000,
                            );
                            break;
                        }
                        if sink.send(Message::Ping(axum::body::Bytes::new())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        } else {
            while let Some(bytes) = rx.recv().await {
                let text = String::from_utf8((*bytes).to_vec()).unwrap_or_default();
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }

        write_mgr.unregister_client(write_id).await;
    });

    // Wait for either task to finish.
    tokio::select! {
        _ = read_task => {},
        _ = write_task => {},
    }

    // Final cleanup — unregister the client.
    notif_mgr.unregister_client(client_id).await;
}

/// Process a single WebSocket message received from the client.
///
/// Returns `true` if the connection should be closed (Close frame or error).
async fn handle_read_msg(
    mgr: &super::manager::NotificationManager,
    client_id: u64,
    private_key: &str,
    last_pong: &AtomicI64,
    msg: Message,
) -> bool {
    match msg {
        Message::Text(text) => {
            if let Ok(notif_msg) = serde_json::from_str::<NotificationMessage>(&text) {
                process_client_message(mgr, client_id, &notif_msg, private_key).await;
            }
            false
        }
        Message::Close(_) => true,
        Message::Ping(_) => {
            // axum handles pong responses automatically
            false
        }
        Message::Pong(_) => {
            last_pong.store(now_nanos(), Ordering::Release);
            false
        }
        Message::Binary(_) => false,
    }
}

/// Returns the current time in nanoseconds since the UNIX epoch.
fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// Process an incoming client message (subscribe or unsubscribe).
async fn process_client_message(
    mgr: &super::manager::NotificationManager,
    client_id: u64,
    msg: &NotificationMessage,
    private_key: &str,
) {
    match msg.msg_type.as_str() {
        "subscribe" => {
            let Ok(sub) = serde_json::from_value::<SubscribeRequest>(msg.content.clone()) else {
                return;
            };

            // Validate all JWT tokens first.
            let mut valid_subs: Vec<(String, i64)> = Vec::new();
            let mut username = String::new();

            for repo in &sub.repos {
                if let Some(claims) =
                    validate_notification_jwt(&repo.jwt_token, private_key, &repo.id)
                {
                    if username.is_empty() {
                        username = claims.username;
                    }
                    valid_subs.push((repo.id.clone(), claims.exp));
                } else {
                    // Invalid/expired token: tell the client to re-fetch a new
                    // token and resubscribe (matches seafile's behavior).
                    let event = JwtExpiredEvent {
                        repo_id: repo.id.clone(),
                    };
                    mgr.notify_client(client_id, &event.into()).await;
                }
            }

            if !valid_subs.is_empty() {
                mgr.subscribe(client_id, &username, &valid_subs).await;
            }
        }
        "unsubscribe" => {
            let Ok(unsub) = serde_json::from_value::<UnsubscribeRequest>(msg.content.clone())
            else {
                return;
            };

            let repo_ids: Vec<String> = unsub.repos.into_iter().map(|r| r.id).collect();
            mgr.unsubscribe(client_id, &repo_ids).await;
        }
        _ => {}
    }
}

/// POST /notification/events — post an event to all subscribers.
///
/// Authenticated via JWT Bearer token (Authorization: Bearer <token>).
/// The token must be signed with the configured private key.
pub async fn post_event(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Json<Value>,
) -> Result<Json<Value>, AppError> {
    let notif_mgr = match &state.notification_manager {
        Some(mgr) => mgr.clone(),
        None => {
            return Err(AppError::NotFound(
                "notification server not configured".into(),
            ));
        }
    };

    // Validate Authorization header JWT.
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header
        .strip_prefix("Bearer ")
        .or_else(|| auth_header.strip_prefix("Token "));

    let token = match token {
        Some(t) => t,
        None => {
            return Err(AppError::Unauthorized);
        }
    };

    // Validate the event submission JWT.
    let private_key = &state.config.notification.private_key;
    if !validate_event_jwt(token, private_key) {
        return Err(AppError::Unauthorized);
    }

    // Parse the event message.
    let notif_msg: NotificationMessage = serde_json::from_value(body.0.clone())
        .map_err(|_| AppError::BadRequest("invalid event format".into()))?;

    // Extract repo_id from content to find subscribers.
    if let Some(repo_id) = notif_msg.content.get("repo_id").and_then(|v| v.as_str()) {
        notif_mgr.notify_repo(repo_id, &notif_msg).await;
    }

    Ok(Json(serde_json::json!({"ret": "ok"})))
}

/// Validate a JWT token for the POST /events endpoint.
///
/// Matches seafile-server/notification-server: the event JWT carries only an
/// `exp` claim (no `sub`), so we validate signature + expiry and nothing else.
fn validate_event_jwt(token: &str, private_key: &str) -> bool {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    let key = DecodingKey::from_secret(private_key.as_bytes());
    jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation).is_ok()
}

/// GET /notification/ping — health check.
pub async fn ping() -> impl IntoResponse {
    Json(serde_json::json!({"ret": "pong"}))
}
