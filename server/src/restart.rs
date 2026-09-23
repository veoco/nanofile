//! In-place server restart: the signal an admin page raises, the listening
//! socket that survives the restart, and the record of a restart that could not
//! move to the address it was asked to.
//!
//! A restart here is *not* a re-exec: the process stays alive, tears the HTTP
//! server and every piece of `AppState` down through the normal graceful
//! shutdown, and builds them again from the settings table. That is what makes
//! the same mechanism work in all three shapes of this binary — headless,
//! desktop tray and Windows service — without dropping the tray icon or
//! breaking the service control manager's association with the process.
//!
//! What it deliberately does not do is load a new binary; the log subscriber
//! and the tray are also decided before the server loop starts (see
//! `infra::settings::Apply::ProcessRestart`).

use std::io;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Response header `/health` carries the current server generation in.
///
/// The "restarting" page polls readiness, and readiness alone cannot tell the
/// server that is going away from the one that replaced it: the listening socket
/// stays bound (that is the point of [`BoundListener`]), so a request sent just
/// before the restart is answered normally. The generation number changes with
/// every in-place restart, which is what makes "the server is back" mean *this
/// is a different server*.
pub const GENERATION_HEADER: &str = "x-nanofile-generation";

/// Bumped at the start of every server generation.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Start a new server generation and return its number.
pub fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// The generation currently serving, as `/health` reports it.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Why one generation of the server returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The process is shutting down: Ctrl+C, SIGTERM, a tray quit, or a
    /// service stop request.
    Shutdown,
    /// An administrator asked for a restart from the settings page.
    Restart,
}

impl StopReason {
    /// Whether the run loop should build the server again.
    pub fn should_restart(self) -> bool {
        matches!(self, StopReason::Restart)
    }
}

/// The admin-triggered restart request, shared between the HTTP handler that
/// raises it and the run loop that acts on it.
///
/// A fresh instance is built with every `AppState`, so a request is scoped to
/// the generation that received it: nothing has to be reset between
/// generations, and a request that arrives while a restart is already draining
/// cannot leak into the next one.
#[derive(Debug, Default)]
pub struct RestartSignal {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl RestartSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the run loop to stop and come back up.
    ///
    /// `notify_one` (not `notify_waiters`) so a request that lands before the
    /// loop starts waiting on this signal is still seen: the notify stores a
    /// permit that the next `wait` consumes immediately.
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Resolves once a restart has been requested.
    ///
    /// Cancel safe: dropping it (because another `select!` branch won) leaves
    /// the request in place for the next wait, which is exactly what should
    /// happen if the admin clicks the button twice.
    pub async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_requested() {
                return;
            }
            notified.await;
        }
    }
}

/// The listening socket a restart carries over.
///
/// Re-binding the same port right after a graceful shutdown is not portable:
/// Windows (and Linux without `SO_REUSEADDR` on the closing side) refuses a
/// `bind` while a drained connection still sits in `TIME_WAIT`, which can last
/// minutes. Keeping the original socket and handing each generation a
/// duplicate avoids the whole problem: the port is never released, and new
/// connections wait in the backlog until the next generation accepts them.
pub struct BoundListener {
    listener: std::net::TcpListener,
    /// The address as configured, which is the identity a restart compares
    /// against — not the resolved local address.
    addr: String,
}

impl BoundListener {
    /// Bind `addr`, or keep the current socket when it is already the one asked
    /// for.
    ///
    /// `retry` is set for a restart: a saved address that is briefly still held
    /// by something else (a previous instance draining, a service stopping) is
    /// worth waiting out. The first start does not retry — a port that is taken
    /// at boot is a configuration error the operator has to see.
    pub async fn acquire(
        current: &mut Option<BoundListener>,
        addr: &str,
        retry: bool,
    ) -> io::Result<()> {
        if current.as_ref().is_some_and(|bound| bound.addr == addr) {
            return Ok(());
        }

        const ATTEMPTS: u32 = 30;
        const DELAY: Duration = Duration::from_millis(500);
        let attempts = if retry { ATTEMPTS } else { 1 };

        let mut last_err = None;
        for attempt in 0..attempts {
            match std::net::TcpListener::bind(addr) {
                Ok(listener) => {
                    // Dropping the old socket is what releases the previous
                    // address; the new one is already bound, so a failure
                    // between the two cannot leave the server without a
                    // listener.
                    *current = Some(BoundListener {
                        listener,
                        addr: addr.to_string(),
                    });
                    return Ok(());
                }
                Err(e) => last_err = Some(e),
            }
            if attempt + 1 < attempts {
                tokio::time::sleep(DELAY).await;
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("no bind attempt made")))
    }

    /// A duplicate of the listening socket, ready to be handed to a server
    /// generation. The original stays open, so the socket outlives it.
    pub fn to_tokio(&self) -> io::Result<tokio::net::TcpListener> {
        let duplicate = self.listener.try_clone()?;
        duplicate.set_nonblocking(true)?;
        tokio::net::TcpListener::from_std(duplicate)
    }

    /// The address as configured.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// The address the socket actually listens on (a configured port of `0`
    /// resolves to the kernel-assigned one here).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// The reason the last restart kept the previous listener, if it did.
///
/// The page that offers the restart button reads this, because the failure
/// happens after the RESTART response has been sent: the admin would otherwise
/// see a saved address that the server silently is not using.
static RESTART_FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// Record that a restart could not move to the configured address.
pub fn note_failure(message: impl Into<String>) {
    let message = message.into();
    tracing::error!("{message}");
    let mut slot = RESTART_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some(message);
}

/// Forget a recorded restart failure — called when a listener is acquired for
/// the address the configuration asked for.
pub fn clear_failure() {
    let mut slot = RESTART_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = None;
}

/// The recorded restart failure, for the settings page's banner.
pub fn failure() -> Option<String> {
    RESTART_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_request_wakes_a_waiter_and_stays_observable() {
        let signal = RestartSignal::new();
        assert!(!signal.is_requested());

        let waiter = {
            let signal = &signal;
            async move { signal.wait().await }
        };
        let signal_ref = &signal;
        let request = async {
            tokio::task::yield_now().await;
            signal_ref.request();
        };
        tokio::join!(waiter, request);
        assert!(signal.is_requested());
    }

    #[tokio::test]
    async fn a_request_that_arrives_before_the_wait_is_not_lost() {
        let signal = RestartSignal::new();
        signal.request();
        // The permit is stored, so this returns immediately instead of parking.
        tokio::time::timeout(Duration::from_secs(1), signal.wait())
            .await
            .expect("a request raised before the wait must still be seen");
    }

    #[tokio::test]
    async fn the_same_address_reuses_the_socket() {
        // A configured port of `0` makes the kernel pick one; re-acquiring the
        // same *configured* string has to keep that socket rather than bind a
        // second random one.
        let mut bound = None;
        BoundListener::acquire(&mut bound, "127.0.0.1:0", false)
            .await
            .unwrap();
        let port = bound.as_ref().unwrap().local_addr().unwrap().port();

        BoundListener::acquire(&mut bound, "127.0.0.1:0", false)
            .await
            .unwrap();
        assert_eq!(bound.as_ref().unwrap().local_addr().unwrap().port(), port);
        assert!(bound.as_ref().unwrap().to_tokio().is_ok());

        // The socket is still bound: an unrelated listener cannot take it.
        assert!(std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_err());
    }

    #[tokio::test]
    async fn a_changed_address_rebinds() {
        let mut bound = None;
        BoundListener::acquire(&mut bound, "127.0.0.1:0", false)
            .await
            .unwrap();
        let old = bound.as_ref().unwrap().local_addr().unwrap().port();

        // Bind a concrete port so the rebind has something stable to check.
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let new_port = probe.local_addr().unwrap().port();
        drop(probe);
        let new_addr = format!("127.0.0.1:{new_port}");

        BoundListener::acquire(&mut bound, &new_addr, false)
            .await
            .unwrap();
        assert_eq!(bound.as_ref().unwrap().addr(), new_addr);
        assert_eq!(
            bound.as_ref().unwrap().local_addr().unwrap().port(),
            new_port
        );
        assert_ne!(old, new_port);
    }

    #[tokio::test]
    async fn a_failed_restart_bind_reports_the_error() {
        // Hold a port, then ask the restart path (retry = false, so the test
        // does not wait out the real retry window) to move to it.
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap();

        let mut bound = None;
        BoundListener::acquire(&mut bound, "127.0.0.1:0", false)
            .await
            .unwrap();
        let err = BoundListener::acquire(&mut bound, &addr.to_string(), false)
            .await
            .expect_err("a held port must not be claimed");
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn the_failure_record_is_set_and_cleared() {
        // Process-wide, so keep this test's value to itself by clearing it at
        // both ends rather than asserting on the initial state.
        clear_failure();
        assert!(failure().is_none());
        note_failure("could not bind 0.0.0.0:8443");
        assert_eq!(failure().as_deref(), Some("could not bind 0.0.0.0:8443"));
        clear_failure();
        assert!(failure().is_none());
    }

    #[test]
    fn only_a_restart_rebuilds_the_server() {
        assert!(StopReason::Restart.should_restart());
        assert!(!StopReason::Shutdown.should_restart());
    }

    /// Every generation is distinct, which is the property the restart page
    /// depends on: a number that repeated would look like the server never came
    /// back.
    #[test]
    fn every_generation_is_a_new_number() {
        let first = next_generation();
        assert_eq!(generation(), first);
        let second = next_generation();
        assert!(second > first, "{second} must follow {first}");
        assert_eq!(generation(), second);
    }
}
