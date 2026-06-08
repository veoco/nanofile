use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use std::sync::RwLock;
use tokio::sync::mpsc;

use super::events::{NotificationJwtClaims, NotificationMessage, RepoSubscription};
use super::events_channel;

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
        });

        {
            let mut clients = self.clients.write().unwrap();
            clients.insert(id, client.clone());
        }

        (id, client)
    }

    /// Remove a client and all its subscriptions.
    pub async fn unregister_client(&self, client_id: u64) {
        // Remove client from all subscription lists.
        {
            let subs = self.subscriptions.read().unwrap();
            let repos: Vec<String> = subs
                .iter()
                .filter(|(_, ids)| ids.contains(&client_id))
                .map(|(repo, _)| repo.clone())
                .collect();
            drop(subs);

            let mut subs = self.subscriptions.write().unwrap();
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
            let mut clients = self.clients.write().unwrap();
            clients.remove(&client_id);
        }
    }

    /// Subscribe a client to a set of repos.
    /// `username` is extracted from the validated JWT token.
    pub async fn subscribe(&self, client_id: u64, username: &str, repos: &[RepoSubscription]) {
        let clients = self.clients.read().unwrap();
        let client = match clients.get(&client_id) {
            Some(c) => c.clone(),
            None => return,
        };
        drop(clients);

        // Set the username on first subscription.
        {
            let mut user = client.user.write().unwrap();
            if user.is_empty() {
                *user = username.to_string();
            }
        }

        let mut subs = self.subscriptions.write().unwrap();
        let mut subscribed = client.subscribed_repos.write().unwrap();

        for repo in repos {
            subs.entry(repo.id.clone()).or_default().insert(client_id);
            subscribed.insert(repo.id.clone());
        }
    }

    /// Unsubscribe a client from a set of repos.
    pub async fn unsubscribe(&self, client_id: u64, repo_ids: &[String]) {
        let mut subs = self.subscriptions.write().unwrap();
        let clients = self.clients.read().unwrap();
        let client = clients.get(&client_id);

        let mut subscribed = match client {
            Some(c) => c.subscribed_repos.write().unwrap(),
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

        let subs = self.subscriptions.read().unwrap();
        let client_ids = match subs.get(repo_id) {
            Some(ids) => ids.clone(),
            None => return,
        };
        drop(subs);

        let clients = self.clients.read().unwrap();
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

        let subs = self.subscriptions.read().unwrap();
        let client_ids = match subs.get(repo_id) {
            Some(ids) => ids.clone(),
            None => return 0,
        };
        drop(subs);

        let clients = self.clients.read().unwrap();
        let mut notified = 0;
        for id in &client_ids {
            if let Some(client) = clients.get(id) {
                let u = client.user.read().unwrap();
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
    pub async fn start_event_listener(&self) {
        let mut rx = events_channel::subscribe_repo_updates();
        let mgr = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok((repo_id, commit_id)) => {
                        let event = super::events::RepoUpdateEvent::new(repo_id, commit_id);
                        mgr.notify(event).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Receiver fell behind — messages were dropped. This happens
                        // when events are produced faster than they're forwarded.
                        // Continue listening rather than crashing the listener task.
                        tracing::warn!("Notification listener lagged by {n} messages, resuming");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::error!("Notification broadcast channel closed");
                        break;
                    }
                }
            }
        });
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
