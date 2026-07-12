use std::collections::HashMap;
use std::sync::RwLock;

use uuid::Uuid;

use tracing::debug;

const TOKEN_EXPIRE_SECS: i64 = 3600; // 1 hour, matching seafile

/// An access token granting permission to upload, update, or download a file.
#[derive(Clone, Debug)]
pub struct AccessToken {
    pub token: String,
    pub repo_id: String,
    pub user_id: i32,
    /// Email / username of the user.
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

/// In-memory web access token manager.
///
/// Generates tokens for the `/upload-api/{token}`, `/update-api/{token}`,
/// and `/download-api/{token}` endpoints.  Tokens are stored in memory
/// and expire after `TOKEN_EXPIRE_SECS` (3600 s).  Expired tokens are
/// cleaned up lazily on access.
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

    /// Generate a new token and store it.
    ///
    /// Returns the token string (a UUID without hyphens).
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

    /// Generate a download token with file metadata.
    ///
    /// Like `generate()` but also stores the file's fs_id and name so the
    /// download-api handler can return the correct Content-Disposition and oid header.
    pub fn generate_download(
        &self,
        repo_id: &str,
        user_id: i32,
        username: &str,
        parent_dir: &str,
        file_fs_id: &str,
        file_name: &str,
    ) -> String {
        let token = self.generate(repo_id, user_id, username, "download", parent_dir);
        if let Ok(mut guard) = self.tokens.write()
            && let Some(entry) = guard.get_mut(&token)
        {
            entry.file_fs_id = Some(file_fs_id.to_string());
            entry.file_name = Some(file_name.to_string());
        }
        token
    }

    /// Validate and return the token.  Returns `None` if the token
    /// doesn't exist or has expired.  Expired tokens are cleaned up
    /// during validation.
    pub fn validate(&self, token: &str) -> Option<AccessToken> {
        let now = chrono::Utc::now().timestamp();
        let mut guard = match self.tokens.write() {
            Ok(g) => g,
            Err(poisoned) => {
                eprintln!("[access_token] VALIDATE lock poisoned, recovering");
                poisoned.into_inner()
            }
        };

        // Clean up expired tokens lazily.
        guard.retain(|_, t| t.expires_at > now);

        debug!(size = guard.len(), "validating access token");
        let entry = guard.get(token)?.clone();
        if entry.expires_at > now {
            Some(entry)
        } else {
            None
        }
    }

    /// Set the upload_link_id on an existing token (used to link uploads back to the source link).
    pub fn set_upload_link_id(&self, token: &str, link_id: i32) {
        if let Ok(mut guard) = self.tokens.write()
            && let Some(entry) = guard.get_mut(token)
        {
            entry.upload_link_id = Some(link_id);
        }
    }

    /// Remove a token from the store (called after successful upload).
    pub fn remove(&self, token: &str) {
        if let Ok(mut guard) = self.tokens.write() {
            guard.remove(token);
        }
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
