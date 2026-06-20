use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use std::sync::{PoisonError, RwLock};
use std::sync::{RwLockReadGuard, RwLockWriteGuard};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::events::{JwtExpiredEvent, NotificationJwtClaims, NotificationMessage};
use crate::events;

/// Channel capacity for outgoing WebSocket messages per client.
/// A connected WebSocket client.
pub struct ClientState {
    /// The authenticated username (email) for this client.
    /// Set on first successful subscribe.
    pub user: RwLock<String>,
    /// Repos this client is subscribed to.
    pub subscribed_repos: RwLock<HashSet<String>>,
    /// Channel to send outgoing messages to this client's write loop.
    pub sender: mpsc::UnboundedSender<Value>,
    /// JWT token expiration timestamps per repo (repo_id → unix timestamp).
    /// Used by the periodic expiry checker to evict expired subscriptions.
    pub token_expirations: RwLock<HashMap<String, i64>>,
}

impl ClientState {
    fn read_user(&self) -> RwLockReadGuard<'_, String> {
        self.user.read().unwrap_or_else(PoisonError::into_inner)
    }
    fn write_user(&self) -> RwLockWriteGuard<'_, String> {
        self.user.write().unwrap_or_else(PoisonError::into_inner)
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

    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Register a new client and return its assigned ID and sender channel.
    pub fn register_client(&self, sender: mpsc::UnboundedSender<Value>) -> (u64, Arc<ClientState>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let client = Arc::new(ClientState {
            user: RwLock::new(String::new()),
            subscribed_repos: RwLock::new(HashSet::new()),
            sender,
            token_expirations: RwLock::new(HashMap::new()),
        });

        {
            let mut clients = self.write_clients();
            clients.insert(id, client.clone());
        }

        (id, client)
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

        // Remove client from the global client map.
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
    /// If a message channel is full, the client is skipped (non-blocking).
    pub async fn notify_repo(&self, repo_id: &str, message: &NotificationMessage) {
        let event_value = serde_json::to_value(message).unwrap_or(Value::Null);

        let subs = self.read_subscriptions();
        let client_ids = match subs.get(repo_id) {
            Some(ids) => ids.clone(),
            None => return,
        };
        drop(subs);

        let clients = self.read_clients();
        for id in &client_ids {
            if let Some(client) = clients.get(id) {
                let _ = client.sender.send(event_value.clone());
            }
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

    /// Notify a specific user about a repo event.
    /// Returns the number of clients that were notified.
    pub async fn notify_user(
        &self,
        repo_id: &str,
        user: &str,
        message: &NotificationMessage,
    ) -> usize {
        let event_value = serde_json::to_value(message).unwrap_or(Value::Null);

        let subs = self.read_subscriptions();
        let client_ids = match subs.get(repo_id) {
            Some(ids) => ids.clone(),
            None => return 0,
        };
        drop(subs);

        let clients = self.read_clients();
        let mut notified = 0;
        for id in &client_ids {
            if let Some(client) = clients.get(id) {
                let u = client.read_user();
                if *u == user && client.sender.send(event_value.clone()).is_ok() {
                    notified += 1;
                }
            }
        }
        notified
    }
}

impl NotificationManager {
    /// Start a background task that listens for repo-update events on the
    /// global broadcast channel and forwards them to WebSocket subscribers.
    /// Pass a `CancellationToken` to allow graceful shutdown.
    pub async fn start_event_listener(&self, token: CancellationToken) {
        let mut rx = events::subscribe_repo_updates();
        let mgr = self.clone();
        tokio::spawn(async move {
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
        });
    }

    /// Start a background task that checks for expired JWT tokens every hour
    /// and sends `jwt-expired` notifications to affected clients.
    /// Pass a `CancellationToken` to allow graceful shutdown.
    pub async fn start_token_expiry_checker(&self, token: CancellationToken) {
        let mgr = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => mgr.check_expired_tokens().await,
                    _ = token.cancelled() => {
                        tracing::info!("Token expiry checker shutting down");
                        break;
                    }
                }
            }
        });
    }

    /// Iterate all clients and check their JWT token expirations.
    /// Expired tokens are removed and a `jwt-expired` message is sent to the client,
    /// matching the seafile notification-server behavior so the seahub frontend
    /// can fetch a new JWT and resubscribe.
    async fn check_expired_tokens(&self) {
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
                if let Ok(value) = serde_json::to_value(&msg) {
                    let _ = client.sender.send(value);
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
        Self::new()
    }
}

/// Validate a JWT token against the notification server's private key.
/// Returns the claims if valid, None otherwise.
pub fn validate_notification_jwt(
    token: &str,
    private_key: &str,
    expected_repo_id: &str,
) -> Option<NotificationJwtClaims> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation};

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
