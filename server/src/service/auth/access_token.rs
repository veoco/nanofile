use std::collections::HashMap;
use std::sync::RwLock;

use uuid::Uuid;

use tracing::debug;

const TOKEN_EXPIRE_SECS: i64 = 3600; // 1 hour, matching seafile

/// Cap on live capability tokens. Minting is a side effect of ordinary
/// authenticated GETs (`upload-link`, `download-link`, block links), so an
/// account can create tokens as fast as it can make requests; without a cap the
/// table would grow with request count rather than with active uploads. At the
/// cap, expired entries are dropped first and a still-full table is cleared:
/// outstanding one-hour URLs stop working, which is the same fail-closed answer
/// the token cache already gives and costs a client one re-fetch.
const MAX_TOKENS: usize = 100_000;

/// Make room for one more token at [`MAX_TOKENS`]: drop expired entries first,
/// and clear the table if a live flood still fills it (fail-closed: the next
/// request mints a new token).
fn make_room(tokens: &mut HashMap<String, AccessToken>) {
    if tokens.len() < MAX_TOKENS {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    tokens.retain(|_, token| token.expires_at > now);
    if tokens.len() >= MAX_TOKENS {
        tracing::warn!(
            "capability token table reached its cap ({MAX_TOKENS}); clearing it so \
             outstanding capability URLs are invalidated instead of unbounded growth"
        );
        tokens.clear();
    }
}

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
    /// Expiry of the upload link this token was minted from, when it is a link
    /// token. The token itself lives one hour, but a link can expire sooner;
    /// carrying the link's own deadline makes that effective immediately instead
    /// of only at the next mint.
    pub upload_link_expires_at: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// In-memory manager for the `/upload-api/{token}`, `/update-api/{token}` and
/// `/api/transfers/download/{token}` bearer tokens; they expire after
/// `TOKEN_EXPIRE_SECS`.
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
            upload_link_expires_at: None,
            created_at: now,
            expires_at: now + TOKEN_EXPIRE_SECS,
        };

        match self.tokens.write() {
            Ok(mut guard) => {
                make_room(&mut guard);
                let prev_len = guard.len();
                guard.insert(token.clone(), entry);
                debug!(repo_id = %repo_id, prev_len, new_len = guard.len(), "access token stored");
            }
            Err(poisoned) => {
                eprintln!("[access_token] WRITE LOCK POISONED! recovering...");
                let mut guard = poisoned.into_inner();
                make_room(&mut guard);
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
            upload_link_expires_at: None,
            created_at: now,
            expires_at: now + TOKEN_EXPIRE_SECS,
        };

        match self.tokens.write() {
            Ok(mut guard) => {
                make_room(&mut guard);
                guard.insert(token.clone(), entry);
                debug!(repo_id = %repo_id, len = guard.len(), "download token stored");
            }
            Err(poisoned) => {
                eprintln!("[access_token] WRITE LOCK POISONED! recovering...");
                let mut guard = poisoned.into_inner();
                make_room(&mut guard);
                guard.insert(token.clone(), entry);
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

    /// Bind an upload token to its upload link. Kept for callers that only know
    /// the link id; [`Self::set_upload_link`] additionally carries the link's
    /// expiry.
    pub fn set_upload_link_id(&self, token: &str, link_id: i32) {
        self.set_upload_link(token, link_id, None);
    }

    /// Bind a block-download token to the file whose blocks it may serve.
    ///
    /// Seafile's block URL names only the token and the block id, so the file id
    /// cannot travel in the path: the fileserver keeps it in the access token
    /// instead and checks the block belongs to that file.
    pub fn set_block_file(&self, token: &str, file_fs_id: &str) {
        if let Ok(mut guard) = self.tokens.write()
            && let Some(entry) = guard.get_mut(token)
        {
            entry.file_fs_id = Some(file_fs_id.to_owned());
        }
    }

    /// Bind an upload token to the link it was minted from and carry the link's
    /// own expiry. `None` keeps the token's one-hour lifetime.
    pub fn set_upload_link(&self, token: &str, link_id: i32, expires_at: Option<i64>) {
        if let Ok(mut guard) = self.tokens.write()
            && let Some(entry) = guard.get_mut(token)
        {
            entry.upload_link_id = Some(link_id);
            entry.upload_link_expires_at = expires_at;
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

    /// Minting is reachable from ordinary authenticated GETs, so the map must not
    /// grow with request count. At the cap, expired entries are dropped before a
    /// new token is inserted.
    #[test]
    fn the_token_table_drops_expired_entries_at_its_cap() {
        let mgr = AccessTokenManager::new();
        {
            let mut guard = mgr.tokens.write().unwrap();
            for i in 0..MAX_TOKENS {
                guard.insert(
                    format!("stale-{i}"),
                    AccessToken {
                        token: format!("stale-{i}"),
                        repo_id: "repo".into(),
                        user_id: 1,
                        username: "u@t".into(),
                        op: "upload".into(),
                        parent_dir: "/".into(),
                        file_fs_id: None,
                        file_name: None,
                        upload_link_id: None,
                        upload_link_expires_at: None,
                        created_at: 0,
                        expires_at: 0,
                    },
                );
            }
        }
        let fresh = mgr.generate("repo1", 1, "u@t.com", "upload", "/");
        let guard = mgr.tokens.read().unwrap();
        assert_eq!(
            guard.len(),
            1,
            "the expired entries must be swept instead of growing the table"
        );
        assert!(guard.contains_key(&fresh));
    }
}
