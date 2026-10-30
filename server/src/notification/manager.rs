use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLock};
use std::sync::{RwLockReadGuard, RwLockWriteGuard};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::events::{JwtExpiredEvent, NotificationJwtClaims, NotificationMessage};
use infra::events;

use crate::repository::Repositories;

/// A connected WebSocket client.
pub struct ClientState {
    /// The authenticated username (email), set on first successful subscribe.
    pub user: RwLock<String>,
    pub subscribed_repos: RwLock<HashSet<String>>,
    /// Bounded channel of pre-serialized messages to this client's write loop.
    pub sender: mpsc::Sender<Arc<[u8]>>,
    /// JWT expiration per repo (repo_id → unix timestamp), for the expiry checker.
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

    /// Whether the client has subscribed at least once with a valid repo JWT.
    /// The username is only ever set, never cleared, so the transition is one-way.
    pub fn is_authenticated(&self) -> bool {
        !self.read_user().is_empty()
    }
    fn read_subscribed_repos(&self) -> RwLockReadGuard<'_, HashSet<String>> {
        self.subscribed_repos
            .read()
            .unwrap_or_else(PoisonError::into_inner)
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

/// In-memory notification subscription manager: the connected WebSocket clients
/// and their repo subscriptions, all mutable state behind `Arc<RwLock<...>>`.
#[derive(Clone)]
pub struct NotificationManager {
    /// All connected clients, keyed by client ID.
    clients: Arc<RwLock<HashMap<u64, Arc<ClientState>>>>,
    /// Subscriptions: repo_id → set of client IDs subscribed to that repo.
    subscriptions: Arc<RwLock<HashMap<String, HashSet<u64>>>>,
    /// Monotonically increasing client ID counter.
    next_id: Arc<AtomicU64>,
    /// Global connection cap (0 = unlimited), atomic so an admin can change it live.
    max_connections: Arc<AtomicU64>,
    /// Cap on concurrent connections per client IP (0 = unlimited).
    max_connections_per_ip: Arc<AtomicU64>,
    /// Library membership is re-checked on subscribe and by the periodic
    /// token-expiry pass, so a removed member stops receiving notifications.
    repos: Arc<Repositories>,
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

    pub fn new(
        max_connections: u64,
        max_connections_per_ip: u64,
        repos: Arc<Repositories>,
    ) -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            max_connections: Arc::new(AtomicU64::new(max_connections)),
            max_connections_per_ip: Arc::new(AtomicU64::new(max_connections_per_ip)),
            repos,
        }
    }

    /// Whether `username` may still read `repo_id`.
    ///
    /// A notification JWT lives three days; without this check a removed member
    /// or a deactivated account keeps receiving repo-update/file-lock events.
    pub async fn subscription_allowed(&self, username: &str, repo_id: &str) -> bool {
        let Ok(Some(user)) = self.repos.user.find_by_email(username).await else {
            return false;
        };
        if !user.is_active || !user.email_verified {
            return false;
        }
        crate::domain::permission::check_repo_read_permission(
            self.repos.member.as_ref(),
            repo_id,
            user.id,
        )
        .await
        .is_ok()
    }

    /// Replace the caps. Connected clients are not dropped; the next registration
    /// sees the new cap.
    pub fn set_connection_limits(&self, max_connections: u64, max_connections_per_ip: u64) {
        self.max_connections
            .store(max_connections, Ordering::Relaxed);
        self.max_connections_per_ip
            .store(max_connections_per_ip, Ordering::Relaxed);
    }

    /// Global cap on concurrent connections (0 = unlimited).
    pub fn max_connections(&self) -> u64 {
        self.max_connections.load(Ordering::Relaxed)
    }

    /// Cap on concurrent connections per client IP (0 = unlimited).
    pub fn max_connections_per_ip(&self) -> u64 {
        self.max_connections_per_ip.load(Ordering::Relaxed)
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

    /// Register a client; `None` when the global or per-IP cap is reached. Both
    /// caps are enforced atomically under the client-map write lock.
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
            // The bounded client map is scanned under the same write lock, so no
            // separate counter is needed; registration is infrequent.
            let same_ip = clients
                .values()
                .filter(|c| c.peer_ip == client.peer_ip)
                .count();
            let per_ip_cap = self.max_connections_per_ip.load(Ordering::Relaxed);
            let global_cap = self.max_connections.load(Ordering::Relaxed);
            let over_ip = per_ip_cap > 0 && same_ip as u64 >= per_ip_cap;
            let over_global = global_cap > 0 && clients.len() as u64 >= global_cap;
            if over_global || over_ip {
                return None;
            }
            clients.insert(id, client.clone());
        }

        Some((id, client))
    }

    /// Remove a client and all its subscriptions.
    pub async fn unregister_client(&self, client_id: u64) {
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

        // `remove` returns the client only if it was still present, so this is
        // idempotent: the read/write loops and socket teardown all call it.
        {
            let mut clients = self.write_clients();
            clients.remove(&client_id);
        }
    }

    /// Subscribe a client; `username` comes from the validated JWT, `repos` are
    /// (repo_id, jwt_exp) pairs.
    pub async fn subscribe(&self, client_id: u64, username: &str, repos: &[(String, i64)]) {
        let clients = self.read_clients();
        let client = match clients.get(&client_id) {
            Some(c) => c.clone(),
            None => return,
        };
        drop(clients);

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

    /// Notify all subscribers of a repo.
    ///
    /// The message is serialized once and the bytes shared; a client whose
    /// bounded channel is full is skipped rather than buffered without bound.
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

    /// Send a message directly to one client (e.g. `jwt-expired`). Non-blocking:
    /// a full bounded channel drops it, like [`notify_repo`](Self::notify_repo).
    pub async fn notify_client(&self, client_id: u64, message: &NotificationMessage) {
        let bytes: Arc<[u8]> = Arc::from(serde_json::to_vec(message).unwrap_or_default());
        let clients = self.read_clients();
        if let Some(client) = clients.get(&client_id) {
            let _ = client.sender.try_send(bytes);
        }
    }

    /// Notify the repo named by the event.
    pub async fn notify(&self, event: impl Into<NotificationMessage>) {
        let msg = event.into();
        if let Some(repo_id) = extract_repo_id(&msg) {
            self.notify_repo(&repo_id, &msg).await;
        }
    }
}

impl NotificationManager {
    /// Forward repo-update events from the global broadcast channel to WebSocket
    /// subscribers until `token` is cancelled. Does **not** spawn internally: the
    /// caller (typically `Scheduler`) owns the tokio task boundary.
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

    /// Single-shot check (called periodically by the `Scheduler`) that evicts
    /// expired subscriptions and sends `jwt-expired` to each affected client.
    ///
    /// Returns how many tokens expired, so the caller can report what it did.
    pub async fn check_expired_tokens(&self) -> u64 {
        let now = chrono::Utc::now().timestamp();
        let mut expired_total = 0u64;
        // Snapshot the clients and drop the lock before the membership queries:
        // holding a `std::sync::RwLockReadGuard` across an await would make this
        // future non-Send.
        let clients: Vec<(u64, Arc<ClientState>)> = self
            .read_clients()
            .iter()
            .map(|(id, client)| (*id, client.clone()))
            .collect();
        for (client_id, client) in &clients {
            let mut expired: Vec<String> = {
                let exps = client.read_token_expirations();
                exps.iter()
                    .filter(|&(_, exp)| *exp <= now)
                    .map(|(repo_id, _)| repo_id.clone())
                    .collect()
            };

            // Revalidate the remaining subscriptions as well: a JWT can be
            // signature-valid long after the member lost access to the library.
            let username = client.read_user().clone();
            if !username.is_empty() {
                let subscribed: Vec<String> =
                    client.read_subscribed_repos().iter().cloned().collect();
                for repo_id in subscribed {
                    if expired.contains(&repo_id) {
                        continue;
                    }
                    if !self.subscription_allowed(&username, &repo_id).await {
                        expired.push(repo_id);
                    }
                }
            }

            if expired.is_empty() {
                continue;
            }
            {
                let mut exps = client.write_token_expirations();
                for repo_id in &expired {
                    exps.remove(repo_id);
                }
            }
            {
                let mut subscribed = client.write_subscribed_repos();
                for repo_id in &expired {
                    subscribed.remove(repo_id);
                }
            }
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
            expired_total += expired.len() as u64;
        }
        expired_total
    }

    /// Clear all client and subscription state; dropping the mpsc senders makes
    /// each write task exit via `unregister_client` when `rx.recv()` returns `None`.
    pub async fn shutdown(&self) {
        tracing::info!("Shutting down notification manager");
        self.write_clients().clear();
        self.write_subscriptions().clear();
    }
}

/// Validate a subscription JWT against the derived subkey; the claims if valid, else `None`.
pub fn validate_notification_jwt(
    token: &str,
    private_key: &str,
    expected_repo_id: &str,
) -> Option<NotificationJwtClaims> {
    // Verify against the subscription subkey, so an event token (signed with a
    // different subkey) cannot be replayed as a subscription. The decoder requires
    // `exp`, rejects an `aud` claim and applies the same 60-second leeway the
    // previous jsonwebtoken configuration used.
    let sub_key = super::keys::subscription_key(private_key);
    let claims: NotificationJwtClaims = crate::service::auth::hs256::decode(token, &sub_key)?;

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
