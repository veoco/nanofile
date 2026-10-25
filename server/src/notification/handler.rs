use axum::extract::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, extract::State};
use futures_util::{SinkExt, StreamExt};
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
/// The handshake carries no credentials, matching the official Seafile
/// notification-server protocol: auth happens per repo via the JWT in each
/// `subscribe` message. Instead, DoS defenses apply here: global and per-IP
/// connection caps (503, checked before the upgrade is committed) and a
/// `subscribe_timeout_secs` deadline for unauthenticated connections.
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
        &state.config().server.trusted_proxies,
    );

    // Enforce caps before the upgrade is committed: `on_upgrade` is never
    // called, so no WebSocket is established and the client gets a plain 503.
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

async fn handle_ws_socket(socket: WebSocket, peer_ip: String, state: Arc<AppState>) {
    let notif_mgr = match &state.notification_manager {
        Some(mgr) => mgr.clone(),
        None => return,
    };

    let private_key = state.config().notification.private_key.clone();
    let ping_interval = state.config().notification.ping_interval;
    let client_timeout = state.config().notification.client_timeout;
    let subscribe_timeout_secs = state.config().notification.subscribe_timeout_secs;
    let keepalive_enabled = ping_interval > 0 && client_timeout > 0;
    let subscribe_timeout = std::time::Duration::from_secs(subscribe_timeout_secs);

    // Shared timestamp (nanos since UNIX epoch) of the last received Pong.
    let last_pong = Arc::new(AtomicI64::new(now_nanos()));

    // Split the socket into independent read/write halves: the read half blocks
    // waiting for the next client frame and must not starve the write half.
    let (mut sink, mut stream) = socket.split();

    // Pre-serialized bytes; the bounded channel caps how much a slow client can
    // buffer before notifications start being dropped.
    let (tx, mut rx) = mpsc::channel::<Arc<[u8]>>(64);

    // Caps can be hit in the race between the pre-upgrade check and this call.
    let (client_id, client_state) = match notif_mgr.register_client(tx, peer_ip) {
        Some(pair) => pair,
        None => {
            tracing::debug!("WebSocket connection rejected during client registration");
            return;
        }
    };

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

    // Forward manager events to the socket; with keepalive enabled this task
    // also drives the server→client ping/pong watchdog.
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

    tokio::select! {
        _ = read_task => {},
        _ = write_task => {},
    }

    notif_mgr.unregister_client(client_id).await;
}

/// Process a single WebSocket message; returns `true` if the connection should
/// be closed (Close frame or error).
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

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

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
                    // Tell the client to re-fetch a token and resubscribe
                    // (matches Seafile's behavior).
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
/// Authenticated via JWT Bearer token (Authorization: Bearer `<token>`) signed
/// with the configured private key.
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

    let private_key = &state.config().notification.private_key;
    if !validate_event_jwt(
        token,
        private_key,
        state.config().notification.accept_legacy_event_tokens,
    ) {
        return Err(AppError::Unauthorized);
    }

    let notif_msg: NotificationMessage = serde_json::from_value(body.0.clone())
        .map_err(|_| AppError::BadRequest("invalid event format".into()))?;

    if let Some(repo_id) = notif_msg.content.get("repo_id").and_then(|v| v.as_str()) {
        notif_mgr.notify_repo(repo_id, &notif_msg).await;
    }

    Ok(Json(serde_json::json!({"ret": "ok"})))
}

/// Validate a JWT token for the POST /events endpoint.
///
/// Events are verified against the **event** subkey, distinct from the
/// subscription key: a subscription token from
/// `/seafhttp/repo/{id}/jwt-token` must not be usable to post events.
/// `accept_legacy` additionally allows the undivided root key for external
/// publishers on the old scheme; it is off by default.
fn validate_event_jwt(token: &str, private_key: &str, accept_legacy: bool) -> bool {
    let event_key = crate::notification::keys::event_key(private_key);
    if crate::service::auth::hs256::decode::<serde_json::Value>(token, &event_key).is_some() {
        return true;
    }

    if accept_legacy {
        return crate::service::auth::hs256::decode::<serde_json::Value>(
            token,
            private_key.as_bytes(),
        )
        .is_some();
    }

    false
}

/// GET /notification/ping — health check.
pub async fn ping() -> impl IntoResponse {
    Json(serde_json::json!({"ret": "pong"}))
}

#[cfg(test)]
mod tests {
    use super::validate_event_jwt;
    use crate::notification::keys::{event_key, subscription_key};
    use crate::service::auth::hs256;

    const PRIVATE_KEY: &str = "notification-private-key-for-tests";

    fn token(key: &[u8], exp: i64) -> String {
        hs256::encode(&serde_json::json!({ "exp": exp }), key).expect("encode token")
    }

    #[test]
    fn event_token_validates_only_against_the_event_subkey() {
        let event = event_key(PRIVATE_KEY);
        assert!(validate_event_jwt(
            &token(&event, 4_102_444_800),
            PRIVATE_KEY,
            false
        ));

        let subscription = subscription_key(PRIVATE_KEY);
        assert!(
            !validate_event_jwt(&token(&subscription, 4_102_444_800), PRIVATE_KEY, false),
            "a subscription token must not authorise event posting"
        );
    }

    #[test]
    fn legacy_root_key_is_opt_in_and_exp_is_required() {
        let legacy = token(PRIVATE_KEY.as_bytes(), 4_102_444_800);
        assert!(!validate_event_jwt(&legacy, PRIVATE_KEY, false));
        assert!(validate_event_jwt(&legacy, PRIVATE_KEY, true));

        let event = event_key(PRIVATE_KEY);
        assert!(
            !validate_event_jwt(&token(&event, 1_000), PRIVATE_KEY, false),
            "an expired event token must be rejected"
        );
        let no_exp = hs256::encode(&serde_json::json!({ "repo_id": "r" }), &event).unwrap();
        assert!(!validate_event_jwt(&no_exp, PRIVATE_KEY, false));
    }
}
