/// Global broadcast channel for repo-update events.
///
/// The `create_commit` function in the storage layer fires events through this channel,
/// and the notification module subscribes to it to forward events to WebSocket clients.
/// This avoids threading `AppState` through the storage layer.
use std::sync::LazyLock;
use tokio::sync::broadcast;

/// A (repo_id, commit_id) pair representing a repo update.
pub type RepoHeadUpdate = (String, String);

/// Maximum number of pending events in the broadcast channel.
const CHANNEL_CAPACITY: usize = 1024;

/// Global broadcast sender for repo HEAD updates.
///
/// The storage layer's `create_commit` sends events here.
/// The notification module subscribes via `subscribe_repo_updates()`.
static REPO_UPDATE_SENDER: LazyLock<broadcast::Sender<RepoHeadUpdate>> = LazyLock::new(|| {
    let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
    tx
});

/// Publish a repo update event to all subscribers.
pub fn publish_repo_update(repo_id: impl Into<String>, commit_id: impl Into<String>) {
    let _ = REPO_UPDATE_SENDER.send((repo_id.into(), commit_id.into()));
}

/// Subscribe to repo update events.
pub fn subscribe_repo_updates() -> broadcast::Receiver<RepoHeadUpdate> {
    REPO_UPDATE_SENDER.subscribe()
}
