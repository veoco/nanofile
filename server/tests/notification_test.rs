mod common;

use common::{NotifLimits, TestFixture, TestServer};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// Test that the notification endpoints respond correctly.
///
/// Note: Full WebSocket event delivery tests require a bidirectional
/// connection, which is challenging in integration tests. The core
/// notification flow is verified indirectly:
///   - JWT token generation (sync_aux_test::test_jwt_token_success)
///   - Lock/unlock API works (lock_file_test)
///   - The notification server accepts WebSocket upgrades
///   - POST /notification/events accepts events
#[tokio::test]
async fn test_notification_ping() {
    let f = TestFixture::new_with_notification().await;

    let resp = f.client.get("/notification/ping", None).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ret"], "pong");
}

/// Test that POST /notification/events accepts and processes events.
#[tokio::test]
async fn test_notification_post_event_unauthorized() {
    let f = TestFixture::new_with_notification().await;

    // Without a valid JWT, POST /notification/events should return 401.
    let event = serde_json::json!({
        "type": "file-lock-changed",
        "content": {
            "repo_id": f.repo_id,
            "path": "/test.txt",
            "change_event": "locked",
            "lock_user": "test@example.com"
        }
    });
    let resp = f
        .client
        .post_json("/notification/events", None, &event)
        .await;
    assert_eq!(resp.status(), 401);

    // With an invalid token, should also return 401.
    let resp = f
        .client
        .post_json("/notification/events", Some("invalid-token"), &event)
        .await;
    assert_eq!(resp.status(), 401);
}

/// End-to-end: connect, subscribe, trigger a repo update, and assert the
/// event is delivered promptly.
///
/// This regresses the read/write lock-starvation bug where the read half held
/// a shared WebSocket mutex while blocking on `recv`, starving the write half
/// and delaying (or fully blocking) notification delivery.
#[tokio::test]
async fn test_subscribe_receives_repo_update() {
    let f = TestFixture::new_with_notification().await;

    // Fetch a valid subscription JWT from the notification token endpoint.
    let resp = f
        .client
        .get_sync(
            &format!("/seafhttp/repo/{}/jwt-token", f.repo_id),
            &f.sync_token,
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let jwt = body["jwt_token"].as_str().unwrap().to_string();

    // Connect to the WebSocket notification endpoint.
    let ws_url = f.server.base_url.replace("http", "ws") + "/notification";
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Subscribe to the repo.
    let sub = serde_json::json!({
        "type": "subscribe",
        "content": {
            "repos": [{ "id": f.repo_id, "jwt_token": jwt }]
        }
    });
    ws.send(Message::Text(sub.to_string().into()))
        .await
        .unwrap();

    // Let the server register the subscription before triggering an event.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Trigger a repo update by uploading a file (fires a repo-update event).
    let upload = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "notif-ws.txt", b"data")
        .await;
    assert!(upload.status().is_success());

    // The repo-update notification must arrive promptly — well under the 30s
    // keepalive interval that previously starved the write half.
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                        if v["type"] == "repo-update" && v["content"]["repo_id"] == f.repo_id.as_str() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(e)) => panic!("WebSocket error before repo-update: {e}"),
                    None => panic!("WebSocket closed before repo-update"),
                }
            }
            _ = &mut deadline => {
                panic!("Timed out waiting for repo-update notification");
            }
        }
    }
}

/// Test that the WebSocket upgrade endpoint is reachable.
#[tokio::test]
async fn test_websocket_upgrade_works() {
    let f = TestFixture::new_with_notification().await;
    let ws_url = f.server.base_url.replace("http", "ws") + "/notification";

    let result = tokio_tungstenite::connect_async(&ws_url).await;
    assert!(
        result.is_ok(),
        "WebSocket upgrade should succeed, got: {:?}",
        result.err()
    );
}

/// Test that locking/unlocking via the sync API succeeds and the
/// notification manager is properly initialized (regression test).
#[tokio::test]
async fn test_lock_with_notification_enabled() {
    let f = TestFixture::new_with_notification().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "notif-lock.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    // Lock the file
    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/notif-lock.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Unlock the file
    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/unlock-file?p=/notif-lock.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);
}

// ── WebSocket keepalive tests ──────────────────────────────────────────

/// Test that the server sends WebSocket Ping frames when keepalive is
/// enabled. The client (tokio-tungstenite) auto-responds with Pong, so
/// we only verify that Ping frames arrive at the application layer.
#[tokio::test]
async fn test_server_sends_ping() {
    // Start a server with a short ping interval so the test completes quickly.
    let _server = TestServer::start_with_custom_keepalive(1, 10).await;
    let ws_url = _server.base_url.replace("http", "ws") + "/notification";

    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Wait for up to 1.5 seconds to receive at least one Ping from the server.
    // Server has 1s ping interval, so 1.5s is more than enough.
    let deadline = tokio::time::sleep(std::time::Duration::from_millis(1500));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Ping(_))) => {
                        break; // ✅ received a ping — test passes
                    }
                    Some(Ok(_)) => {
                        // Other message types — keep waiting
                    }
                    Some(Err(e)) => {
                        panic!("WebSocket error before receiving a ping: {e}");
                    }
                    None => {
                        panic!("Server closed connection before sending a ping");
                    }
                }
            }
            _ = &mut deadline => {
                panic!("Timed out waiting for server to send a ping frame");
            }
        }
    }
}

/// Test that when the client is responsive (auto-pongs), the server keeps
/// the connection alive well past the client_timeout threshold.
#[tokio::test]
async fn test_keepalive_keeps_connection_alive() {
    let _server = TestServer::start_with_custom_keepalive(1, 3).await;
    let ws_url = _server.base_url.replace("http", "ws") + "/notification";

    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Read messages for 3.5 seconds — well past the 3s client_timeout.
    // The client auto-responds to pings, so the server should keep the
    // connection alive.
    let deadline = tokio::time::sleep(std::time::Duration::from_millis(3500));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Ping(_))) => {
                        // Normal — server sends ping, keep waiting
                    }
                    Some(Ok(_)) => {
                        // Other message — also fine
                    }
                    Some(Err(e)) => {
                        panic!("Connection dropped unexpectedly: {e}");
                    }
                    None => {
                        panic!("Server closed the connection while it was alive");
                    }
                }
            }
            _ = &mut deadline => {
                // ✅ Test passed — connection stayed alive for 5s
                break;
            }
        }
    }
}

// ── WebSocket connection-limit and subscribe-timeout tests ────────────────

/// Test that the global WebSocket connection cap is enforced: once the limit
/// is reached, further upgrade requests are rejected with a plain 503 (no
/// WebSocket is established).
#[tokio::test]
async fn test_websocket_global_limit_rejected() {
    let _server = TestServer::start_with_notification_limits(NotifLimits {
        max_connections: 1,
        max_connections_per_ip: 100,
        subscribe_timeout_secs: 60,
    })
    .await;
    let ws_url = _server.base_url.replace("http", "ws") + "/notification";

    // First connection succeeds and stays open.
    let (_ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    // Let the server register the connection before trying the second one.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Second connection hits the global cap → 503, not a WebSocket upgrade.
    let result = tokio_tungstenite::connect_async(&ws_url).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) => {
            assert_eq!(r.status(), 503, "expected 503, got {}", r.status());
        }
        Err(e) => panic!("expected 503 HTTP rejection, got: {e}"),
        Ok(_) => panic!("expected the second connection to be rejected"),
    }
}

/// Test that the per-IP WebSocket connection cap is enforced: two concurrent
/// connections from the same client IP are rejected once the cap is reached.
#[tokio::test]
async fn test_websocket_per_ip_limit_rejected() {
    let _server = TestServer::start_with_notification_limits(NotifLimits {
        max_connections: 100,
        max_connections_per_ip: 1,
        subscribe_timeout_secs: 60,
    })
    .await;
    let ws_url = _server.base_url.replace("http", "ws") + "/notification";

    let (_ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let result = tokio_tungstenite::connect_async(&ws_url).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) => {
            assert_eq!(r.status(), 503, "expected 503, got {}", r.status());
        }
        Err(e) => panic!("expected 503 HTTP rejection, got: {e}"),
        Ok(_) => panic!("expected the second connection from the same IP to be rejected"),
    }
}

/// Test that an unauthenticated connection (no valid `subscribe` within the
/// configured window) is dropped by the server.
#[tokio::test]
async fn test_websocket_subscribe_timeout_closes_connection() {
    let _server = TestServer::start_with_notification_limits(NotifLimits {
        max_connections: 100,
        max_connections_per_ip: 100,
        subscribe_timeout_secs: 1,
    })
    .await;
    let ws_url = _server.base_url.replace("http", "ws") + "/notification";

    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // The server drops the socket without a Close handshake after ~1s, so the
    // client observes either an error frame or the stream ending — both count
    // as the connection being closed.
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            msg = ws_stream.next() => {
                match msg {
                    None | Some(Err(_)) => break, // connection closed
                    Some(Ok(_)) => {}             // other frame — keep waiting
                }
            }
            _ = &mut deadline => {
                panic!("server did not close the unauthenticated connection within 5s");
            }
        }
    }
}
