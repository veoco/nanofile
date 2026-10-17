//! Functional capability catalog for the unified API-key system.
//!
//! A key is a set of independently grantable capabilities. `ROUTES` maps every
//! `/api2` and `/api/v2.1` route to the capability it needs, so enforcement lives
//! at one point ([`crate::middleware::auth::AuthUser`]) instead of in every
//! handler. Every credential is classified, not only keys: a session satisfies
//! every capability because it *is* the account, so the table records what a route
//! needs of a key plus the routes closed to keys ([`RouteAccess::SessionOnly`]).
//!
//! Capabilities are stable ids persisted as a canonical comma-separated list:
//! [`Capability::id`] is the on-disk contract, not the enum discriminant.

use axum::http::Method;

/// One independently grantable feature of the API.
///
/// The declaration order defines the bit index in [`CapabilitySet`], so never
/// reorder a variant; the persisted form is [`Capability::id`], not the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    AccountRead,
    AccountWrite,
    DeviceRead,
    DeviceWrite,
    LibraryRead,
    LibraryCreate,
    LibraryUpdate,
    LibraryDelete,
    LibraryPassword,
    FileRead,
    FileWrite,
    FileDelete,
    TrashRead,
    TrashWrite,
    HistoryRead,
    HistoryWrite,
    ShareLinkRead,
    ShareLinkWrite,
    UploadLinkRead,
    UploadLinkWrite,
    MetadataRead,
    MetadataWrite,
    TagRead,
    TagWrite,
    StarRead,
    StarWrite,
    ActivityRead,
    SearchRead,
    SearchReindex,
    SearchWrite,
    SyncRead,
    SyncWrite,
    SyncToken,
    NotificationRead,
    WebdavRead,
    WebdavWrite,
    AdminUserRead,
    AdminUserWrite,
    KeyRead,
    KeyWrite,
}

/// Grouping used by the management UI and the catalog endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapabilityDomain {
    Account,
    Device,
    Invitation,
    Library,
    File,
    Trash,
    History,
    Sharing,
    Metadata,
    Discovery,
    Sync,
    Notification,
    Webdav,
    Admin,
    Key,
}

impl CapabilityDomain {
    pub const ALL: [CapabilityDomain; 15] = [
        CapabilityDomain::Account,
        CapabilityDomain::Device,
        CapabilityDomain::Invitation,
        CapabilityDomain::Library,
        CapabilityDomain::File,
        CapabilityDomain::Trash,
        CapabilityDomain::History,
        CapabilityDomain::Sharing,
        CapabilityDomain::Metadata,
        CapabilityDomain::Discovery,
        CapabilityDomain::Sync,
        CapabilityDomain::Notification,
        CapabilityDomain::Webdav,
        CapabilityDomain::Admin,
        CapabilityDomain::Key,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            CapabilityDomain::Account => "account",
            CapabilityDomain::Device => "device",
            CapabilityDomain::Invitation => "invitation",
            CapabilityDomain::Library => "library",
            CapabilityDomain::File => "file",
            CapabilityDomain::Trash => "trash",
            CapabilityDomain::History => "history",
            CapabilityDomain::Sharing => "sharing",
            CapabilityDomain::Metadata => "metadata",
            CapabilityDomain::Discovery => "discovery",
            CapabilityDomain::Sync => "sync",
            CapabilityDomain::Notification => "notification",
            CapabilityDomain::Webdav => "webdav",
            CapabilityDomain::Admin => "admin",
            CapabilityDomain::Key => "key",
        }
    }
}

impl Capability {
    /// Every capability, in discriminant order.
    ///
    /// Only capabilities with an enforcement point appear here: offering a grant
    /// nothing consults would be a lie (removed ids live in [`RETIRED_IDS`]).
    pub const ALL: [Capability; 40] = [
        Capability::AccountRead,
        Capability::AccountWrite,
        Capability::DeviceRead,
        Capability::DeviceWrite,
        Capability::LibraryRead,
        Capability::LibraryCreate,
        Capability::LibraryUpdate,
        Capability::LibraryDelete,
        Capability::LibraryPassword,
        Capability::FileRead,
        Capability::FileWrite,
        Capability::FileDelete,
        Capability::TrashRead,
        Capability::TrashWrite,
        Capability::HistoryRead,
        Capability::HistoryWrite,
        Capability::ShareLinkRead,
        Capability::ShareLinkWrite,
        Capability::UploadLinkRead,
        Capability::UploadLinkWrite,
        Capability::MetadataRead,
        Capability::MetadataWrite,
        Capability::TagRead,
        Capability::TagWrite,
        Capability::StarRead,
        Capability::StarWrite,
        Capability::ActivityRead,
        Capability::SearchRead,
        Capability::SearchReindex,
        Capability::SearchWrite,
        Capability::SyncRead,
        Capability::SyncWrite,
        Capability::SyncToken,
        Capability::NotificationRead,
        Capability::WebdavRead,
        Capability::WebdavWrite,
        Capability::AdminUserRead,
        Capability::AdminUserWrite,
        Capability::KeyRead,
        Capability::KeyWrite,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Capability::AccountRead => "account.read",
            Capability::AccountWrite => "account.write",
            Capability::DeviceRead => "device.read",
            Capability::DeviceWrite => "device.write",
            Capability::LibraryRead => "library.read",
            Capability::LibraryCreate => "library.create",
            Capability::LibraryUpdate => "library.update",
            Capability::LibraryDelete => "library.delete",
            Capability::LibraryPassword => "library.password",
            Capability::FileRead => "file.read",
            Capability::FileWrite => "file.write",
            Capability::FileDelete => "file.delete",
            Capability::TrashRead => "trash.read",
            Capability::TrashWrite => "trash.write",
            Capability::HistoryRead => "history.read",
            Capability::HistoryWrite => "history.write",
            Capability::ShareLinkRead => "share_link.read",
            Capability::ShareLinkWrite => "share_link.write",
            Capability::UploadLinkRead => "upload_link.read",
            Capability::UploadLinkWrite => "upload_link.write",
            Capability::MetadataRead => "metadata.read",
            Capability::MetadataWrite => "metadata.write",
            Capability::TagRead => "tag.read",
            Capability::TagWrite => "tag.write",
            Capability::StarRead => "star.read",
            Capability::StarWrite => "star.write",
            Capability::ActivityRead => "activity.read",
            Capability::SearchRead => "search.read",
            Capability::SearchReindex => "search.reindex",
            Capability::SearchWrite => "search.write",
            Capability::SyncRead => "sync.read",
            Capability::SyncWrite => "sync.write",
            Capability::SyncToken => "sync.token",
            Capability::NotificationRead => "notification.read",
            Capability::WebdavRead => "webdav.read",
            Capability::WebdavWrite => "webdav.write",
            Capability::AdminUserRead => "admin.user.read",
            Capability::AdminUserWrite => "admin.user.write",
            Capability::KeyRead => "key.read",
            Capability::KeyWrite => "key.write",
        }
    }

    pub fn from_id(id: &str) -> Option<Capability> {
        Capability::ALL.iter().copied().find(|c| c.id() == id)
    }

    pub fn is_retired(id: &str) -> bool {
        RETIRED_IDS.contains(&id)
    }

    const fn bit(self) -> u128 {
        1u128 << (self as u32)
    }

    pub const fn domain(self) -> CapabilityDomain {
        match self {
            Capability::AccountRead | Capability::AccountWrite => CapabilityDomain::Account,
            Capability::DeviceRead | Capability::DeviceWrite => CapabilityDomain::Device,
            Capability::LibraryRead
            | Capability::LibraryCreate
            | Capability::LibraryUpdate
            | Capability::LibraryDelete
            | Capability::LibraryPassword => CapabilityDomain::Library,
            Capability::FileRead | Capability::FileWrite | Capability::FileDelete => {
                CapabilityDomain::File
            }
            Capability::TrashRead | Capability::TrashWrite => CapabilityDomain::Trash,
            Capability::HistoryRead | Capability::HistoryWrite => CapabilityDomain::History,
            Capability::ShareLinkRead
            | Capability::ShareLinkWrite
            | Capability::UploadLinkRead
            | Capability::UploadLinkWrite => CapabilityDomain::Sharing,
            Capability::MetadataRead
            | Capability::MetadataWrite
            | Capability::TagRead
            | Capability::TagWrite => CapabilityDomain::Metadata,
            Capability::StarRead
            | Capability::StarWrite
            | Capability::ActivityRead
            | Capability::SearchRead
            | Capability::SearchReindex
            | Capability::SearchWrite => CapabilityDomain::Discovery,
            Capability::SyncRead | Capability::SyncWrite | Capability::SyncToken => {
                CapabilityDomain::Sync
            }
            Capability::NotificationRead => CapabilityDomain::Notification,
            Capability::WebdavRead | Capability::WebdavWrite => CapabilityDomain::Webdav,
            Capability::AdminUserRead | Capability::AdminUserWrite => CapabilityDomain::Admin,
            Capability::KeyRead | Capability::KeyWrite => CapabilityDomain::Key,
        }
    }

    /// Whether granting this capability lets the caller change server state.
    /// Drives presets and labels only; enforcement itself is per route.
    pub const fn is_write(self) -> bool {
        !matches!(
            self,
            Capability::AccountRead
                | Capability::DeviceRead
                | Capability::LibraryRead
                | Capability::FileRead
                | Capability::TrashRead
                | Capability::HistoryRead
                | Capability::ShareLinkRead
                | Capability::UploadLinkRead
                | Capability::MetadataRead
                | Capability::TagRead
                | Capability::StarRead
                | Capability::ActivityRead
                | Capability::SearchRead
                | Capability::SyncRead
                | Capability::NotificationRead
                | Capability::WebdavRead
                | Capability::AdminUserRead
                | Capability::KeyRead
        )
    }

    /// Whether this capability is granted only to sessions.
    ///
    /// `key.*` is reserved: a key that can mint keys could grant itself more than
    /// it holds. `admin.*` is grantable only by an admin (the service checks).
    pub const fn is_reserved_to_sessions(self) -> bool {
        matches!(self, Capability::KeyRead | Capability::KeyWrite)
    }
}

/// Identifiers that were once grantable and are not any more.
///
/// A capability is offered only while something enforces it: `invitation.*` was
/// a session-only admin feature with no route, `history.manage` had nothing
/// beyond `history.read`/`history.write`, and `member.*`/`group.read` guarded
/// sharing and groups that are gone (the remaining group routes are stubs gated
/// on `library.read`, as is `related-users`).
///
/// A stored list may still name them: a key minted from `full` or `readonly_all`
/// before this build carries them. Stored lists drop such ids
/// ([`CapabilitySet::parse_stored`]) — a grant nothing consults cannot make a key
/// more permissive — while client input remains strict ([`CapabilitySet::parse`]
/// rejects a retired id rather than accepting a no-op).
pub const RETIRED_IDS: &[&str] = &[
    "invitation.read",
    "invitation.write",
    "history.manage",
    "member.read",
    "member.write",
    "group.read",
];

/// Capabilities implied by another capability, applied on parse so a stored list
/// never needs the implied entries: a client posting only `file.write` still gets
/// `file.read`.
const IMPLICATIONS: &[(Capability, &[Capability])] = &[
    (Capability::AccountWrite, &[Capability::AccountRead]),
    (Capability::DeviceWrite, &[Capability::DeviceRead]),
    (Capability::LibraryCreate, &[Capability::LibraryRead]),
    (Capability::LibraryUpdate, &[Capability::LibraryRead]),
    (Capability::LibraryDelete, &[Capability::LibraryRead]),
    (Capability::LibraryPassword, &[Capability::LibraryRead]),
    (Capability::FileWrite, &[Capability::FileRead]),
    (Capability::FileDelete, &[Capability::FileWrite]),
    (
        Capability::TrashWrite,
        &[Capability::TrashRead, Capability::FileDelete],
    ),
    (Capability::HistoryRead, &[Capability::FileRead]),
    (Capability::HistoryWrite, &[Capability::HistoryRead]),
    (Capability::ShareLinkWrite, &[Capability::ShareLinkRead]),
    (Capability::UploadLinkWrite, &[Capability::UploadLinkRead]),
    (Capability::MetadataRead, &[Capability::LibraryRead]),
    (Capability::MetadataWrite, &[Capability::MetadataRead]),
    (Capability::TagRead, &[Capability::MetadataRead]),
    (Capability::TagWrite, &[Capability::TagRead]),
    (Capability::StarWrite, &[Capability::StarRead]),
    (Capability::SearchReindex, &[Capability::SearchRead]),
    (Capability::SearchWrite, &[Capability::SearchRead]),
    (Capability::SyncWrite, &[Capability::SyncRead]),
    (Capability::SyncToken, &[Capability::SyncWrite]),
    (Capability::WebdavWrite, &[Capability::WebdavRead]),
    (Capability::AdminUserWrite, &[Capability::AdminUserRead]),
    (Capability::KeyWrite, &[Capability::KeyRead]),
];

fn implied_by(capability: Capability) -> &'static [Capability] {
    IMPLICATIONS
        .iter()
        .find(|(c, _)| *c == capability)
        .map(|(_, implied)| *implied)
        .unwrap_or(&[])
}

/// A set of capabilities, stored in memory as a bit set.
///
/// `u128` covers the catalog (40 today); a 129th capability fails to compile at
/// `Capability::bit`, prompting a widening together with the persisted format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet(u128);

/// Why a persisted capability list could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownCapability(pub String);

impl std::fmt::Display for UnknownCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown capability: {}", self.0)
    }
}

impl std::error::Error for UnknownCapability {}

impl CapabilitySet {
    pub const EMPTY: CapabilitySet = CapabilitySet(0);

    pub fn insert(&mut self, capability: Capability) {
        self.0 |= capability.bit();
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn len(&self) -> u32 {
        self.0.count_ones()
    }

    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        Capability::ALL
            .iter()
            .copied()
            .filter(|c| self.contains(*c))
    }

    /// Whether every capability in `other` is present here.
    pub fn contains_all(&self, other: &CapabilitySet) -> bool {
        self.0 & other.0 == other.0
    }

    /// Parse an identifier list and expand the implication closure.
    ///
    /// An unknown id is an error rather than a silent drop: a typo must not
    /// produce a key lacking the access its creator intended. Client input path;
    /// stored lists use [`CapabilitySet::parse_stored`], which tolerates retirement.
    pub fn parse(ids: &[String]) -> Result<CapabilitySet, UnknownCapability> {
        let mut set = CapabilitySet::EMPTY;
        for id in ids {
            let capability =
                Capability::from_id(id).ok_or_else(|| UnknownCapability(id.clone()))?;
            set.insert(capability);
        }
        set.expand();
        Ok(set)
    }

    /// Parse an identifier list that came out of the database.
    ///
    /// Retired ids ([`RETIRED_IDS`]) are dropped, so a key minted before a
    /// retirement keeps working; one that is neither live nor retired is an error.
    pub fn parse_stored(ids: &[String]) -> Result<CapabilitySet, UnknownCapability> {
        let live: Vec<String> = ids
            .iter()
            .filter(|id| !Capability::is_retired(id))
            .cloned()
            .collect();
        CapabilitySet::parse(&live)
    }

    /// Expand every capability to its full implication closure.
    pub fn expand(&mut self) {
        loop {
            let mut added = false;
            for capability in self.iter().collect::<Vec<_>>() {
                for implied in implied_by(capability) {
                    if !self.contains(*implied) {
                        self.insert(*implied);
                        added = true;
                    }
                }
            }
            if !added {
                return;
            }
        }
    }

    /// Canonical, sorted identifier list (what gets persisted).
    pub fn to_ids(&self) -> Vec<&'static str> {
        self.iter().map(|c| c.id()).collect()
    }

    /// Parse the canonical on-disk form (comma-separated identifiers).
    pub fn from_canonical(value: &str) -> Result<CapabilitySet, UnknownCapability> {
        let ids: Vec<String> = value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        CapabilitySet::parse_stored(&ids)
    }

    pub fn to_canonical(&self) -> String {
        self.to_ids().join(",")
    }
}

/// What a route requires of the caller; consulted for every credential, not only
/// keys (see `Credential::allows_route`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteAccess {
    /// No credential: a public endpoint, or a handler that authenticates the
    /// request itself (e.g. `POST /api2/device-wiped/`).
    Public,
    /// Any valid credential is enough; used for probes such as `/api2/auth/ping/`.
    AnyAuthenticated,
    /// The account itself; API keys are rejected. Covers login/logout, SSO and key
    /// management: a key that can mint keys could grant itself more than it holds.
    SessionOnly,
    /// A key needs this capability; a session satisfies it because it *is* the
    /// account, not a subset of it.
    Capability(Capability),
}

/// Route classification table.
///
/// `{name}` matches exactly one non-empty segment; the table covers every `/api2`
/// and `/api/v2.1` route (see `server/tests/capability_coverage_test.rs`). A
/// route missing here is denied to every credential, the intended failure.
#[allow(clippy::type_complexity)]
const ROUTES: &[(&str, &str, RouteAccess)] = &[
    ("POST", "/api2/auth-token/", RouteAccess::Public),
    ("GET", "/api2/auth/ping/", RouteAccess::AnyAuthenticated),
    ("POST", "/api2/logout-device/", RouteAccess::SessionOnly),
    ("GET", "/api2/ping/", RouteAccess::Public),
    ("GET", "/api2/server-info/", RouteAccess::Public),
    // device-wiped reads its token from the form or header, never reaching `AuthUser`.
    ("POST", "/api2/device-wiped/", RouteAccess::Public),
    ("POST", "/api2/client-sso-link/", RouteAccess::Public),
    ("GET", "/api2/client-sso-link/{token}/", RouteAccess::Public),
    ("POST", "/api2/client-login/", RouteAccess::SessionOnly),
    (
        "GET",
        "/api2/account/info/",
        RouteAccess::Capability(Capability::AccountRead),
    ),
    (
        "PUT",
        "/api2/account/info/",
        RouteAccess::Capability(Capability::AccountWrite),
    ),
    // Account creation over the API is admin-only (the handler enforces it).
    (
        "POST",
        "/api2/account/",
        RouteAccess::Capability(Capability::AdminUserWrite),
    ),
    (
        "POST",
        "/api2/accounts/",
        RouteAccess::Capability(Capability::AdminUserWrite),
    ),
    (
        "GET",
        "/api2/avatars/user/{email}/resized/{size}/",
        RouteAccess::Capability(Capability::AccountRead),
    ),
    (
        "POST",
        "/api/v2.1/user-avatar/",
        RouteAccess::Capability(Capability::AccountWrite),
    ),
    (
        "GET",
        "/api2/devices/",
        RouteAccess::Capability(Capability::DeviceRead),
    ),
    (
        "DELETE",
        "/api2/devices/",
        RouteAccess::Capability(Capability::DeviceWrite),
    ),
    // Credential inventory: wider than `/api2/devices/` (sessions and sync tokens
    // are not devices) but the same management surface, so the same capability.
    (
        "GET",
        "/api2/credentials/",
        RouteAccess::Capability(Capability::DeviceRead),
    ),
    (
        "DELETE",
        "/api2/credentials/{kind}/{id}/",
        RouteAccess::Capability(Capability::DeviceWrite),
    ),
    (
        "GET",
        "/api2/unseen_messages/",
        RouteAccess::Capability(Capability::NotificationRead),
    ),
    (
        "GET",
        "/api2/repos/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "POST",
        "/api2/repos/",
        RouteAccess::Capability(Capability::LibraryCreate),
    ),
    (
        "GET",
        "/api/v2.1/repos/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/",
        RouteAccess::Capability(Capability::LibraryUpdate),
    ),
    (
        "DELETE",
        "/api2/repos/{repo_id}/",
        RouteAccess::Capability(Capability::LibraryDelete),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/{repo_id}/",
        RouteAccess::Capability(Capability::LibraryDelete),
    ),
    (
        "GET",
        "/api2/default-repo/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "POST",
        "/api2/default-repo/",
        RouteAccess::Capability(Capability::LibraryCreate),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/dir/sub_repo/",
        RouteAccess::Capability(Capability::LibraryCreate),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/set-password/",
        RouteAccess::Capability(Capability::LibraryPassword),
    ),
    (
        "PUT",
        "/api/v2.1/repos/{repo_id}/set-password/",
        RouteAccess::Capability(Capability::LibraryPassword),
    ),
    (
        "GET",
        "/api/v2.1/deleted-repos/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "POST",
        "/api/v2.1/deleted-repos/",
        RouteAccess::Capability(Capability::LibraryDelete),
    ),
    (
        "DELETE",
        "/api/v2.1/deleted-repos/",
        RouteAccess::Capability(Capability::LibraryDelete),
    ),
    (
        "DELETE",
        "/api/v2.1/deleted-repos/{repo_id}/",
        RouteAccess::Capability(Capability::LibraryDelete),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/dir/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/dir/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "DELETE",
        "/api2/repos/{repo_id}/dir/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/dir/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/dir/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/{repo_id}/dir/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/dir/detail/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/dir/move/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/dir/rename/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/dir/shared_items/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/file/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/file/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "PUT",
        "/api2/repos/{repo_id}/file/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "DELETE",
        "/api2/repos/{repo_id}/file/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/file/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/{repo_id}/file/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/file/rename/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/file/move/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/file/detail/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/file/exif/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/file/index-text/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/file/reindex/",
        RouteAccess::Capability(Capability::SearchReindex),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/thumbnail/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/file-uploaded-bytes/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/file-uploaded-bytes/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/zip-task/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    // Outside the API namespaces, but serves the same bytes as `/api2/.../file/`.
    (
        "GET",
        "/repos/{repo_id}/files/{*path}",
        RouteAccess::Capability(Capability::FileRead),
    ),
    // These answer with a bearer URL (or a block link), so they are write-level
    // even on GET: a read-only key must not be able to mint an upload credential.
    (
        "GET",
        "/api2/repos/{repo_id}/upload-link/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/update-link/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/upload-blks-link/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/update-blks-link/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/fileops/delete/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/fileops/copy/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api2/repos/{repo_id}/fileops/move/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api/v2.1/repos/sync-batch-move-item/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api/v2.1/repos/sync-batch-copy-item/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api/v2.1/repos/batch-delete-item/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/batch-delete-item/",
        RouteAccess::Capability(Capability::FileDelete),
    ),
    (
        "POST",
        "/api/v2.1/repos/async-batch-copy-item/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api/v2.1/repos/async-batch-move-item/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "POST",
        "/api/v2.1/copy-move-task/",
        RouteAccess::Capability(Capability::FileWrite),
    ),
    (
        "GET",
        "/api/v2.1/query-copy-move-progress/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/trash/",
        RouteAccess::Capability(Capability::TrashRead),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/{repo_id}/trash/",
        RouteAccess::Capability(Capability::TrashWrite),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/trash/revert-dirents/",
        RouteAccess::Capability(Capability::TrashWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/trash2/",
        RouteAccess::Capability(Capability::TrashRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/trash2/search/",
        RouteAccess::Capability(Capability::TrashRead),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/trash2/revert/",
        RouteAccess::Capability(Capability::TrashWrite),
    ),
    (
        "GET",
        "/api2/repo_history_changes/{repo_id}/",
        RouteAccess::Capability(Capability::HistoryRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/file/history/",
        RouteAccess::Capability(Capability::HistoryRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/file/revision/",
        RouteAccess::Capability(Capability::HistoryRead),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/file/revision/restore/",
        RouteAccess::Capability(Capability::HistoryWrite),
    ),
    (
        "GET",
        "/api2/shared-links/",
        RouteAccess::Capability(Capability::ShareLinkRead),
    ),
    (
        "POST",
        "/api2/shared-links/",
        RouteAccess::Capability(Capability::ShareLinkWrite),
    ),
    (
        "DELETE",
        "/api2/shared-links/{token}",
        RouteAccess::Capability(Capability::ShareLinkWrite),
    ),
    (
        "GET",
        "/api/v2.1/share-links/",
        RouteAccess::Capability(Capability::ShareLinkRead),
    ),
    (
        "POST",
        "/api/v2.1/share-links/",
        RouteAccess::Capability(Capability::ShareLinkWrite),
    ),
    (
        "POST",
        "/api/v2.1/multi-share-links/",
        RouteAccess::Capability(Capability::ShareLinkWrite),
    ),
    (
        "GET",
        "/api/v2.1/share-links/{token}/",
        RouteAccess::Capability(Capability::ShareLinkRead),
    ),
    (
        "PUT",
        "/api/v2.1/share-links/{token}/",
        RouteAccess::Capability(Capability::ShareLinkWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/share-links/{token}/",
        RouteAccess::Capability(Capability::ShareLinkWrite),
    ),
    (
        "GET",
        "/api2/upload-links/",
        RouteAccess::Capability(Capability::UploadLinkRead),
    ),
    (
        "POST",
        "/api2/upload-links/",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "DELETE",
        "/api2/upload-links/{token}",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "GET",
        "/api/v2.1/upload-links/",
        RouteAccess::Capability(Capability::UploadLinkRead),
    ),
    (
        "POST",
        "/api/v2.1/upload-links/",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/upload-links/clean-invalid/",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "GET",
        "/api/v2.1/upload-links/{token}/",
        RouteAccess::Capability(Capability::UploadLinkRead),
    ),
    (
        "PUT",
        "/api/v2.1/upload-links/{token}/",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/upload-links/{token}/",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "GET",
        "/api/v2.1/upload-links/{token}/upload/",
        RouteAccess::Capability(Capability::UploadLinkWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/upload-links/",
        RouteAccess::Capability(Capability::UploadLinkRead),
    ),
    (
        "GET",
        "/api2/groups/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "GET",
        "/api/v2.1/groups/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/related-users/",
        RouteAccess::Capability(Capability::LibraryRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/metadata/",
        RouteAccess::Capability(Capability::MetadataRead),
    ),
    (
        "PUT",
        "/api/v2.1/repos/{repo_id}/metadata/",
        RouteAccess::Capability(Capability::MetadataWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/metadata/record/",
        RouteAccess::Capability(Capability::MetadataRead),
    ),
    (
        "PUT",
        "/api/v2.1/repos/{repo_id}/metadata/record/",
        RouteAccess::Capability(Capability::MetadataWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/metadata/tags/",
        RouteAccess::Capability(Capability::TagRead),
    ),
    (
        "POST",
        "/api/v2.1/repos/{repo_id}/metadata/tags/",
        RouteAccess::Capability(Capability::TagWrite),
    ),
    (
        "PUT",
        "/api/v2.1/repos/{repo_id}/metadata/tags/",
        RouteAccess::Capability(Capability::TagWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/{repo_id}/metadata/tags/",
        RouteAccess::Capability(Capability::TagWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/metadata/tags-status/",
        RouteAccess::Capability(Capability::TagRead),
    ),
    (
        "PUT",
        "/api/v2.1/repos/{repo_id}/metadata/tags-status/",
        RouteAccess::Capability(Capability::TagWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/repos/{repo_id}/metadata/tags-status/",
        RouteAccess::Capability(Capability::TagWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/metadata/tag-files/{tag_id}/",
        RouteAccess::Capability(Capability::TagRead),
    ),
    (
        "PUT",
        "/api/v2.1/repos/{repo_id}/metadata/file-tags/",
        RouteAccess::Capability(Capability::TagWrite),
    ),
    (
        "GET",
        "/api2/starredfiles/",
        RouteAccess::Capability(Capability::StarRead),
    ),
    (
        "GET",
        "/api/v2.1/starred-items/",
        RouteAccess::Capability(Capability::StarRead),
    ),
    (
        "POST",
        "/api/v2.1/starred-items/",
        RouteAccess::Capability(Capability::StarWrite),
    ),
    (
        "DELETE",
        "/api/v2.1/starred-items/",
        RouteAccess::Capability(Capability::StarWrite),
    ),
    (
        "GET",
        "/api/v2.1/activities/",
        RouteAccess::Capability(Capability::ActivityRead),
    ),
    (
        "GET",
        "/api2/search/",
        RouteAccess::Capability(Capability::SearchRead),
    ),
    (
        "GET",
        "/api/v2.1/search-file/",
        RouteAccess::Capability(Capability::SearchRead),
    ),
    (
        "GET",
        "/api/v2.1/search-file",
        RouteAccess::Capability(Capability::SearchRead),
    ),
    (
        "POST",
        "/api2/reindex/",
        RouteAccess::Capability(Capability::SearchReindex),
    ),
    (
        "GET",
        "/api2/reindex-progress/",
        RouteAccess::Capability(Capability::SearchReindex),
    ),
    (
        "POST",
        "/api2/index-file-text/",
        RouteAccess::Capability(Capability::SearchWrite),
    ),
    // The minted sync token is a full read/write library credential, so issuance
    // is write-level: a read-only key must not be able to obtain one.
    (
        "GET",
        "/api2/repo-tokens/",
        RouteAccess::Capability(Capability::SyncToken),
    ),
    (
        "GET",
        "/api2/repos/{repo_id}/download-info/",
        RouteAccess::Capability(Capability::SyncToken),
    ),
    (
        "GET",
        "/api/v2.1/smart-link/",
        RouteAccess::Capability(Capability::FileRead),
    ),
    (
        "GET",
        "/api2/admin/users/",
        RouteAccess::Capability(Capability::AdminUserRead),
    ),
    (
        "POST",
        "/api2/admin/users/",
        RouteAccess::Capability(Capability::AdminUserWrite),
    ),
    (
        "PUT",
        "/api2/admin/users/{user_id}/",
        RouteAccess::Capability(Capability::AdminUserWrite),
    ),
    (
        "DELETE",
        "/api2/admin/users/{user_id}/",
        RouteAccess::Capability(Capability::AdminUserWrite),
    ),
    // Session-only, catalog included: a key that can mint keys could grant itself
    // more than it holds, so no part of this surface accepts key authentication.
    ("GET", "/api2/api-keys/catalog/", RouteAccess::SessionOnly),
    ("GET", "/api2/api-keys/", RouteAccess::SessionOnly),
    ("POST", "/api2/api-keys/", RouteAccess::SessionOnly),
    ("GET", "/api2/api-keys/{id}/", RouteAccess::SessionOnly),
    ("PUT", "/api2/api-keys/{id}/", RouteAccess::SessionOnly),
    ("DELETE", "/api2/api-keys/{id}/", RouteAccess::SessionOnly),
];

/// Classify a request path for the credential guard.
///
/// `None` means the path is outside the classified surface or absent from
/// `ROUTES`; callers must treat that as denied, so a new unclassified route fails
/// closed instead of accepting every credential.
pub fn required_access(method: &Method, path: &str) -> Option<RouteAccess> {
    if !is_classified_prefix(path) {
        return None;
    }
    // Preflights are answered by the CORS layer; classifying them as "any
    // credential" keeps a stray OPTIONS from being reported as an unknown route.
    if method == Method::OPTIONS {
        return Some(RouteAccess::AnyAuthenticated);
    }
    // A HEAD request is served by the matching GET route.
    let method = if method == Method::HEAD {
        "GET"
    } else {
        method.as_str()
    };

    ROUTES
        .iter()
        .filter(|(m, pattern, _)| *m == method && pattern_matches(pattern, path))
        .max_by_key(|(_, pattern, _)| specificity(pattern))
        .map(|(_, _, access)| *access)
}

/// Human-readable list of the enforcement points that consult `capability`.
///
/// The management UI renders it as a tooltip and `/api2/api-keys/catalog/` returns
/// it, so "what does granting this buy?" is answered by the route table itself.
/// Every capability in [`Capability::ALL`] has at least one entry.
pub fn enforced_by(capability: Capability) -> Vec<String> {
    // Two surfaces authenticate on their own and never reach the route table:
    // WebDAV's Basic-auth extractor and the sync protocol's `/seafhttp` guard.
    match capability {
        Capability::WebdavRead => {
            return vec!["GET|HEAD|PROPFIND|REPORT /dav/{repo_id}/…".to_string()];
        }
        Capability::WebdavWrite => {
            return vec!["PUT|POST|DELETE|MKCOL|COPY|MOVE /dav/{repo_id}/…".to_string()];
        }
        Capability::SyncRead => return vec!["GET /seafhttp/…".to_string()],
        Capability::SyncWrite => return vec!["POST|PUT /seafhttp/…".to_string()],
        _ => {}
    }
    ROUTES
        .iter()
        .filter(|(_, _, access)| matches!(access, RouteAccess::Capability(c) if *c == capability))
        .map(|(method, pattern, _)| format!("{method} {pattern}"))
        .collect()
}

fn is_classified_prefix(path: &str) -> bool {
    path == "/api2"
        || path.starts_with("/api2/")
        || path == "/api/v2.1"
        || path.starts_with("/api/v2.1/")
        // The web file-content endpoint is outside the API namespaces but accepts
        // an API key, so it must be classified rather than fall through to "deny".
        || path.starts_with("/repos/")
}

/// Whether `path` matches a `{name}`-style pattern.
///
/// `{name}` matches one non-empty segment; `{*name}` matches the remaining ones.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let path_segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    for (index, segment) in pattern_segments.iter().enumerate() {
        if let Some(_rest) = segment.strip_prefix("{*").and_then(|s| s.strip_suffix('}')) {
            // A catch-all must consume at least one segment, matching axum.
            return path_segments.len() > index;
        }
        let Some(actual) = path_segments.get(index) else {
            return false;
        };
        if segment.starts_with('{') && segment.ends_with('}') {
            // A placeholder never matches an empty segment (doubled slash).
            if actual.is_empty() {
                return false;
            }
        } else if segment != actual {
            return false;
        }
    }
    pattern_segments.len() == path_segments.len()
}

/// Prefer the pattern with more literal characters, so a specific route wins.
fn specificity(pattern: &str) -> usize {
    pattern
        .trim_matches('/')
        .split('/')
        .filter(|segment| !(segment.starts_with('{') && segment.ends_with('}')))
        .map(str::len)
        .sum()
}

/// Whether a `/seafhttp/...` request mutates repository state.
///
/// The sync protocol has no route table here (paths arrive both in full and
/// prefix-stripped form), so this classifies by method plus an explicit list of
/// read-only POSTs; anything unrecognised is a write, so a read-only key fails
/// closed on a new endpoint.
pub fn requires_sync_write(method: &Method, path: &str) -> bool {
    if method == Method::GET || method == Method::HEAD {
        return false;
    }
    // `folder-perm` and the batch `locked-files` query carry their token in the
    // body and only read; `head-commits-multi` is a batch metadata read.
    const READ_ONLY_POST_SUFFIXES: [&str; 3] =
        ["/head-commits-multi", "/folder-perm", "/locked-files"];
    let trimmed = path.trim_end_matches('/');
    !READ_ONLY_POST_SUFFIXES
        .iter()
        .any(|suffix| trimmed.ends_with(suffix))
}

/// Whether a WebDAV method mutates repository state.
///
/// `PROPFIND`/`REPORT` are reads; every other method, including unimplemented
/// ones, counts as a write so a read-only key fails closed.
pub fn webdav_requires_write(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS" | "PROPFIND" | "REPORT")
}

/// A named starting point offered by the management UI.
pub struct Preset {
    pub id: &'static str,
    pub capabilities: &'static [Capability],
}

/// Presets are convenience only: they expand to ordinary capability sets and the
/// user can edit any of them before saving.
///
/// A preset stays within its own surface. `webdav_*` is the sharpest case: the
/// `/dav/...` guard consults only `webdav.*` plus the library binding, so adding
/// `file.*` or `library.read` would hand a WebDAV key the REST API of the bound
/// libraries, including `file.delete`.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "sync_client_rw",
        capabilities: &[
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::FileDelete,
            Capability::TrashRead,
            Capability::TrashWrite,
            Capability::HistoryRead,
            Capability::MetadataRead,
            Capability::TagRead,
            Capability::StarRead,
            Capability::StarWrite,
            Capability::ActivityRead,
            Capability::SearchRead,
            Capability::SyncRead,
            Capability::SyncWrite,
            Capability::SyncToken,
            Capability::NotificationRead,
        ],
    },
    Preset {
        id: "sync_client_ro",
        capabilities: &[
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::TrashRead,
            Capability::HistoryRead,
            Capability::MetadataRead,
            Capability::TagRead,
            Capability::StarRead,
            Capability::ActivityRead,
            Capability::SearchRead,
            Capability::SyncRead,
            Capability::NotificationRead,
        ],
    },
    Preset {
        id: "webdav_rw",
        capabilities: &[Capability::WebdavRead, Capability::WebdavWrite],
    },
    Preset {
        id: "webdav_ro",
        capabilities: &[Capability::WebdavRead],
    },
    Preset {
        id: "ci_upload",
        capabilities: &[
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::UploadLinkRead,
            Capability::UploadLinkWrite,
        ],
    },
    Preset {
        id: "readonly_all",
        capabilities: &[
            Capability::AccountRead,
            Capability::DeviceRead,
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::TrashRead,
            Capability::HistoryRead,
            Capability::ShareLinkRead,
            Capability::UploadLinkRead,
            Capability::MetadataRead,
            Capability::TagRead,
            Capability::StarRead,
            Capability::ActivityRead,
            Capability::SearchRead,
            Capability::SyncRead,
            Capability::NotificationRead,
            Capability::WebdavRead,
        ],
    },
    Preset {
        id: "full",
        capabilities: &[
            Capability::AccountRead,
            Capability::AccountWrite,
            Capability::DeviceRead,
            Capability::DeviceWrite,
            Capability::LibraryRead,
            Capability::LibraryCreate,
            Capability::LibraryUpdate,
            Capability::LibraryDelete,
            Capability::LibraryPassword,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::FileDelete,
            Capability::TrashRead,
            Capability::TrashWrite,
            Capability::HistoryRead,
            Capability::HistoryWrite,
            Capability::ShareLinkRead,
            Capability::ShareLinkWrite,
            Capability::UploadLinkRead,
            Capability::UploadLinkWrite,
            Capability::MetadataRead,
            Capability::MetadataWrite,
            Capability::TagRead,
            Capability::TagWrite,
            Capability::StarRead,
            Capability::StarWrite,
            Capability::ActivityRead,
            Capability::SearchRead,
            Capability::SearchReindex,
            Capability::SearchWrite,
            Capability::SyncRead,
            Capability::SyncWrite,
            Capability::SyncToken,
            Capability::NotificationRead,
            Capability::WebdavRead,
            Capability::WebdavWrite,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> CapabilitySet {
        let owned: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        CapabilitySet::parse(&owned).expect("known capabilities")
    }

    /// A preset expanded the way the service expands it for the picker.
    fn preset_set(id: &str) -> CapabilitySet {
        let preset = PRESETS.iter().find(|p| p.id == id).expect("preset");
        let mut set = CapabilitySet::EMPTY;
        for capability in preset.capabilities {
            set.insert(*capability);
        }
        set.expand();
        set
    }

    fn catalog_set() -> CapabilitySet {
        let mut set = CapabilitySet::EMPTY;
        for capability in Capability::ALL {
            set.insert(capability);
        }
        set
    }

    #[test]
    fn catalog_ids_are_unique_and_round_trip() {
        assert_eq!(Capability::ALL.len(), 40);
        let mut ids: Vec<&str> = Capability::ALL.iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Capability::ALL.len(), "duplicate capability id");
        for capability in Capability::ALL {
            assert_eq!(Capability::from_id(capability.id()), Some(capability));
        }
        assert_eq!(Capability::from_id("file.upload"), None);
        // A retired id is out of the catalog but still recognised as retired.
        for id in RETIRED_IDS {
            assert_eq!(
                Capability::from_id(id),
                None,
                "{id} must be out of the catalog"
            );
            assert!(Capability::is_retired(id));
        }
        assert!(!Capability::is_retired("file.upload"));
    }

    #[test]
    fn every_capability_has_a_domain_and_a_write_label() {
        for capability in Capability::ALL {
            assert!(
                CapabilityDomain::ALL.contains(&capability.domain()),
                "{} has no registered domain",
                capability.id()
            );
        }
        // Read capabilities stay out of the write presets.
        assert!(!Capability::FileRead.is_write());
        assert!(Capability::FileWrite.is_write());
        assert!(Capability::FileDelete.is_write());
        assert!(Capability::SyncToken.is_write());
        assert!(Capability::WebdavWrite.is_write());
        assert!(!Capability::WebdavRead.is_write());
        // Writing index text is a write; rebuilding an index is a different one.
        assert!(Capability::SearchWrite.is_write());
        assert!(Capability::SearchReindex.is_write());
        assert!(!Capability::SearchRead.is_write());
    }

    #[test]
    fn write_implies_read_transitively() {
        let parsed = set(&["file.delete"]);
        assert!(parsed.contains(Capability::FileDelete));
        assert!(parsed.contains(Capability::FileWrite));
        assert!(parsed.contains(Capability::FileRead));
        assert!(!parsed.contains(Capability::ShareLinkRead));

        let parsed = set(&["tag.write"]);
        assert!(parsed.contains(Capability::TagRead));
        assert!(parsed.contains(Capability::MetadataRead));
        assert!(parsed.contains(Capability::LibraryRead));

        let parsed = set(&["sync.token"]);
        assert!(parsed.contains(Capability::SyncWrite));
        assert!(parsed.contains(Capability::SyncRead));
    }

    #[test]
    fn search_write_implies_search_read_but_not_reindex() {
        let parsed = set(&["search.write"]);
        assert!(parsed.contains(Capability::SearchWrite));
        assert!(parsed.contains(Capability::SearchRead));
        assert!(!parsed.contains(Capability::SearchReindex));

        // Rebuilding an index is not the same power as writing its text, so the
        // implication does not run the other way either.
        let parsed = set(&["search.reindex"]);
        assert!(parsed.contains(Capability::SearchRead));
        assert!(!parsed.contains(Capability::SearchWrite));
    }

    #[test]
    fn manual_index_writes_need_search_write() {
        assert_eq!(
            required_access(&Method::POST, "/api2/index-file-text/"),
            Some(RouteAccess::Capability(Capability::SearchWrite))
        );
        // A whole-repository rebuild (and its progress) stays on reindex.
        assert_eq!(
            required_access(&Method::POST, "/api2/reindex/"),
            Some(RouteAccess::Capability(Capability::SearchReindex))
        );
        assert_eq!(
            required_access(&Method::GET, "/api2/reindex-progress/"),
            Some(RouteAccess::Capability(Capability::SearchReindex))
        );
    }

    #[test]
    fn canonical_form_round_trips() {
        let parsed = set(&["file.delete", "webdav.write", "file.delete"]);
        let canonical = parsed.to_canonical();
        assert_eq!(
            canonical,
            "file.read,file.write,file.delete,webdav.read,webdav.write"
        );
        assert_eq!(CapabilitySet::from_canonical(&canonical).unwrap(), parsed);
        assert_eq!(
            CapabilitySet::from_canonical("").unwrap(),
            CapabilitySet::EMPTY
        );
    }

    #[test]
    fn unknown_capability_is_an_error() {
        let err = CapabilitySet::parse(&["file.rede".to_string()]).unwrap_err();
        assert_eq!(err.0, "file.rede");
        assert_eq!(err.to_string(), "unknown capability: file.rede");
    }

    #[test]
    fn retired_ids_are_dropped_from_stored_lists_but_rejected_as_input() {
        // A key minted by an older build keeps working with what we can enforce.
        let parsed = CapabilitySet::from_canonical("invitation.read,file.read,history.manage")
            .expect("stored lists tolerate retirement");
        assert_eq!(parsed, set(&["file.read"]));
        assert_eq!(parsed.to_canonical(), "file.read");

        // Client input stays strict: a grant that does nothing must be refused,
        // not silently accepted.
        for id in RETIRED_IDS {
            assert!(
                CapabilitySet::parse(&[(*id).to_string()]).is_err(),
                "{id} must not be accepted from a client"
            );
        }
    }

    #[test]
    fn every_catalog_capability_has_an_enforcement_point() {
        for capability in Capability::ALL {
            // `key.*` is reserved to sessions and never granted (the management
            // routes are `SessionOnly`), so it is outside the picker's promise.
            if capability.is_reserved_to_sessions() {
                continue;
            }
            assert!(
                !enforced_by(capability).is_empty(),
                "{} is offered by the catalog but no surface enforces it",
                capability.id()
            );
        }
        // The four surfaces that are not in the route table are still named.
        assert_eq!(
            enforced_by(Capability::WebdavRead),
            vec!["GET|HEAD|PROPFIND|REPORT /dav/{repo_id}/…".to_string()]
        );
        assert_eq!(
            enforced_by(Capability::SyncRead),
            vec!["GET /seafhttp/…".to_string()]
        );
    }

    #[test]
    fn route_table_only_names_live_capabilities() {
        for (method, pattern, access) in ROUTES {
            if let RouteAccess::Capability(capability) = access {
                assert!(
                    Capability::ALL.contains(capability),
                    "{method} {pattern} requires {}, which is not in the catalog",
                    capability.id()
                );
            }
        }
        // Every live capability is enforced by the table or by one of the four
        // non-table surfaces; `key.*` is the deliberate exception, reserved.
        let non_table = [
            Capability::SyncRead,
            Capability::SyncWrite,
            Capability::WebdavRead,
            Capability::WebdavWrite,
        ];
        for capability in Capability::ALL {
            if capability.is_reserved_to_sessions() {
                continue;
            }
            let in_table = ROUTES.iter().any(
                |(_, _, access)| matches!(access, RouteAccess::Capability(c) if *c == capability),
            );
            assert!(
                in_table || non_table.contains(&capability),
                "{} is in the catalog but nowhere in the route table",
                capability.id()
            );
        }
    }

    #[test]
    fn webdav_presets_grant_exactly_the_webdav_surface() {
        let mut ro = CapabilitySet::EMPTY;
        let mut rw = CapabilitySet::EMPTY;
        let mut all = CapabilitySet::EMPTY;
        for capability in Capability::ALL {
            if capability.domain() == CapabilityDomain::Webdav {
                all.insert(capability);
                if !capability.is_write() {
                    ro.insert(capability);
                }
                rw.insert(capability);
            }
        }
        // A WebDAV key authenticates with `webdav.*` and its library binding
        // alone; anything else in these presets would hand it the REST API too.
        assert_eq!(preset_set("webdav_ro"), ro);
        assert_eq!(preset_set("webdav_rw"), rw);
        assert_eq!(all, set(&["webdav.read", "webdav.write"]));
        assert!(
            catalog_set().contains_all(&preset_set("webdav_rw")),
            "a preset may only name capabilities the catalog offers"
        );
    }

    #[test]
    fn ci_upload_cannot_mint_a_repository_credential() {
        let ci = preset_set("ci_upload");
        assert!(ci.contains(Capability::FileWrite));
        assert!(ci.contains(Capability::UploadLinkWrite));
        // `sync.token` mints a full read/write credential for the bound libraries
        // (see `/api2/repo-tokens/`), which an upload preset must not hand out.
        assert!(!ci.contains(Capability::SyncToken));
        assert!(!ci.contains(Capability::SyncRead));
        assert!(!ci.contains(Capability::SyncWrite));
    }

    #[test]
    fn blanket_presets_follow_the_catalog() {
        let mut full = CapabilitySet::EMPTY;
        let mut read_only = CapabilitySet::EMPTY;
        for capability in Capability::ALL {
            let admin = matches!(
                capability,
                Capability::AdminUserRead | Capability::AdminUserWrite
            );
            // No preset may grant `admin.*` or the session-reserved `key.*`.
            if admin || capability.is_reserved_to_sessions() {
                continue;
            }
            full.insert(capability);
            if !capability.is_write() {
                read_only.insert(capability);
            }
        }
        full.expand();
        read_only.expand();
        assert_eq!(preset_set("full"), full, "full must track the catalog");
        assert_eq!(
            preset_set("readonly_all"),
            read_only,
            "readonly_all must track every read capability"
        );
    }

    #[test]
    fn presets_never_contain_reserved_capabilities() {
        for preset in PRESETS {
            let mut parsed = CapabilitySet::EMPTY;
            for capability in preset.capabilities {
                parsed.insert(*capability);
            }
            parsed.expand();
            for capability in parsed.iter() {
                assert!(
                    !capability.is_reserved_to_sessions(),
                    "preset {} grants reserved {}",
                    preset.id,
                    capability.id()
                );
                assert!(
                    !matches!(
                        capability,
                        Capability::AdminUserRead | Capability::AdminUserWrite
                    ),
                    "preset {} grants an admin capability",
                    preset.id
                );
            }
        }
    }

    #[test]
    fn read_only_presets_contain_no_write_capability() {
        for id in ["sync_client_ro", "webdav_ro", "readonly_all"] {
            let preset = PRESETS.iter().find(|p| p.id == id).expect("preset");
            for capability in preset.capabilities {
                assert!(
                    !capability.is_write(),
                    "read-only preset {id} contains {}",
                    capability.id()
                );
            }
        }
    }

    #[test]
    fn placeholder_positions_do_not_match_empty_segments() {
        assert!(pattern_matches(
            "/api2/repos/{repo_id}/dir/",
            "/api2/repos/abc/dir/"
        ));
        assert!(!pattern_matches(
            "/api2/repos/{repo_id}/dir/",
            "/api2/repos//dir/"
        ));
        assert!(!pattern_matches(
            "/api2/repos/{repo_id}/dir/",
            "/api2/repos/abc/dir/detail/"
        ));
    }

    #[test]
    fn catch_all_matches_one_or_more_segments() {
        assert!(pattern_matches(
            "/repos/{repo_id}/files/{*path}",
            "/repos/abc/files/a/b.txt"
        ));
        assert!(pattern_matches(
            "/repos/{repo_id}/files/{*path}",
            "/repos/abc/files/one"
        ));
        assert!(
            !pattern_matches("/repos/{repo_id}/files/{*path}", "/repos/abc/files/"),
            "the catch-all must consume at least one segment, like axum's"
        );
        assert_eq!(
            required_access(&Method::GET, "/repos/abc/files/a/b.txt"),
            Some(RouteAccess::Capability(Capability::FileRead))
        );
    }

    #[test]
    fn specific_route_wins_over_a_broader_one() {
        // `/api/v2.1/repos/{repo_id}/file/` (write/delete) must not be
        // shadowed by `/api/v2.1/repos/{repo_id}/dir/`, and vice versa.
        assert_eq!(
            required_access(&Method::DELETE, "/api/v2.1/repos/abc/file/"),
            Some(RouteAccess::Capability(Capability::FileDelete))
        );
        assert_eq!(
            required_access(&Method::POST, "/api/v2.1/repos/abc/file/"),
            Some(RouteAccess::Capability(Capability::FileWrite))
        );
    }

    #[test]
    fn head_follows_the_get_route() {
        assert_eq!(
            required_access(&Method::HEAD, "/api2/repos/abc/dir/"),
            Some(RouteAccess::Capability(Capability::FileRead))
        );
    }

    #[test]
    fn unknown_and_out_of_scope_paths_return_none() {
        assert_eq!(
            required_access(&Method::GET, "/seafhttp/repo/x/commit/HEAD"),
            None
        );
        assert_eq!(required_access(&Method::GET, "/dav/repo/"), None);
        assert_eq!(required_access(&Method::GET, "/api2/not-a-route/"), None);
        assert_eq!(required_access(&Method::GET, "/api20/repos/"), None);
    }

    #[test]
    fn key_management_is_session_only() {
        for (method, path) in [
            (Method::GET, "/api2/api-keys/"),
            (Method::POST, "/api2/api-keys/"),
            (Method::GET, "/api2/api-keys/catalog/"),
            (Method::GET, "/api2/api-keys/7/"),
            (Method::PUT, "/api2/api-keys/7/"),
            (Method::DELETE, "/api2/api-keys/7/"),
        ] {
            assert_eq!(
                required_access(&method, path),
                Some(RouteAccess::SessionOnly),
                "the key API must never be reachable with a key ({method} {path})"
            );
        }
    }

    #[test]
    fn capability_minting_routes_are_write_level() {
        for path in [
            "/api2/repo-tokens/",
            "/api2/repos/abc/download-info/",
            "/api2/repos/abc/upload-link/",
            "/api2/repos/abc/update-link/",
            "/api2/repos/abc/upload-blks-link/",
            "/api2/repos/abc/update-blks-link/",
        ] {
            let access = required_access(&Method::GET, path).expect("classified");
            match access {
                RouteAccess::Capability(capability) => assert!(
                    capability.is_write(),
                    "{path} must require a write-level capability"
                ),
                other => panic!("{path} has unexpected access {other:?}"),
            }
        }
    }
}
