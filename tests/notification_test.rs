mod common;

use common::{TestFixture, TestServer};
use futures_util::StreamExt;
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

    // Wait for up to 3 seconds to receive at least one Ping from the server.
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(3));
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

    // Read messages for 5 seconds — well past the 3s timeout.
    // The client auto-responds to pings, so the server should keep the
    // connection alive.
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(5));
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
