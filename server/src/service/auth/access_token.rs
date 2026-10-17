use std::collections::HashMap;
use std::sync::RwLock;

use uuid::Uuid;

use tracing::debug;

const TOKEN_EXPIRE_SECS: i64 = 3600; // 1 hour, matching seafile

/// Above this size `validate` purges under the write lock; below it validation is read-lock only.
const CLEANUP_THRESHOLD: usize = 10_000;

/// An access token granting permission to upload, update, or download a file.
#[derive(Clone, Debug)]
pub struct AccessToken {
    pub token: String,
    pub repo_id: String,
    pub user_id: i32,
    pub username: String,
    /// "upload", "update", or "download"
    pub op: String,
    /// Parent directory for uploads (e.g. "/dir"), or file path for downloads.
    pub parent_dir: String,
    /// File fs_id (set for download tokens, used for the oid response header).
    pub file_fs_id: Option<String>,
    /// File name (set for download tokens, used for Content-Disposition).
    pub file_name: Option<String>,
    /// Upload link database ID (set for upload-link tokens, used to count uploads).
    pub upload_link_id: Option<i32>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// In-memory manager for the `/upload-api/{token}`, `/update-api/{token}` and
/// `/download-api/{token}` bearer tokens; they expire after `TOKEN_EXPIRE_SECS`.
pub struct AccessTokenManager {
    tokens: RwLock<HashMap<String, AccessToken>>,
}

impl Default for AccessTokenManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AccessTokenManager {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Generate a new token and store it; returns a UUID without hyphens.
    pub fn generate(
        &self,
        repo_id: &str,
        user_id: i32,
        username: &str,
        op: &str,
        parent_dir: &str,
    ) -> String {
        let now = chrono::Utc::now().timestamp();
        let token = Uuid::new_v4().to_string().replace('-', "");

        let entry = AccessToken {
            token: token.clone(),
            repo_id: repo_id.to_owned(),
            user_id,
            username: username.to_owned(),
            op: op.to_owned(),
            parent_dir: parent_dir.to_owned(),
            file_fs_id: None,
            file_name: None,
            upload_link_id: None,
            created_at: now,
            expires_at: now + TOKEN_EXPIRE_SECS,
        };

        match self.tokens.write() {
            Ok(mut guard) => {
                let prev_len = guard.len();
                guard.insert(token.clone(), entry);
                debug!(repo_id = %repo_id, prev_len, new_len = guard.len(), "access token stored");
            }
            Err(poisoned) => {
                eprintln!("[access_token] WRITE LOCK POISONED! recovering...");
                let mut guard = poisoned.into_inner();
                guard.insert(token.clone(), entry);
                debug!(repo_id = %repo_id, "access token stored (after poison recovery)");
            }
        }

        token
    }

    /// Like [`Self::generate`], but also stores the fs_id and name the download
    /// handler needs for the `oid` and `Content-Disposition` headers.
    pub fn generate_download(
        &self,
        repo_id: &str,
        user_id: i32,
        username: &str,
        parent_dir: &str,
        file_fs_id: &str,
        file_name: &str,
    ) -> String {
        let now = chrono::Utc::now().timestamp();
        let token = Uuid::new_v4().to_string().replace('-', "");

        let entry = AccessToken {
            token: token.clone(),
            repo_id: repo_id.to_owned(),
            user_id,
            username: username.to_owned(),
            op: "download".to_owned(),
            parent_dir: parent_dir.to_owned(),
            file_fs_id: Some(file_fs_id.to_string()),
            file_name: Some(file_name.to_string()),
            upload_link_id: None,
            created_at: now,
            expires_at: now + TOKEN_EXPIRE_SECS,
        };

        match self.tokens.write() {
            Ok(mut guard) => {
                guard.insert(token.clone(), entry);
                debug!(repo_id = %repo_id, len = guard.len(), "download token stored");
            }
            Err(poisoned) => {
                eprintln!("[access_token] WRITE LOCK POISONED! recovering...");
                poisoned.into_inner().insert(token.clone(), entry);
            }
        }

        token
    }

    /// Validate and return a token, or `None` when it is missing or expired.
    /// Expired entries are purged lazily only above `CLEANUP_THRESHOLD`, so the
    /// common valid-token path takes just a read lock.
    pub fn validate(&self, token: &str) -> Option<AccessToken> {
        let now = chrono::Utc::now().timestamp();

        if let Ok(guard) = self.tokens.read()
            && let Some(entry) = guard.get(token)
            && entry.expires_at > now
        {
            return Some(entry.clone());
        }

        // Slow path: missing or expired token; purge once the map has grown large.
        let mut guard = match self.tokens.write() {
            Ok(g) => g,
            Err(poisoned) => {
                eprintln!("[access_token] VALIDATE lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        if guard.len() > CLEANUP_THRESHOLD {
            guard.retain(|_, t| t.expires_at > now);
        }

        debug!(size = guard.len(), "validating access token");
        let entry = guard.get(token)?;
        if entry.expires_at > now {
            Some(entry.clone())
        } else {
            None
        }
    }

    pub fn set_upload_link_id(&self, token: &str, link_id: i32) {
        if let Ok(mut guard) = self.tokens.write()
            && let Some(entry) = guard.get_mut(token)
        {
            entry.upload_link_id = Some(link_id);
        }
    }

    pub fn remove(&self, token: &str) {
        if let Ok(mut guard) = self.tokens.write() {
            guard.remove(token);
        }
    }

    /// Drop every outstanding token belonging to `user_id`.
    ///
    /// A leaked one-hour capability URL must not keep working after a password
    /// change, reset, deactivation or logout; callers revoke on those events.
    pub fn revoke_user(&self, user_id: i32) -> usize {
        let mut guard = match self.tokens.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let before = guard.len();
        guard.retain(|_, t| t.user_id != user_id);
        before - guard.len()
    }

    /// Drop `user_id`'s tokens for one repository (unshare, deleted library): the
    /// token was minted while they were a member, so it must not outlive it.
    pub fn revoke_user_repo(&self, user_id: i32, repo_id: &str) -> usize {
        let mut guard = match self.tokens.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let before = guard.len();
        guard.retain(|_, t| !(t.user_id == user_id && t.repo_id == repo_id));
        before - guard.len()
    }

    /// Drop the tokens minted from upload link `link_id`; deleting the link must
    /// also stop uploaders that already exchanged it for a token.
    pub fn revoke_upload_link(&self, link_id: i32) -> usize {
        let mut guard = match self.tokens.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let before = guard.len();
        guard.retain(|_, t| t.upload_link_id != Some(link_id));
        before - guard.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_validate() {
        let mgr = AccessTokenManager::new();
        let token = mgr.generate("repo1", 1, "user@test.com", "upload", "/dir");
        assert!(!token.is_empty());
        let result = mgr.validate(&token);
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.repo_id, "repo1");
        assert_eq!(info.user_id, 1);
        assert_eq!(info.username, "user@test.com");
        assert_eq!(info.op, "upload");
        assert_eq!(info.parent_dir, "/dir");
    }

    #[test]
    fn test_invalid_token() {
        let mgr = AccessTokenManager::new();
        assert!(mgr.validate("nonexistent").is_none());
    }

    #[test]
    fn test_remove() {
        let mgr = AccessTokenManager::new();
        let token = mgr.generate("repo1", 1, "u@t.com", "upload", "/");
        mgr.remove(&token);
        assert!(mgr.validate(&token).is_none());
    }

    #[test]
    fn test_cleanup_expired() {
        let mgr = AccessTokenManager::new();
        let token = mgr.generate("repo1", 1, "u@t.com", "upload", "/");
        if let Ok(mut guard) = mgr.tokens.write()
            && let Some(entry) = guard.get_mut(&token)
        {
            entry.expires_at = 0;
        }
        assert!(mgr.validate(&token).is_none());
    }
}
