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
            conn = listener.accept() => match conn {
                Ok(accepted) => accepted,
                // Some accept errors describe the connection that was queued, not the
                // listener: a peer that resets while it waits in the accept queue is
                // reported as `ECONNABORTED`/`ECONNRESET` (Windows does this for an
                // ordinary aborted connection), and tokio hands the error straight to
                // us. Propagating one ended this loop silently — the run loop only
                // awaits the task on a shutdown signal — leaving the port bound with
                // nobody accepting. Keep serving instead.
                Err(error) if is_transient_accept_error(error.kind()) => {
                    tracing::debug!("accept failed, continuing: {error}");
                    continue;
                }
                Err(error) => return Err(error),
            },
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

/// Whether an `accept` error is about one queued connection rather than the listener.
///
/// Tokio's own documentation calls these "not necessarily fatal"; the kinds here are
/// what a peer aborting a connection produces (`ECONNABORTED`, and `ECONNRESET` for
/// Windows' aborted queue entry), plus a call interrupted by a signal. Anything else
/// means the listening socket itself failed, which a retry cannot heal.
fn is_transient_accept_error(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind;
    matches!(
        kind,
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::is_transient_accept_error;
    use std::io::ErrorKind;

    /// One connection aborted while it waited for the accept loop to reach it.
    #[test]
    fn an_aborted_queued_connection_is_not_fatal() {
        for kind in [
            ErrorKind::ConnectionAborted,
            ErrorKind::ConnectionReset,
            ErrorKind::Interrupted,
        ] {
            assert!(
                is_transient_accept_error(kind),
                "{kind:?} must not end the accept loop"
            );
        }
    }

    /// A listener that failed is not something the loop can retry past.
    #[test]
    fn a_broken_listener_is_fatal() {
        for kind in [
            ErrorKind::Other,
            ErrorKind::InvalidInput,
            ErrorKind::NotConnected,
            ErrorKind::PermissionDenied,
        ] {
            assert!(
                !is_transient_accept_error(kind),
                "{kind:?} must end the accept loop"
            );
        }
    }
}
