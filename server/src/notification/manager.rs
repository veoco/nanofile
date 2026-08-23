use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLock};
use std::sync::{RwLockReadGuard, RwLockWriteGuard};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::events::{JwtExpiredEvent, NotificationJwtClaims, NotificationMessage};
use infra::events;

/// Channel capacity for outgoing WebSocket messages per client.
/// A connected WebSocket client.
pub struct ClientState {
    /// The authenticated username (email) for this client.
    /// Set on first successful subscribe.
    pub user: RwLock<String>,
    /// Repos this client is subscribed to.
    pub subscribed_repos: RwLock<HashSet<String>>,
    /// Bounded channel to send outgoing (pre-serialized) messages to this
    /// client's write loop.
    pub sender: mpsc::Sender<Arc<[u8]>>,
    /// JWT token expiration timestamps per repo (repo_id → unix timestamp).
    /// Used by the periodic expiry checker to evict expired subscriptions.
    pub token_expirations: RwLock<HashMap<String, i64>>,
    /// The client's effective IP (TCP peer, or X-Forwarded-For when behind a
    /// trusted proxy). Used for the per-IP connection cap and audit logging.
    pub peer_ip: String,
}

impl ClientState {
    fn read_user(&self) -> RwLockReadGuard<'_, String> {
        self.user.read().unwrap_or_else(PoisonError::into_inner)
    }
    fn write_user(&self) -> RwLockWriteGuard<'_, String> {
        self.user.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the client has successfully subscribed at least once (i.e. has
    /// presented a valid repo JWT). The username is only ever set — never
    /// cleared — so this transition is one-way.
    pub fn is_authenticated(&self) -> bool {
        !self.read_user().is_empty()
    }
    fn write_subscribed_repos(&self) -> RwLockWriteGuard<'_, HashSet<String>> {
        self.subscribed_repos
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
    fn read_token_expirations(&self) -> RwLockReadGuard<'_, HashMap<String, i64>> {
        self.token_expirations
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }
    fn write_token_expirations(&self) -> RwLockWriteGuard<'_, HashMap<String, i64>> {
        self.token_expirations
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// In-memory notification subscription manager.
///
/// Tracks all connected WebSocket clients and their repo subscriptions.
/// Thread-safe: all mutable state is behind `Arc<RwLock<...>>`.
#[derive(Clone)]
pub struct NotificationManager {
    /// All connected clients, keyed by client ID.
    clients: Arc<RwLock<HashMap<u64, Arc<ClientState>>>>,
    /// Subscriptions: repo_id → set of client IDs subscribed to that repo.
    subscriptions: Arc<RwLock<HashMap<String, HashSet<u64>>>>,
    /// Monotonically increasing client ID counter.
    next_id: Arc<AtomicU64>,
    /// Global cap on concurrent connections (0 = unlimited).
    max_connections: u64,
    /// Cap on concurrent connections per client IP (0 = unlimited).
    max_connections_per_ip: u64,
}

impl NotificationManager {
    fn read_clients(&self) -> RwLockReadGuard<'_, HashMap<u64, Arc<ClientState>>> {
        self.clients.read().unwrap_or_else(PoisonError::into_inner)
    }
    fn write_clients(&self) -> RwLockWriteGuard<'_, HashMap<u64, Arc<ClientState>>> {
        self.clients.write().unwrap_or_else(PoisonError::into_inner)
    }
    fn read_subscriptions(&self) -> RwLockReadGuard<'_, HashMap<String, HashSet<u64>>> {
        self.subscriptions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }
    fn write_subscriptions(&self) -> RwLockWriteGuard<'_, HashMap<String, HashSet<u64>>> {
        self.subscriptions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn new(max_connections: u64, max_connections_per_ip: u64) -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            max_connections,
            max_connections_per_ip,
        }
    }

    /// Global cap on concurrent connections (0 = unlimited).
    pub fn max_connections(&self) -> u64 {
        self.max_connections
    }

    /// Cap on concurrent connections per client IP (0 = unlimited).
    pub fn max_connections_per_ip(&self) -> u64 {
        self.max_connections_per_ip
    }

    /// Number of currently connected clients.
    pub fn connection_count(&self) -> usize {
        self.read_clients().len()
    }

    /// Number of currently connected clients from `peer_ip`.
    pub fn connection_count_by_ip(&self, peer_ip: &str) -> usize {
        self.read_clients()
            .values()
            .filter(|c| c.peer_ip == peer_ip)
            .count()
    }

    /// Register a new client and return its assigned ID and client state.
    ///
    /// Enforces the global and per-IP connection caps atomically under the
    /// client-map write lock. Returns `None` when either cap is reached.
    pub fn register_client(
        &self,
        sender: mpsc::Sender<Arc<[u8]>>,
        peer_ip: String,
    ) -> Option<(u64, Arc<ClientState>)> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let client = Arc::new(ClientState {
            user: RwLock::new(String::new()),
            subscribed_repos: RwLock::new(HashSet::new()),
            sender,
            token_expirations: RwLock::new(HashMap::new()),
            peer_ip,
        });

        {
            let mut clients = self.write_clients();
            // Per-IP limit is checked by scanning the (bounded) client map
            // under the same write lock, so no separate counter to keep in
            // sync. Registration is infrequent so the scan is negligible.
            let same_ip = clients
                .values()
                .filter(|c| c.peer_ip == client.peer_ip)
                .count();
            let over_ip =
                self.max_connections_per_ip > 0 && same_ip as u64 >= self.max_connections_per_ip;
            let over_global =
                self.max_connections > 0 && clients.len() as u64 >= self.max_connections;
            if over_global || over_ip {
                return None;
            }
            clients.insert(id, client.clone());
        }

        Some((id, client))
    }

    /// Remove a client and all its subscriptions.
    pub async fn unregister_client(&self, client_id: u64) {
        // Remove client from all subscription lists.
        {
            let subs = self.read_subscriptions();
            let repos: Vec<String> = subs
                .iter()
                .filter(|(_, ids)| ids.contains(&client_id))
                .map(|(repo, _)| repo.clone())
                .collect();
            drop(subs);

            let mut subs = self.write_subscriptions();
            for repo in &repos {
                if let Some(ids) = subs.get_mut(repo) {
                    ids.remove(&client_id);
                    if ids.is_empty() {
                        subs.remove(repo);
                    }
                }
            }
        }

        // Remove client from the global client map. `remove` returns the
        // client only if it was still present, so this is idempotent — the
        // read/write loops and the socket teardown all call it for the same
        // client_id.
        {
            let mut clients = self.write_clients();
            clients.remove(&client_id);
        }
    }

    /// Subscribe a client to a set of repos.
    /// `username` is extracted from the validated JWT token.
    /// `repos` is a list of (repo_id, jwt_exp_timestamp) pairs.
    pub async fn subscribe(&self, client_id: u64, username: &str, repos: &[(String, i64)]) {
        let clients = self.read_clients();
        let client = match clients.get(&client_id) {
            Some(c) => c.clone(),
            None => return,
        };
        drop(clients);

        // Set the username on first subscription.
        {
            let mut user = client.write_user();
            if user.is_empty() {
                *user = username.to_string();
            }
        }

        let mut subs = self.write_subscriptions();
        let mut subscribed = client.write_subscribed_repos();
        let mut expirations = client.write_token_expirations();

        for (repo_id, exp) in repos {
            subs.entry(repo_id.clone()).or_default().insert(client_id);
            subscribed.insert(repo_id.clone());
            expirations.insert(repo_id.clone(), *exp);
        }
    }

    /// Unsubscribe a client from a set of repos.
    pub async fn unsubscribe(&self, client_id: u64, repo_ids: &[String]) {
        let mut subs = self.write_subscriptions();
        let clients = self.read_clients();
        let client = clients.get(&client_id);

        let mut subscribed = match client {
            Some(c) => c.write_subscribed_repos(),
            None => return,
        };

        for repo_id in repo_ids {
            subscribed.remove(repo_id);
            if let Some(ids) = subs.get_mut(repo_id) {
                ids.remove(&client_id);
                if ids.is_empty() {
                    subs.remove(repo_id);
                }
            }
        }
    }

    /// Notify all subscribers of a repo about an event.
    ///
    /// The message is serialized once and the bytes shared across subscribers.
    /// If a client's bounded channel is full, that client is skipped
    /// (non-blocking) rather than unboundedly buffering.
    pub async fn notify_repo(&self, repo_id: &str, message: &NotificationMessage) {
        let bytes: Arc<[u8]> = Arc::from(serde_json::to_vec(message).unwrap_or_default());

        let subs = self.read_subscriptions();
        let client_ids = match subs.get(repo_id) {
            Some(ids) => ids.clone(),
            None => return,
        };
        drop(subs);

        let clients = self.read_clients();
        for id in &client_ids {
            if let Some(client) = clients.get(id) {
                let _ = client.sender.try_send(bytes.clone());
            }
        }
    }

    /// Send a message directly to a single connected client.
    ///
    /// Used for client-specific control messages such as `jwt-expired`.
    /// Non-blocking: if the client's bounded channel is full the message is
    /// dropped, mirroring [`notify_repo`](Self::notify_repo).
    pub async fn notify_client(&self, client_id: u64, message: &NotificationMessage) {
        let bytes: Arc<[u8]> = Arc::from(serde_json::to_vec(message).unwrap_or_default());
        let clients = self.read_clients();
        if let Some(client) = clients.get(&client_id) {
            let _ = client.sender.try_send(bytes);
        }
    }

    /// Notify all subscribers of a repo about an event.
    /// Convenience method that accepts a serializable event.
    pub async fn notify(&self, event: impl Into<NotificationMessage>) {
        let msg = event.into();
        if let Some(repo_id) = extract_repo_id(&msg) {
            self.notify_repo(&repo_id, &msg).await;
        }
    }
}

impl NotificationManager {
    /// Run the event listener loop that forwards repo-update events from the
    /// global broadcast channel to WebSocket subscribers.
    ///
    /// Runs until `token` is cancelled. Does **not** spawn internally — the
    /// caller (typically [`Scheduler`](crate::scheduler::Scheduler)) owns the
    /// tokio task boundary.
    pub async fn run_event_listener(&self, token: CancellationToken) {
        let mut rx = events::subscribe_repo_updates();
        let mgr = self.clone();
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok((repo_id, commit_id)) => {
                            let event = super::events::RepoUpdateEvent::new(repo_id, commit_id);
                            mgr.notify(event).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Notification listener lagged by {n} messages, resuming");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::error!("Notification broadcast channel closed");
                            break;
                        }
                    }
                }
                _ = token.cancelled() => {
                    tracing::info!("Notification listener shutting down");
                    break;
                }
            }
        }
    }

    /// Check all clients for expired JWT tokens and send `jwt-expired`
    /// notifications.
    ///
    /// This is a single-shot check intended to be called periodically by
    /// the [`Scheduler`](crate::scheduler::Scheduler).
    pub async fn check_expired_tokens(&self) {
        let now = chrono::Utc::now().timestamp();
        let clients = self.read_clients();
        for (client_id, client) in clients.iter() {
            // Collect expired repo_ids.
            let expired: Vec<String> = {
                let exps = client.read_token_expirations();
                exps.iter()
                    .filter(|&(_, exp)| *exp <= now)
                    .map(|(repo_id, _)| repo_id.clone())
                    .collect()
            };
            if expired.is_empty() {
                continue;
            }
            // Remove from token_expirations.
            {
                let mut exps = client.write_token_expirations();
                for repo_id in &expired {
                    exps.remove(repo_id);
                }
            }
            // Remove from subscribed_repos.
            {
                let mut subscribed = client.write_subscribed_repos();
                for repo_id in &expired {
                    subscribed.remove(repo_id);
                }
            }
            // Remove from global subscriptions map.
            {
                let mut subs = self.write_subscriptions();
                for repo_id in &expired {
                    if let Some(ids) = subs.get_mut(repo_id) {
                        ids.remove(client_id);
                        if ids.is_empty() {
                            subs.remove(repo_id);
                        }
                    }
                }
            }
            // Send jwt-expired notification to the client.
            for repo_id in &expired {
                let event = JwtExpiredEvent {
                    repo_id: repo_id.clone(),
                };
                let msg: NotificationMessage = event.into();
                if let Ok(bytes) = serde_json::to_vec(&msg) {
                    let bytes: Arc<[u8]> = Arc::from(bytes);
                    let _ = client.sender.try_send(bytes);
                }
            }
        }
    }

    /// Gracefully shut down all WebSocket connections by clearing all client
    /// and subscription state. Dropping the mpsc senders causes each write
    /// task's `rx.recv()` to return `None`, which triggers `unregister_client`
    /// and a clean exit.
    pub async fn shutdown(&self) {
        tracing::info!("Shutting down notification manager");
        self.write_clients().clear();
        self.write_subscriptions().clear();
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new(1024, 64)
    }
}

/// Validate a JWT token against the notification server's private key.
/// Returns the claims if valid, None otherwise.
pub fn validate_notification_jwt(
    token: &str,
    private_key: &str,
    expected_repo_id: &str,
) -> Option<NotificationJwtClaims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.required_spec_claims = std::collections::HashSet::new();
    // We check repo_id manually below.
    validation.sub = None;
    validation.iss = None;

    let key = DecodingKey::from_secret(private_key.as_bytes());
    let token_data =
        jsonwebtoken::decode::<NotificationJwtClaims>(token, &key, &validation).ok()?;

    let claims = token_data.claims;

    // Verify the repo_id matches.
    if claims.repo_id != expected_repo_id {
        return None;
    }

    Some(claims)
}

fn extract_repo_id(msg: &NotificationMessage) -> Option<String> {
    msg.content
        .get("repo_id")
        .and_then(|v| v.as_str())
        .map(String::from)
}
