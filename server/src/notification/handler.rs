use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{Json, extract::State, response::IntoResponse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use super::events::{NotificationMessage, SubscribeRequest, UnsubscribeRequest};
use super::manager::validate_notification_jwt;
use crate::AppState;
use base::error::AppError;

/// GET /notification/ — WebSocket upgrade endpoint.
///
/// The client connects via WebSocket and then sends subscribe/unsubscribe
/// messages to register for repo notifications.
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_socket(socket, state))
}

/// Handle an upgraded WebSocket connection.
async fn handle_ws_socket(socket: WebSocket, state: Arc<AppState>) {
    let notif_mgr = match &state.notification_manager {
        Some(mgr) => mgr.clone(),
        None => return,
    };

    let private_key = state.config.notification.private_key.clone();
    let ping_interval = state.config.notification.ping_interval;
    let client_timeout = state.config.notification.client_timeout;
    let keepalive_enabled = ping_interval > 0 && client_timeout > 0;

    // Shared timestamp (nanos since UNIX epoch) of the last received Pong.
    let last_pong = Arc::new(AtomicI64::new(now_nanos()));

    // We need to split the websocket into read/write halves.
    // Since axum's WebSocket doesn't implement futures::Stream/Sink directly,
    // we use two tasks connected by an mpsc channel for outgoing messages.
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let (client_id, _client_state) = notif_mgr.register_client(tx);

    // Use a mutex to serialize writes to the WebSocket
    // (not safe to call send from multiple tasks concurrently).
    let ws = Arc::new(tokio::sync::Mutex::new(socket));
    let ws_write = ws.clone();

    // Task: read messages from the WebSocket.
    // When keepalive is enabled, recv() is called with a timeout equal to
    // ping_interval — on each tick we check the pong deadline and, if the
    // client is still alive, send a Ping frame.  This avoids a separate
    // keepalive task contending for the WebSocket mutex.
    let read_mgr = notif_mgr.clone();
    let read_id = client_id;
    let read_key = private_key.clone();
    let read_pong = last_pong.clone();

    let read_task = tokio::spawn(async move {
        if keepalive_enabled {
            let interval = std::time::Duration::from_secs(ping_interval);
            let timeout = std::time::Duration::from_secs(client_timeout);
            let timeout_ns = timeout.as_nanos() as i64;

            loop {
                let mut ws_lock = ws.lock().await;
                let timed_out = tokio::time::timeout(interval, ws_lock.recv()).await;
                drop(ws_lock);

                match timed_out {
                    Ok(Some(Ok(msg))) => {
                        if handle_read_msg(&read_mgr, read_id, &read_key, &read_pong, msg).await {
                            break;
                        }
                    }
                    Ok(Some(Err(_))) => break,
                    Ok(None) => break,
                    Err(_elapsed) => {
                        // No message received within ping_interval: keepalive tick.
                        let last = read_pong.load(Ordering::Acquire);
                        let elapsed = now_nanos() - last;
                        if elapsed > timeout_ns {
                            tracing::debug!(
                                "WebSocket client timed out after {}s without pong",
                                elapsed / 1_000_000_000,
                            );
                            break;
                        }

                        let mut ws_lock = ws.lock().await;
                        if ws_lock
                            .send(Message::Ping(axum::body::Bytes::new()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        } else {
            // Keepalive disabled — simple blocking read loop.
            loop {
                let mut ws_lock = ws.lock().await;
                let msg = ws_lock.recv().await;
                drop(ws_lock);

                match msg {
                    Some(Ok(msg)) => {
                        if handle_read_msg(&read_mgr, read_id, &read_key, &read_pong, msg).await {
                            break;
                        }
                    }
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }

        read_mgr.unregister_client(read_id).await;
    });

    // Task: forward events from the notification manager channel to the WebSocket.
    let write_mgr = notif_mgr.clone();
    let write_id = client_id;

    let write_task = tokio::spawn(async move {
        while let Some(value) = rx.recv().await {
            let text = serde_json::to_string(&value).unwrap_or_default();
            let mut ws_lock = ws_write.lock().await;
            if ws_lock.send(Message::Text(text.into())).await.is_err() {
                break;
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
fn validate_event_jwt(token: &str, private_key: &str) -> bool {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.sub = Some("nanofile-events".to_string());

    let key = DecodingKey::from_secret(private_key.as_bytes());
    jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation).is_ok()
}

/// GET /notification/ping — health check.
pub async fn ping() -> impl IntoResponse {
    Json(serde_json::json!({"ret": "pong"}))
}
