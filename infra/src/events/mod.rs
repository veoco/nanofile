/// Global broadcast channel for repo HEAD updates: `create_commit` fires events and
/// the notification module subscribes, avoiding `AppState` in the storage layer.
use std::sync::LazyLock;
use tokio::sync::broadcast;

type RepoHeadUpdate = (String, String);

const CHANNEL_CAPACITY: usize = 1024;

/// Global sender; storage's `create_commit` publishes, notifications subscribe.
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
