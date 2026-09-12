//! Functional capability catalog for the unified API-key system.
//!
//! A key is not "an account token" or "a WebDAV token"; it is a set of
//! independently grantable capabilities, each naming one feature of the API.
//! The table in [`ROUTES`] maps every `/api2` and `/api/v2.1` route to the
//! capability it needs, so enforcement lives at one point
//! ([`crate::middleware::auth::AuthUser`]) instead of being re-stated by every
//! handler.
//!
//! Every credential is classified, not only keys. A session satisfies every
//! capability because it *is* the account, so what the table really records is
//! "what does this route need of a key", plus the routes that are closed to keys
//! altogether ([`RouteAccess::SessionOnly`]).
//!
//! Three properties matter and are covered by tests:
//!
//! * **Write implies read.** Granting `file.write` without `file.read` would
//!   produce a key that can upload but not list, so the implication closure is
//!   expanded when capabilities are parsed.
//! * **Unknown means denied.** [`required_access`] answering `None` for a route
//!   a caller is reaching is a hard 403, so a newly added route fails closed
//!   until it is classified.
//! * **Capabilities are stable ids.** They are persisted as a canonical
//!   comma-separated list, so the enum's discriminants are an in-memory detail
//!   while [`Capability::id`] is the on-disk contract.

use axum::http::Method;

/// One independently grantable feature of the API.
///
/// The declaration order defines the bit index in [`CapabilitySet`]; never
/// reorder or remove a variant without also migrating stored capability lists
/// (the persisted form is [`Capability::id`], not the discriminant, so
/// reordering is safe as long as ids stay).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    AccountRead,
    AccountWrite,
    DeviceRead,
    DeviceWrite,
    InvitationRead,
    InvitationWrite,
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
    HistoryManage,
    ShareLinkRead,
    ShareLinkWrite,
    UploadLinkRead,
    UploadLinkWrite,
    MemberRead,
    MemberWrite,
    GroupRead,
    MetadataRead,
    MetadataWrite,
    TagRead,
    TagWrite,
    StarRead,
    StarWrite,
    ActivityRead,
    SearchRead,
    SearchReindex,
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
    pub const ALL: [Capability; 45] = [
        Capability::AccountRead,
        Capability::AccountWrite,
        Capability::DeviceRead,
        Capability::DeviceWrite,
        Capability::InvitationRead,
        Capability::InvitationWrite,
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
        Capability::HistoryManage,
        Capability::ShareLinkRead,
        Capability::ShareLinkWrite,
        Capability::UploadLinkRead,
        Capability::UploadLinkWrite,
        Capability::MemberRead,
        Capability::MemberWrite,
        Capability::GroupRead,
        Capability::MetadataRead,
        Capability::MetadataWrite,
        Capability::TagRead,
        Capability::TagWrite,
        Capability::StarRead,
        Capability::StarWrite,
        Capability::ActivityRead,
        Capability::SearchRead,
        Capability::SearchReindex,
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

    /// Stable identifier used in the database and over the API.
    pub const fn id(self) -> &'static str {
        match self {
            Capability::AccountRead => "account.read",
            Capability::AccountWrite => "account.write",
            Capability::DeviceRead => "device.read",
            Capability::DeviceWrite => "device.write",
            Capability::InvitationRead => "invitation.read",
            Capability::InvitationWrite => "invitation.write",
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
            Capability::HistoryManage => "history.manage",
            Capability::ShareLinkRead => "share_link.read",
            Capability::ShareLinkWrite => "share_link.write",
            Capability::UploadLinkRead => "upload_link.read",
            Capability::UploadLinkWrite => "upload_link.write",
            Capability::MemberRead => "member.read",
            Capability::MemberWrite => "member.write",
            Capability::GroupRead => "group.read",
            Capability::MetadataRead => "metadata.read",
            Capability::MetadataWrite => "metadata.write",
            Capability::TagRead => "tag.read",
            Capability::TagWrite => "tag.write",
            Capability::StarRead => "star.read",
            Capability::StarWrite => "star.write",
            Capability::ActivityRead => "activity.read",
            Capability::SearchRead => "search.read",
            Capability::SearchReindex => "search.reindex",
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

    /// Parse a persisted identifier.
    pub fn from_id(id: &str) -> Option<Capability> {
        Capability::ALL.iter().copied().find(|c| c.id() == id)
    }

    /// Index into [`CapabilitySet`]'s bit set.
    const fn bit(self) -> u128 {
        1u128 << (self as u32)
    }

    pub const fn domain(self) -> CapabilityDomain {
        match self {
            Capability::AccountRead | Capability::AccountWrite => CapabilityDomain::Account,
            Capability::DeviceRead | Capability::DeviceWrite => CapabilityDomain::Device,
            Capability::InvitationRead | Capability::InvitationWrite => {
                CapabilityDomain::Invitation
            }
            Capability::LibraryRead
            | Capability::LibraryCreate
            | Capability::LibraryUpdate
            | Capability::LibraryDelete
            | Capability::LibraryPassword => CapabilityDomain::Library,
            Capability::FileRead | Capability::FileWrite | Capability::FileDelete => {
                CapabilityDomain::File
            }
            Capability::TrashRead | Capability::TrashWrite => CapabilityDomain::Trash,
            Capability::HistoryRead | Capability::HistoryWrite | Capability::HistoryManage => {
                CapabilityDomain::History
            }
            Capability::ShareLinkRead
            | Capability::ShareLinkWrite
            | Capability::UploadLinkRead
            | Capability::UploadLinkWrite
            | Capability::MemberRead
            | Capability::MemberWrite
            | Capability::GroupRead => CapabilityDomain::Sharing,
            Capability::MetadataRead
            | Capability::MetadataWrite
            | Capability::TagRead
            | Capability::TagWrite => CapabilityDomain::Metadata,
            Capability::StarRead
            | Capability::StarWrite
            | Capability::ActivityRead
            | Capability::SearchRead
            | Capability::SearchReindex => CapabilityDomain::Discovery,
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
    ///
    /// Drives the "read-only" presets and the UI's read/write labelling; it is
    /// not itself an enforcement rule (enforcement is per route).
    pub const fn is_write(self) -> bool {
        !matches!(
            self,
            Capability::AccountRead
                | Capability::DeviceRead
                | Capability::InvitationRead
                | Capability::LibraryRead
                | Capability::FileRead
                | Capability::TrashRead
                | Capability::HistoryRead
                | Capability::ShareLinkRead
                | Capability::UploadLinkRead
                | Capability::MemberRead
                | Capability::GroupRead
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

    /// Whether this capability is granted to a non-admin user at all.
    ///
    /// `key.*` is reserved: a key may never manage keys, because a key that can
    /// mint keys can grant itself more than it holds. `admin.*` is grantable
    /// only by an admin (checked by the service, which knows the caller).
    pub const fn is_reserved_to_sessions(self) -> bool {
        matches!(self, Capability::KeyRead | Capability::KeyWrite)
    }
}

/// Capabilities implied by another capability.
///
/// The closure is applied on parse, so a stored list never needs to contain the
/// implied entries. Keeping "write implies read" here (rather than in the UI)
/// means a client that posts only `file.write` still gets `file.read`.
const IMPLICATIONS: &[(Capability, &[Capability])] = &[
    (Capability::AccountWrite, &[Capability::AccountRead]),
    (Capability::DeviceWrite, &[Capability::DeviceRead]),
    (Capability::InvitationWrite, &[Capability::InvitationRead]),
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
    (Capability::HistoryManage, &[Capability::HistoryRead]),
    (Capability::ShareLinkWrite, &[Capability::ShareLinkRead]),
    (Capability::UploadLinkWrite, &[Capability::UploadLinkRead]),
    (Capability::MemberRead, &[Capability::LibraryRead]),
    (Capability::MemberWrite, &[Capability::MemberRead]),
    (Capability::MetadataRead, &[Capability::LibraryRead]),
    (Capability::MetadataWrite, &[Capability::MetadataRead]),
    (Capability::TagRead, &[Capability::MetadataRead]),
    (Capability::TagWrite, &[Capability::TagRead]),
    (Capability::StarWrite, &[Capability::StarRead]),
    (Capability::SearchReindex, &[Capability::SearchRead]),
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
/// `u128` comfortably covers the catalog (45 today); adding a 129th capability
/// would be a compile-time error at [`Capability::bit`], which is the intended
/// prompt to widen the type together with the persisted format.
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

    /// Parse a persisted identifier list and expand the implication closure.
    ///
    /// An unknown id is an error rather than a silent drop: a typo must not
    /// produce a key that quietly lacks the access its creator intended.
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
        CapabilitySet::parse(&ids)
    }

    /// Render the canonical on-disk form.
    pub fn to_canonical(&self) -> String {
        self.to_ids().join(",")
    }
}

/// What a route requires of the caller.
///
/// This is the complete policy: it is consulted for every credential, not only
/// for keys (see `Credential::allows_route`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteAccess {
    /// No credential: a public endpoint, or a handler that authenticates the
    /// request itself (for example `POST /api2/device-wiped/`, which reads the
    /// token from either a form field or a header).
    Public,
    /// Any valid credential is enough. Used for probes such as
    /// `GET /api2/auth/ping/`, which only answers "is this token accepted".
    AnyAuthenticated,
    /// The account itself is required; API keys are rejected. Used for the
    /// authentication plumbing (login/logout, client login, SSO) and for key
    /// management itself: a key that can mint keys could grant itself more than
    /// it holds.
    SessionOnly,
    /// A key needs this capability; a session satisfies it because a session is
    /// the account, not a subset of it.
    Capability(Capability),
}

/// Route classification table.
///
/// `{name}` matches exactly one non-empty path segment. The table covers every
/// `/api2` and `/api/v2.1` route (see the coverage test in
/// `server/tests/capability_coverage_test.rs`); a route that is missing here is
/// denied to every credential, which is the intended failure direction.
#[allow(clippy::type_complexity)]
const ROUTES: &[(&str, &str, RouteAccess)] = &[
    // ── Authentication plumbing ──────────────────────────────────────────
    ("POST", "/api2/auth-token/", RouteAccess::Public),
    ("GET", "/api2/auth/ping/", RouteAccess::AnyAuthenticated),
    ("POST", "/api2/logout-device/", RouteAccess::SessionOnly),
    ("GET", "/api2/ping/", RouteAccess::Public),
    ("GET", "/api2/server-info/", RouteAccess::Public),
    // device-wiped reads the token from a form field or the Authorization
    // header itself, so it never reaches `AuthUser`.
    ("POST", "/api2/device-wiped/", RouteAccess::Public),
    ("POST", "/api2/client-sso-link/", RouteAccess::Public),
    ("GET", "/api2/client-sso-link/{token}/", RouteAccess::Public),
    ("POST", "/api2/client-login/", RouteAccess::SessionOnly),
    // ── Account / devices / notifications ────────────────────────────────
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
    (
        "GET",
        "/api2/unseen_messages/",
        RouteAccess::Capability(Capability::NotificationRead),
    ),
    // ── Library metadata ─────────────────────────────────────────────────
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
    // ── Files and directories ────────────────────────────────────────────
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
    // The web UI's file-content endpoint lives outside the API namespaces but
    // serves the same bytes as `GET /api2/repos/{id}/file/`, so it is a read.
    (
        "GET",
        "/repos/{repo_id}/files/{*path}",
        RouteAccess::Capability(Capability::FileRead),
    ),
    // ── Upload / download capability URLs ────────────────────────────────
    // These answer with a bearer URL (or a block link), so they are write-level
    // even though two of them are served on GET: a read-only key must not be
    // able to mint a credential that can upload.
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
    // ── Batch file operations ────────────────────────────────────────────
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
    // ── Trash and history ────────────────────────────────────────────────
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
    // ── Sharing: links, members, groups ──────────────────────────────────
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
        "/api2/beshared-repos/{repo_id}/",
        RouteAccess::Capability(Capability::MemberRead),
    ),
    (
        "POST",
        "/api2/beshared-repos/{repo_id}/",
        RouteAccess::Capability(Capability::MemberWrite),
    ),
    (
        "PUT",
        "/api2/beshared-repos/{repo_id}/",
        RouteAccess::Capability(Capability::MemberWrite),
    ),
    (
        "DELETE",
        "/api2/beshared-repos/{repo_id}/",
        RouteAccess::Capability(Capability::MemberWrite),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/related-users/",
        RouteAccess::Capability(Capability::MemberRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/custom-share-permissions/",
        RouteAccess::Capability(Capability::MemberRead),
    ),
    (
        "GET",
        "/api/v2.1/repos/{repo_id}/custom-share-permissions/{permission_id}/",
        RouteAccess::Capability(Capability::MemberRead),
    ),
    (
        "GET",
        "/api2/groups/",
        RouteAccess::Capability(Capability::GroupRead),
    ),
    (
        "GET",
        "/api2/groupandcontacts/",
        RouteAccess::Capability(Capability::GroupRead),
    ),
    (
        "GET",
        "/api2/search-user/",
        RouteAccess::Capability(Capability::GroupRead),
    ),
    (
        "GET",
        "/api/v2.1/groups/",
        RouteAccess::Capability(Capability::GroupRead),
    ),
    // ── Metadata and tags ────────────────────────────────────────────────
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
    // ── Discovery: starred, activities, search ───────────────────────────
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
        RouteAccess::Capability(Capability::SearchReindex),
    ),
    // ── Sync token issuance ──────────────────────────────────────────────
    // The minted sync token is a full read/write library credential, so
    // issuance is write-level: a read-only key must not be able to obtain one.
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
    // ── Misc ─────────────────────────────────────────────────────────────
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
    // ── Unified key management ───────────────────────────────────────────
    // Session-only, including the read-only catalog: a key that can mint keys
    // could grant itself more than it holds, so no part of the management
    // surface accepts key authentication.
    ("GET", "/api2/api-keys/catalog/", RouteAccess::SessionOnly),
    ("GET", "/api2/api-keys/", RouteAccess::SessionOnly),
    ("POST", "/api2/api-keys/", RouteAccess::SessionOnly),
    ("GET", "/api2/api-keys/{id}/", RouteAccess::SessionOnly),
    ("PUT", "/api2/api-keys/{id}/", RouteAccess::SessionOnly),
    ("DELETE", "/api2/api-keys/{id}/", RouteAccess::SessionOnly),
];

/// Classify a request path for the credential guard.
///
/// Returns `None` when the path is outside the classified API surface or is not
/// in [`ROUTES`]. Callers enforcing access must treat `None` as denied: a route
/// added without a classification then fails closed instead of silently
/// accepting every credential.
pub fn required_access(method: &Method, path: &str) -> Option<RouteAccess> {
    if !is_classified_prefix(path) {
        return None;
    }
    // Preflight requests are answered by the CORS layer and never reach a
    // handler with credentials; classifying them as "any credential" keeps a
    // stray OPTIONS from being reported as an unknown route.
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

fn is_classified_prefix(path: &str) -> bool {
    path == "/api2"
        || path.starts_with("/api2/")
        || path == "/api/v2.1"
        || path.starts_with("/api/v2.1/")
        // The unified web file-content endpoint is served outside the API
        // namespaces but also accepts an API key, so it has to be classified
        // rather than falling through to "deny".
        || path.starts_with("/repos/")
}

/// Whether `path` matches a `{name}`-style pattern.
///
/// `{name}` matches exactly one non-empty segment; a trailing `{*name}`
/// (axum's catch-all) matches one or more remaining segments.
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
            // A placeholder never matches an empty segment, so a doubled slash
            // cannot satisfy a parameter position.
            if actual.is_empty() {
                return false;
            }
        } else if segment != actual {
            return false;
        }
    }
    pattern_segments.len() == path_segments.len()
}

/// Prefer the pattern with more literal (non-placeholder) characters, so a
/// specific route registered after a broader one still wins.
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
/// The sync protocol has no route table of its own here (its paths arrive both
/// in full and prefix-stripped form, because axum's `nest` rewrites the URI), so
/// the classification is by method with an explicit list of read-only POSTs.
/// Anything unrecognised counts as a write, so a read-only key fails closed on a
/// new endpoint.
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
/// `PROPFIND`/`REPORT` are reads that a plain method check would miss; every
/// other method — including ones this server does not implement — counts as a
/// write, so a read-only key fails closed.
pub fn webdav_requires_write(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS" | "PROPFIND" | "REPORT")
}

/// A named starting point offered by the management UI.
pub struct Preset {
    pub id: &'static str,
    pub capabilities: &'static [Capability],
}

/// Presets are convenience only; they expand to ordinary capability sets, and
/// the user can edit any of them before saving.
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
        capabilities: &[
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::FileDelete,
            Capability::MetadataRead,
            Capability::WebdavRead,
            Capability::WebdavWrite,
        ],
    },
    Preset {
        id: "webdav_ro",
        capabilities: &[
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::MetadataRead,
            Capability::WebdavRead,
        ],
    },
    Preset {
        id: "ci_upload",
        capabilities: &[
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::UploadLinkRead,
            Capability::UploadLinkWrite,
            Capability::SyncRead,
            Capability::SyncToken,
        ],
    },
    Preset {
        id: "readonly_all",
        capabilities: &[
            Capability::AccountRead,
            Capability::DeviceRead,
            Capability::InvitationRead,
            Capability::LibraryRead,
            Capability::FileRead,
            Capability::TrashRead,
            Capability::HistoryRead,
            Capability::ShareLinkRead,
            Capability::UploadLinkRead,
            Capability::MemberRead,
            Capability::GroupRead,
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
            Capability::InvitationRead,
            Capability::InvitationWrite,
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
            Capability::HistoryManage,
            Capability::ShareLinkRead,
            Capability::ShareLinkWrite,
            Capability::UploadLinkRead,
            Capability::UploadLinkWrite,
            Capability::MemberRead,
            Capability::MemberWrite,
            Capability::GroupRead,
            Capability::MetadataRead,
            Capability::MetadataWrite,
            Capability::TagRead,
            Capability::TagWrite,
            Capability::StarRead,
            Capability::StarWrite,
            Capability::ActivityRead,
            Capability::SearchRead,
            Capability::SearchReindex,
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

    #[test]
    fn catalog_ids_are_unique_and_round_trip() {
        assert_eq!(Capability::ALL.len(), 45);
        let mut ids: Vec<&str> = Capability::ALL.iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Capability::ALL.len(), "duplicate capability id");
        for capability in Capability::ALL {
            assert_eq!(Capability::from_id(capability.id()), Some(capability));
        }
        assert_eq!(Capability::from_id("file.upload"), None);
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
