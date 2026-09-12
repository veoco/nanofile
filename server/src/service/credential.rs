//! The account's credential inventory: everything long-lived it holds.
//!
//! A user can hold four kinds of credential, and before this service only two
//! of them could be listed (`api_keys` had a page, client sessions had the
//! devices page). Sync tokens — one row per library per device, 365 days by
//! default — and 90-day 2FA device trusts had no read path at all, so the
//! owner could neither see them nor revoke one.
//!
//! The views below are deliberate: they carry metadata only. A `sync_tokens`
//! row holds a ciphertext of a bearer credential that this server can decrypt,
//! so serialising the model would hand the inventory the same power as the token
//! itself. Nothing here touches that column.

use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::session_source::SessionSource;
use crate::repository::Repositories;
use crate::ui::user_agent;
use base::error::AppError;

/// Which list a revocation addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    /// A browser session, by `api_tokens` row id.
    BrowserSession,
    /// A repository sync token, by `sync_tokens` row id.
    SyncToken,
    /// A 2FA device trust, by `s2fa_tokens` row id.
    DeviceTrust,
}

impl CredentialKind {
    /// Parse the form value the page sends back.
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "browser_session" => Some(CredentialKind::BrowserSession),
            "sync_token" => Some(CredentialKind::SyncToken),
            "device_trust" => Some(CredentialKind::DeviceTrust),
            _ => None,
        }
    }

    /// Revoking a *client* is addressed by device, not by row id, because one
    /// device owns several credentials (its session, its sync tokens and its
    /// 2FA trust) that all have to go together. It keeps its own route.
    pub const fn id(self) -> &'static str {
        match self {
            CredentialKind::BrowserSession => "browser_session",
            CredentialKind::SyncToken => "sync_token",
            CredentialKind::DeviceTrust => "device_trust",
        }
    }
}

/// A client application's session, grouped by device.
#[derive(Debug, Clone)]
pub struct ClientSessionView {
    pub platform: String,
    pub platform_display: String,
    pub device_id: String,
    pub device_name: String,
    pub client_version: String,
    pub created_ts: i64,
    pub is_desktop_client: bool,
}

/// A session a browser holds.
#[derive(Debug, Clone)]
pub struct BrowserSessionView {
    pub id: i32,
    /// The login this session came from, already translated for display by the
    /// page from [`BrowserSessionView::source`].
    pub source: SessionSource,
    /// What the browser looked like, e.g. `Chrome · macOS`. Empty when the
    /// login carried no usable `User-Agent`.
    pub client: String,
    pub created_ts: i64,
    /// Whether this is the session the reader is using.
    pub is_current: bool,
}

/// A repository sync token, shown without its value.
#[derive(Debug, Clone)]
pub struct SyncTokenView {
    pub id: i32,
    pub repo_id: String,
    /// `None` when the library no longer exists.
    pub repo_name: Option<String>,
    pub device_name: Option<String>,
    pub peer_ip: Option<String>,
    pub client_version: Option<String>,
    pub created_ts: i64,
    pub last_sync_ts: Option<i64>,
    pub expires_at: Option<i64>,
}

/// A device that may skip the second factor.
#[derive(Debug, Clone)]
pub struct DeviceTrustView {
    pub id: i32,
    pub device_name: Option<String>,
    pub device_id: Option<String>,
    pub created_ts: i64,
    pub expires_at: i64,
}

/// Every long-lived credential the account holds.
#[derive(Debug, Clone, Default)]
pub struct CredentialInventory {
    pub clients: Vec<ClientSessionView>,
    pub browsers: Vec<BrowserSessionView>,
    pub sync_tokens: Vec<SyncTokenView>,
    pub device_trusts: Vec<DeviceTrustView>,
    /// API keys are managed on their own page; the inventory only links there,
    /// so this is a count rather than a second implementation.
    pub api_key_count: usize,
}

impl CredentialInventory {
    /// Whether the account holds nothing but the session being used.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
            && self.browsers.is_empty()
            && self.sync_tokens.is_empty()
            && self.device_trusts.is_empty()
            && self.api_key_count == 0
    }
}

pub struct CredentialService {
    repos: Arc<Repositories>,
}

impl CredentialService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Everything `user_id` holds, ready to render.
    ///
    /// `current_session_id` is the row the reader is using, marked so they can
    /// tell it apart before revoking anything.
    pub async fn inventory(
        &self,
        user_id: i32,
        current_session_id: Option<i32>,
    ) -> Result<CredentialInventory, AppError> {
        let sessions = self.repos.api_token.list_sessions(user_id).await?;
        let tokens = self.repos.sync_token.list_for_user(user_id).await?;
        let trusts = self.repos.s2fa_token.list_for_user(user_id).await?;
        let key_count = self.repos.api_key.find_by_user(user_id).await?.len();

        // One query for every library named by a sync token, instead of one per
        // row: an account syncing twenty libraries would otherwise pay twenty
        // round trips to render one page.
        let repo_ids: Vec<String> = tokens.iter().map(|t| t.repo_id.clone()).collect();
        let repo_names: HashMap<String, String> = self
            .repos
            .repo
            .find_by_ids(&repo_ids)
            .await?
            .into_iter()
            .map(|repo| (repo.id, repo.name))
            .collect();

        let mut inventory = CredentialInventory {
            api_key_count: key_count,
            ..Default::default()
        };

        for session in sessions {
            // An unknown source is treated as a browser session: the safe
            // direction is to show it in the wrong group, because it stays
            // visible and revocable either way.
            let source = SessionSource::from_id(&session.source).unwrap_or(SessionSource::Web);
            if source.is_browser() {
                inventory.browsers.push(BrowserSessionView {
                    id: session.id,
                    source,
                    client: session
                        .user_agent
                        .as_deref()
                        .map(user_agent::describe)
                        .unwrap_or_default(),
                    created_ts: session.created_at,
                    is_current: current_session_id == Some(session.id),
                });
            } else {
                let platform = session.platform.clone().unwrap_or_default();
                let device_id = session.device_id.clone().unwrap_or_default();
                // One device owns one session row per login, so collapse the
                // repeats the way the devices page always has.
                if inventory
                    .clients
                    .iter()
                    .any(|c| c.platform == platform && c.device_id == device_id)
                {
                    continue;
                }
                inventory.clients.push(ClientSessionView {
                    platform_display: platform_display_name(&platform),
                    platform,
                    device_id,
                    device_name: session.device_name.unwrap_or_default(),
                    client_version: session.client_version.unwrap_or_default(),
                    created_ts: session.created_at,
                    is_desktop_client: is_desktop_platform(
                        session.platform.as_deref().unwrap_or_default(),
                    ),
                });
            }
        }

        inventory.sync_tokens = tokens
            .into_iter()
            .map(|token| SyncTokenView {
                repo_name: repo_names.get(&token.repo_id).cloned(),
                repo_id: token.repo_id,
                id: token.id,
                device_name: token.peer_name,
                peer_ip: token.peer_ip,
                client_version: token.client_version,
                created_ts: token.created_at,
                last_sync_ts: token.last_sync_time,
                expires_at: token.expires_at,
            })
            .collect();

        inventory.device_trusts = trusts
            .into_iter()
            .map(|trust| DeviceTrustView {
                id: trust.id,
                device_name: trust.device_name,
                device_id: trust.device_id,
                created_ts: trust.created_at,
                expires_at: trust.expires_at,
            })
            .collect();

        Ok(inventory)
    }

    /// Revoke one credential, scoped to its owner.
    ///
    /// Every branch passes `user_id`, so an id that reached the page from
    /// somewhere else cannot delete another account's row.
    pub async fn revoke(
        &self,
        user_id: i32,
        kind: CredentialKind,
        id: i32,
    ) -> Result<(), AppError> {
        let removed = match kind {
            CredentialKind::BrowserSession => {
                self.repos
                    .api_token
                    .delete_by_id_and_user(id, user_id)
                    .await?
            }
            CredentialKind::SyncToken => {
                self.repos
                    .sync_token
                    .delete_by_id_and_user(id, user_id)
                    .await?
            }
            CredentialKind::DeviceTrust => {
                self.repos
                    .s2fa_token
                    .delete_by_id_and_user(id, user_id)
                    .await?
            }
        };
        // Zero means the row was not this user's, or was already gone. Both are
        // "there is nothing here for you", and saying so is better than
        // reporting a success the caller cannot observe.
        if removed == 0 {
            return Err(AppError::NotFound("no such credential".into()));
        }
        Ok(())
    }
}

/// Whether the platform is a desktop client, which only changes the icon.
pub fn is_desktop_platform(platform: &str) -> bool {
    matches!(platform, "windows" | "linux" | "mac")
}

/// Human-readable platform name, matching the devices page.
pub fn platform_display_name(platform: &str) -> String {
    match platform {
        "windows" => "Windows".to_string(),
        "linux" => "Linux".to_string(),
        "mac" => "macOS".to_string(),
        "ios" => "iOS".to_string(),
        "android" => "Android".to_string(),
        "harmonyos" => "HarmonyOS".to_string(),
        "" => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_form_value() {
        for kind in [
            CredentialKind::BrowserSession,
            CredentialKind::SyncToken,
            CredentialKind::DeviceTrust,
        ] {
            assert_eq!(CredentialKind::from_id(kind.id()), Some(kind));
        }
        // A client session has no kind: it is revoked by device, not by id.
        assert_eq!(CredentialKind::from_id("client_session"), None);
    }

    #[test]
    fn platform_names_match_the_devices_page() {
        assert_eq!(platform_display_name("mac"), "macOS");
        assert_eq!(platform_display_name(""), "");
        assert_eq!(platform_display_name("plan9"), "plan9");
        assert!(is_desktop_platform("linux"));
        assert!(!is_desktop_platform("android"));
    }

    #[test]
    fn an_empty_inventory_says_so() {
        assert!(CredentialInventory::default().is_empty());
        let mut inventory = CredentialInventory {
            api_key_count: 1,
            ..Default::default()
        };
        assert!(!inventory.is_empty());
        inventory.api_key_count = 0;
        inventory.browsers.push(BrowserSessionView {
            id: 1,
            source: SessionSource::Web,
            client: String::new(),
            created_ts: 0,
            is_current: true,
        });
        assert!(!inventory.is_empty());
    }
}
