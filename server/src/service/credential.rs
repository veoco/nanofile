//! The account's credential inventory: everything long-lived it holds.
//!
//! Four kinds: browser sessions, client device sessions, repository sync tokens
//! and 2FA device trusts. The views carry metadata only: a `sync_tokens` row
//! holds a ciphertext of a bearer credential this server can decrypt, so
//! serialising the model would hand the inventory the same power as the token
//! itself. Nothing here touches that column.

use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::permission::RepoScope;
use crate::domain::session_source::SessionSource;
use crate::repository::Repositories;
use crate::ui::user_agent;
use base::error::AppError;

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
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "browser_session" => Some(CredentialKind::BrowserSession),
            "sync_token" => Some(CredentialKind::SyncToken),
            "device_trust" => Some(CredentialKind::DeviceTrust),
            _ => None,
        }
    }

    /// Revoking a *client* is addressed by device, not by row id, because one
    /// device owns several credentials that all have to go together.
    pub const fn id(self) -> &'static str {
        match self {
            CredentialKind::BrowserSession => "browser_session",
            CredentialKind::SyncToken => "sync_token",
            CredentialKind::DeviceTrust => "device_trust",
        }
    }
}

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

#[derive(Debug, Clone)]
pub struct BrowserSessionView {
    pub id: i32,
    /// The login this session came from, translated for display by the page.
    pub source: SessionSource,
    /// What the browser looked like, e.g. `Chrome · macOS`; empty without a
    /// usable `User-Agent`.
    pub client: String,
    pub created_ts: i64,
    pub is_current: bool,
}

impl BrowserSessionView {
    /// Whether this session is one the owner could sign out from here.
    pub fn is_other(&self) -> bool {
        !self.is_current
    }
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
    /// The client identifier the token was minted for. This is what links a token
    /// to a device, because a device name is display text two machines can share.
    pub peer_id: Option<String>,
}

impl SyncTokenView {
    /// Whether the token has already expired.
    ///
    /// A missing `expires_at` means "never", which is the opposite of expired.
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|expires| expires <= now)
    }

    /// Whether the token is close enough to expiry to be worth warning about.
    pub fn expires_soon(&self, now: i64) -> bool {
        self.expires_at
            .is_some_and(|expires| expires > now && expires - now <= EXPIRING_SOON_SECS)
    }

    pub fn is_stale(&self, now: i64) -> bool {
        match self.last_sync_ts {
            None => self.created_ts <= now - STALE_SECS,
            Some(last) => last <= now - STALE_SECS,
        }
    }
}

const STALE_SECS: i64 = 30 * 24 * 60 * 60;

const EXPIRING_SOON_SECS: i64 = 7 * 24 * 60 * 60;

/// A device a client holds, with everything else that device owns.
///
/// The page is organised around devices, not credential kinds, because "what can
/// this machine still do to my account?" is the owner's question. A device is
/// keyed by `platform` + `device_id` — identifiers a client reports — never by
/// its name, which is display text two machines can legitimately share. A
/// library has one sync token per device; a token whose device has no session is
/// listed separately by the page.
#[derive(Debug, Clone)]
pub struct DeviceGroup {
    pub platform: String,
    pub platform_display: String,
    pub device_id: String,
    pub device_name: String,
    pub client_version: String,
    pub created_ts: i64,
    pub is_desktop_client: bool,
    /// Sync tokens whose `peer_id` is this device's `device_id`.
    pub sync_tokens: Vec<SyncTokenView>,
    /// The 2FA trusts whose `device_id` is this device's `device_id`. Usually one,
    /// but a row left by an older build still belongs to this device rather than
    /// to the page's "unknown device" section.
    pub device_trusts: Vec<DeviceTrustView>,
}

#[derive(Debug, Clone)]
pub struct DeviceTrustView {
    pub id: i32,
    pub device_name: Option<String>,
    pub device_id: Option<String>,
    pub created_ts: i64,
    pub expires_at: i64,
}

/// The at-a-glance state of the inventory, for the summary strip: counts, plus
/// the two flags an owner can act on (two-factor on at all, and how many sync
/// tokens are expired or have gone quiet).
#[derive(Debug, Clone, Default)]
pub struct CredentialSummary {
    pub browser_sessions: usize,
    pub client_devices: usize,
    pub sync_tokens: usize,
    pub api_keys: usize,
    pub device_trusts: usize,
    pub expired_sync_tokens: usize,
    pub stale_sync_tokens: usize,
    /// Whether the account has two-factor enabled. Filled in by the page, not here:
    /// this service deliberately reads no 2FA state.
    pub two_fa_enabled: bool,
}

impl CredentialSummary {
    pub fn has_warning(&self) -> bool {
        !self.two_fa_enabled || self.expired_sync_tokens > 0 || self.stale_sync_tokens > 0
    }
}

/// Every long-lived credential the account holds.
#[derive(Debug, Clone, Default)]
pub struct CredentialInventory {
    pub clients: Vec<ClientSessionView>,
    pub browsers: Vec<BrowserSessionView>,
    pub sync_tokens: Vec<SyncTokenView>,
    pub device_trusts: Vec<DeviceTrustView>,
    /// One entry per client device, carrying the sync tokens and 2FA trust that
    /// belong to it. Unmatched tokens stay out and are listed separately.
    pub devices: Vec<DeviceGroup>,
    /// API keys are managed on their own page; this is a count, not a second
    /// implementation.
    pub api_key_count: usize,
    /// The summary strip's numbers, computed here so the page does not re-derive
    /// the same rules.
    pub summary: CredentialSummary,
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

    /// How many other browser sessions a bulk revoke would remove; zero means the
    /// action is not worth showing.
    pub fn other_browser_sessions(&self) -> usize {
        self.browsers.iter().filter(|b| b.is_other()).count()
    }
}

/// Which set of credentials a bulk revoke addresses.
///
/// There is deliberately no "every sync token": the page lists sync tokens grouped
/// by owning device, so such a button would look like it belonged to the list
/// underneath it while doing far more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkRevoke {
    /// Every browser session except the one asking.
    OtherBrowserSessions,
}

impl BulkRevoke {
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "sign_out_others" => Some(BulkRevoke::OtherBrowserSessions),
            _ => None,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            BulkRevoke::OtherBrowserSessions => "sign_out_others",
        }
    }
}

pub struct CredentialService {
    repos: Arc<Repositories>,
}

impl CredentialService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Everything `user_id` holds, ready to render. `current_session_id` is the row
    /// the reader is using, marked so they can tell it apart before revoking.
    pub async fn inventory(
        &self,
        user_id: i32,
        current_session_id: Option<i32>,
        scope: &RepoScope,
    ) -> Result<CredentialInventory, AppError> {
        let sessions = self.repos.api_token.list_sessions(user_id).await?;
        // A key bound to one library must not learn the metadata of sync tokens
        // for the account's other libraries.
        let tokens: Vec<_> = self
            .repos
            .sync_token
            .list_for_user(user_id)
            .await?
            .into_iter()
            .filter(|token| scope.allows(&token.repo_id))
            .collect();
        let trusts = self.repos.s2fa_token.list_for_user(user_id).await?;
        let key_count = self.repos.api_key.find_by_user(user_id).await?.len();

        // One query for every library named by a sync token, not one per row.
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
            // An unknown source is treated as a browser session: showing it in the
            // wrong group still leaves it visible and revocable.
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
                // One device owns one session row per login; collapse the repeats.
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
                peer_id: token.peer_id,
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

        let now = chrono::Utc::now().timestamp();
        inventory.devices = group_devices(
            &inventory.clients,
            &inventory.sync_tokens,
            &inventory.device_trusts,
        );
        inventory.summary = summarize(&inventory, now);

        Ok(inventory)
    }

    /// Revoke one credential, scoped to its owner.
    ///
    /// Every branch passes `user_id`, so an id from somewhere else cannot delete
    /// another account's row.
    pub async fn revoke(
        &self,
        user_id: i32,
        kind: CredentialKind,
        id: i32,
        scope: &RepoScope,
    ) -> Result<(), AppError> {
        let removed = match kind {
            CredentialKind::BrowserSession => {
                self.repos
                    .api_token
                    .delete_by_id_and_user(id, user_id)
                    .await?
            }
            CredentialKind::SyncToken => {
                let token = self
                    .repos
                    .sync_token
                    .list_for_user(user_id)
                    .await?
                    .into_iter()
                    .find(|token| token.id == id)
                    .ok_or_else(|| AppError::NotFound("no such credential".into()))?;
                if !scope.allows(&token.repo_id) {
                    return Err(AppError::NotFound("no such credential".into()));
                }
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
        // Zero means the row was not this user's, or was already gone: both are
        // "there is nothing here for you", not a success the caller cannot observe.
        if removed == 0 {
            return Err(AppError::NotFound("no such credential".into()));
        }
        Ok(())
    }

    /// Revoke every credential in a set, scoped to its owner.
    ///
    /// `keep_session_id` is decided by the caller from the authenticated session,
    /// never from a form field: signing yourself out by accident is the one outcome
    /// this action must not have.
    ///
    /// Returns how many rows were removed; zero is not an error, because "there was
    /// nothing left to revoke" is a state the page can report.
    pub async fn revoke_bulk(
        &self,
        user_id: i32,
        what: BulkRevoke,
        keep_session_id: Option<i32>,
    ) -> Result<u64, AppError> {
        match what {
            BulkRevoke::OtherBrowserSessions => {
                self.repos
                    .api_token
                    .delete_browser_sessions_except(user_id, keep_session_id)
                    .await
            }
        }
    }
}

/// Group credentials by the device that holds them.
///
/// A device is identified by `platform` + `device_id`, the two values a client
/// reports and cannot accidentally share; its name is display text only, and
/// merging two "laptop"s would hide one device's credentials behind the other's.
fn group_devices(
    clients: &[ClientSessionView],
    tokens: &[SyncTokenView],
    trusts: &[DeviceTrustView],
) -> Vec<DeviceGroup> {
    clients
        .iter()
        .map(|client| DeviceGroup {
            platform: client.platform.clone(),
            platform_display: client.platform_display.clone(),
            device_id: client.device_id.clone(),
            device_name: client.device_name.clone(),
            client_version: client.client_version.clone(),
            created_ts: client.created_ts,
            is_desktop_client: client.is_desktop_client,
            sync_tokens: tokens
                .iter()
                .filter(|t| {
                    !client.device_id.is_empty()
                        && t.peer_id.as_deref() == Some(client.device_id.as_str())
                })
                .cloned()
                .collect(),
            device_trusts: trusts
                .iter()
                .filter(|t| {
                    !client.device_id.is_empty()
                        && t.device_id.as_deref() == Some(client.device_id.as_str())
                })
                .cloned()
                .collect(),
        })
        .collect()
}

fn summarize(inventory: &CredentialInventory, now: i64) -> CredentialSummary {
    CredentialSummary {
        browser_sessions: inventory.browsers.len(),
        client_devices: inventory.clients.len(),
        sync_tokens: inventory.sync_tokens.len(),
        api_keys: inventory.api_key_count,
        device_trusts: inventory.device_trusts.len(),
        expired_sync_tokens: inventory
            .sync_tokens
            .iter()
            .filter(|t| t.is_expired(now))
            .count(),
        stale_sync_tokens: inventory
            .sync_tokens
            .iter()
            .filter(|t| t.is_stale(now) && !t.is_expired(now))
            .count(),
        two_fa_enabled: false,
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
        assert_eq!(
            inventory.other_browser_sessions(),
            0,
            "the only session is ours"
        );
    }

    #[test]
    fn bulk_kinds_round_trip_through_their_form_value() {
        let what = BulkRevoke::OtherBrowserSessions;
        assert_eq!(BulkRevoke::from_id(what.id()), Some(what));
        assert_eq!(BulkRevoke::from_id("delete_everything"), None);
        // The retiree: a form that still posts the old action must not be accepted,
        // because that button deleted every token, not only the unowned ones listed
        // under it.
        assert_eq!(BulkRevoke::from_id("revoke_sync_tokens"), None);
    }

    fn client(platform: &str, device_id: &str, name: &str) -> ClientSessionView {
        ClientSessionView {
            platform: platform.to_string(),
            platform_display: platform_display_name(platform),
            device_id: device_id.to_string(),
            device_name: name.to_string(),
            client_version: "3.0.4".to_string(),
            created_ts: 1_000,
            is_desktop_client: is_desktop_platform(platform),
        }
    }

    fn token(
        id: i32,
        peer_id: Option<&str>,
        last_sync: Option<i64>,
        expires: Option<i64>,
    ) -> SyncTokenView {
        SyncTokenView {
            id,
            repo_id: "r1".to_string(),
            repo_name: Some("repo".to_string()),
            device_name: Some("laptop".to_string()),
            peer_ip: None,
            client_version: None,
            created_ts: 1_000,
            last_sync_ts: last_sync,
            expires_at: expires,
            peer_id: peer_id.map(str::to_string),
        }
    }

    #[test]
    fn a_device_owns_only_the_tokens_that_carry_its_id() {
        let devices = group_devices(
            &[
                client("mac", "dev-a", "laptop"),
                client("linux", "dev-b", "laptop"),
            ],
            &[
                token(1, Some("dev-a"), Some(2_000), None),
                token(2, Some("dev-b"), Some(2_000), None),
                token(3, None, Some(2_000), None),
            ],
            &[],
        );

        assert_eq!(devices.len(), 2, "two devices, both named laptop");
        assert_eq!(devices[0].sync_tokens.len(), 1);
        assert_eq!(devices[0].sync_tokens[0].id, 1);
        assert_eq!(devices[1].sync_tokens[0].id, 2);
        // A token with no peer id belongs to no device and is shown on its own.
        assert!(
            devices
                .iter()
                .all(|d| d.sync_tokens.iter().all(|t| t.id != 3)),
            "an unowned token is not attached to either device"
        );
    }

    #[test]
    fn a_device_without_an_id_owns_nothing() {
        let devices = group_devices(
            &[client("", "", "")],
            &[token(1, Some(""), Some(2_000), None)],
            &[],
        );
        assert_eq!(devices.len(), 1);
        assert!(
            devices[0].sync_tokens.is_empty(),
            "an empty device id must not match every token with no peer id"
        );
    }

    #[test]
    fn a_device_trust_is_matched_only_by_id() {
        let trust = DeviceTrustView {
            id: 9,
            device_name: Some("laptop".to_string()),
            device_id: Some("dev-a".to_string()),
            created_ts: 1_000,
            expires_at: i64::MAX,
        };
        let devices = group_devices(
            &[
                client("mac", "dev-a", "laptop"),
                client("linux", "dev-b", "laptop"),
            ],
            &[],
            std::slice::from_ref(&trust),
        );
        assert_eq!(
            devices[0]
                .device_trusts
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![9]
        );
        assert!(
            devices[1].device_trusts.is_empty(),
            "names do not match devices"
        );
    }

    /// A row minted by an older build, which added a trust per login instead of
    /// replacing the device's, must still be shown under its device; otherwise it
    /// reaches the "unknown device" section, where a known device must never appear.
    #[test]
    fn every_trust_of_a_device_is_attached_to_it() {
        let trust = |id: i32| DeviceTrustView {
            id,
            device_name: Some("laptop".to_string()),
            device_id: Some("dev-a".to_string()),
            created_ts: 1_000,
            expires_at: i64::MAX,
        };
        let devices = group_devices(
            &[client("mac", "dev-a", "laptop")],
            &[],
            &[trust(1), trust(2)],
        );

        assert_eq!(
            devices[0]
                .device_trusts
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![1, 2],
            "both rows belong to the device, not to the unattached list"
        );
    }

    #[test]
    fn expiry_and_staleness_have_the_expected_boundaries() {
        let now = 10 * STALE_SECS;
        // No expiry means never, which is the opposite of expired.
        let forever = token(1, None, Some(now), None);
        assert!(!forever.is_expired(now));
        assert!(!forever.expires_soon(now));

        let expired = token(2, None, Some(now), Some(now - 1));
        assert!(expired.is_expired(now));
        assert!(!expired.expires_soon(now));

        let soon = token(3, None, Some(now), Some(now + 60));
        assert!(!soon.is_expired(now));
        assert!(soon.expires_soon(now));

        let later = token(4, None, Some(now), Some(now + EXPIRING_SOON_SECS + 1));
        assert!(!later.expires_soon(now));

        // Never synced: stale once the token itself is old enough (`token()` mints
        // at t=1000, older than any threshold below).
        assert!(token(5, None, None, None).is_stale(now));
        assert!(!token(6, None, Some(now - 60), None).is_stale(now));
        assert!(token(7, None, Some(now - STALE_SECS - 1), None).is_stale(now));
    }

    #[test]
    fn the_summary_counts_what_the_page_shows() {
        let now = 1_000_000;
        let mut inventory = CredentialInventory {
            api_key_count: 2,
            clients: vec![client("mac", "dev-a", "laptop")],
            browsers: vec![
                BrowserSessionView {
                    id: 1,
                    source: SessionSource::Web,
                    client: "Chrome · macOS".to_string(),
                    created_ts: now,
                    is_current: true,
                },
                BrowserSessionView {
                    id: 2,
                    source: SessionSource::Web,
                    client: "Firefox · Windows".to_string(),
                    created_ts: now,
                    is_current: false,
                },
            ],
            sync_tokens: vec![
                token(1, Some("dev-a"), Some(now), Some(now - 1)),
                token(2, Some("dev-a"), Some(now - STALE_SECS - 1), None),
                token(3, Some("dev-a"), Some(now), Some(now + 10 * 24 * 3600)),
            ],
            device_trusts: Vec::new(),
            ..Default::default()
        };
        inventory.devices = group_devices(
            &inventory.clients,
            &inventory.sync_tokens,
            &inventory.device_trusts,
        );
        inventory.summary = summarize(&inventory, now);

        assert_eq!(inventory.summary.browser_sessions, 2);
        assert_eq!(inventory.summary.client_devices, 1);
        assert_eq!(inventory.summary.sync_tokens, 3);
        assert_eq!(inventory.summary.api_keys, 2);
        assert_eq!(inventory.summary.expired_sync_tokens, 1);
        assert_eq!(
            inventory.summary.stale_sync_tokens, 1,
            "the expired token is not also counted as stale"
        );
        assert_eq!(inventory.other_browser_sessions(), 1);
        // Two-factor is filled in by the page, so the summary alone cannot say the
        // account is healthy.
        assert!(!inventory.summary.two_fa_enabled);
        assert!(inventory.summary.has_warning());

        let mut healthy = inventory.clone();
        healthy.summary.two_fa_enabled = true;
        healthy.summary.expired_sync_tokens = 0;
        healthy.summary.stale_sync_tokens = 0;
        assert!(!healthy.summary.has_warning());
    }
}
