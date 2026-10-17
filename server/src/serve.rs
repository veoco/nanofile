//! HTTP server loop with hyper-level timeouts.
//!
//! `axum::serve` never installs a hyper `Timer`, so hyper's `header_read_timeout`
//! is inert and slow-header (slowloris) connections are unbounded;
//! [`serve_with_timeouts`] is axum's own accept loop with a timer, a header-read
//! deadline, connect-info injection, WebSocket upgrades and graceful shutdown.
//! The binary and the test harness both build their router here.
/// Injects `ConnectInfo` into every request before it reaches the router: the
/// custom loop cannot construct `axum::serve`'s private `IncomingStream`, so the
/// request is extended here instead.
#[derive(Clone)]
pub struct ConnectInfoService {
    app: axum::Router,
    remote_addr: std::net::SocketAddr,
}

impl tower::Service<axum::http::Request<axum::body::Body>> for ConnectInfoService {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: axum::http::Request<axum::body::Body>) -> Self::Future {
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(self.remote_addr));
        let mut app = self.app.clone();
        Box::pin(async move { app.call(req).await })
    }
}

/// Serve `app` on `listener` with a hyper-level header-read timeout.
///
/// `axum::serve` never installs a hyper `Timer`, so hyper's `header_read_timeout`
/// arms no deadline and a client trickling request headers can hold a connection
/// open forever; tower-http's `TimeoutLayer` does not help, because its timer only
/// starts once the request head has been parsed. Connect info, WebSocket upgrades
/// and graceful shutdown behave as before.
pub async fn serve_with_timeouts(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    shutdown: tokio::sync::oneshot::Receiver<()>,
    header_read_timeout: Option<std::time::Duration>,
) -> std::io::Result<()> {
    use futures_util::FutureExt;
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;
    use tower::ServiceExt;

    // Drop the receiver when the shutdown future resolves: that closes the
    // channel, which is how every in-flight connection learns to drain.
    let (signal_tx, signal_rx) = tokio::sync::watch::channel(());
    tokio::spawn(async move {
        let _ = shutdown.await;
        tracing::info!("Stopping new connections, draining in-flight requests");
        drop(signal_rx);
    });

    let (close_tx, close_rx) = tokio::sync::watch::channel(());

    loop {
        let (stream, remote_addr) = tokio::select! {
            conn = listener.accept() => conn?,
            _ = signal_tx.closed() => break,
        };
        let io = TokioIo::new(stream);
        let service = ConnectInfoService {
            app: app.clone(),
            remote_addr,
        };
        // hyper hands the service a `Request<Incoming>`; axum's own loop maps
        // the body to `axum::body::Body` before adapting it, so do the same.
        let hyper_service = TowerToHyperService::new(service.map_request(
            |req: axum::http::Request<hyper::body::Incoming>| req.map(axum::body::Body::new),
        ));

        let signal_tx = signal_tx.clone();
        let close_rx = close_rx.clone();
        tokio::spawn(async move {
            let mut builder = Builder::new(TokioExecutor::new());
            builder.http2().enable_connect_protocol();
            // Without `.timer(...)` hyper cannot enforce any HTTP/1 timeout.
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(header_read_timeout);

            let mut conn =
                std::pin::pin!(builder.serve_connection_with_upgrades(io, hyper_service));
            // `fuse` so the branch stops being polled once the channel closes, which
            // would otherwise spin the select on a resolved future while draining.
            let mut signal_closed = std::pin::pin!(signal_tx.closed().fuse());
            loop {
                tokio::select! {
                    result = conn.as_mut() => {
                        if let Err(err) = result {
                            tracing::debug!("connection error: {err}");
                        }
                        break;
                    }
                    _ = &mut signal_closed => {
                        conn.as_mut().graceful_shutdown();
                    }
                }
            }
            drop(close_rx);
        });
    }

    drop(close_rx);
    drop(listener);

    // Wait for in-flight connections to finish.
    close_tx.closed().await;
    Ok(())
}
