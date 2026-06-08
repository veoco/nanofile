use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level message format for the notification protocol.
/// Used both for client→server messages (subscribe/unsubscribe)
/// and server→client messages (event notifications).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub content: Value,
}

/// A repo update event — fired when a repo's HEAD commit changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoUpdateEvent {
    pub repo_id: String,
    pub commit_id: String,
}

/// A file lock changed event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLockEvent {
    pub repo_id: String,
    pub path: String,
    pub change_event: String,
    pub lock_user: String,
}

/// A folder permission changed event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderPermEvent {
    pub repo_id: String,
    pub path: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub change_event: String,
    pub user: String,
    pub group: i32,
    pub perm: String,
}

/// A comment update event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentEvent {
    pub repo_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub file_uuid: String,
    pub file_path: String,
}

/// Subscribe request — client sends repos it wants notifications for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub repos: Vec<RepoSubscription>,
}

/// Unsubscribe request — client removes repos from its subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeRequest {
    pub repos: Vec<RepoUnsubscription>,
}

/// A single repo subscription with JWT token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoSubscription {
    pub id: String,
    pub jwt_token: String,
}

/// A single repo unsubscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoUnsubscription {
    pub id: String,
}

/// JWT claims embedded in the notification subscription token.
/// Seafile format: { repo_id, username, exp }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationJwtClaims {
    pub repo_id: String,
    pub username: String,
    pub exp: i64,
}

impl RepoUpdateEvent {
    pub fn new(repo_id: impl Into<String>, commit_id: impl Into<String>) -> Self {
        Self {
            repo_id: repo_id.into(),
            commit_id: commit_id.into(),
        }
    }
}

impl From<RepoUpdateEvent> for NotificationMessage {
    fn from(event: RepoUpdateEvent) -> Self {
        NotificationMessage {
            msg_type: "repo-update".to_string(),
            content: serde_json::to_value(event).unwrap_or_default(),
        }
    }
}

impl From<FileLockEvent> for NotificationMessage {
    fn from(event: FileLockEvent) -> Self {
        NotificationMessage {
            msg_type: "file-lock-changed".to_string(),
            content: serde_json::to_value(event).unwrap_or_default(),
        }
    }
}

impl From<FolderPermEvent> for NotificationMessage {
    fn from(event: FolderPermEvent) -> Self {
        NotificationMessage {
            msg_type: "folder-perm-changed".to_string(),
            content: serde_json::to_value(event).unwrap_or_default(),
        }
    }
}

impl From<CommentEvent> for NotificationMessage {
    fn from(event: CommentEvent) -> Self {
        NotificationMessage {
            msg_type: "comment-update".to_string(),
            content: serde_json::to_value(event).unwrap_or_default(),
        }
    }
}
