//! Tests for the custom HTTP serve loop (`server::serve`).
//!
//! `axum::serve` builds each connection without a hyper `Timer`, which makes
//! hyper's default `header_read_timeout` inert — a client that dribbles request
//! headers holds the connection open indefinitely. These tests pin the
//! replacement behaviour.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A client that stops mid-request-head must be disconnected by the header-read
/// deadline rather than keeping the connection (and its task) forever.
#[tokio::test]
async fn slow_headers_are_dropped() {
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = server::serve::serve_with_timeouts(
            listener,
            app,
            shutdown_rx,
            Some(Duration::from_millis(300)),
        )
        .await;
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Send a partial request head, then nothing at all.
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap();

    let mut buf = [0u8; 1024];
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.read(&mut buf).await {
                // EOF or a reset both mean the server gave up on us.
                Ok(0) | Err(_) => return true,
                // Drain a 408 response if hyper sends one before closing.
                Ok(_) => continue,
            }
        }
    })
    .await;
    assert_eq!(
        closed,
        Ok(true),
        "the connection should be closed by the header-read timeout"
    );

    let _ = shutdown_tx.send(());
}

/// A complete request is still served normally, and a second request on the
/// same connection works (keep-alive is preserved).
#[tokio::test]
async fn normal_requests_still_work() {
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = server::serve::serve_with_timeouts(
            listener,
            app,
            shutdown_rx,
            Some(Duration::from_secs(30)),
        )
        .await;
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    for _ in 0..2 {
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 1024];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("response within timeout")
            .expect("read succeeds");
        let response = String::from_utf8_lossy(&buf[..read]).to_string();
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "expected 200, got {response:?}"
        );
        assert!(response.ends_with("ok"), "expected body, got {response:?}");
    }

    let _ = shutdown_tx.send(());
}

/// The shutdown signal stops the accept loop and lets the serve future finish.
#[tokio::test]
async fn shutdown_signal_stops_the_listener() {
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        server::serve::serve_with_timeouts(
            listener,
            app,
            shutdown_rx,
            Some(Duration::from_secs(30)),
        )
        .await
    });

    shutdown_tx.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("serve future should finish promptly")
        .expect("serve task should not panic");
    assert!(
        result.is_ok(),
        "graceful shutdown should return Ok, got {result:?}"
    );
}
