//! What authenticated a request, and the authorization that follows from it.
//!
//! Two questions have to be answered for every request: *who is calling*, and
//! *what is this caller allowed to do*. Before this type existed a single field
//! answered both, and answered the second one by omission:
//!
//! ```ignore
//! pub struct AuthUser {
//!     // `None` meant both "this is a session" and "this credential is not
//!     // subject to the capability table".
//!     pub key: Option<KeyAuthority>,
//! }
//! ```
//!
//! Two meanings behind one absence is easy to get wrong, and the route guard
//! could only be applied to the `Some` half — a session silently skipped it.
//! Naming the credential makes each rule explicit and applies the route table
//! to every caller:
//!
//! * [`Credential::Session`] *is* the account, so it satisfies every capability
//!   — but it is also the only credential the key-management surface accepts,
//!   because a key that could mint keys could grant itself more than it holds.
//! * [`Credential::Key`] carries exactly what was granted to it, plus a
//!   per-library ceiling that narrows a route's requirement further.
//! * [`Credential::SyncToken`] is bound to one library by construction and is
//!   never accepted by the API surfaces; it is refused rather than silently
//!   treated as an account credential.

use crate::domain::api_key::KeyAuthority;
use crate::domain::capability::RouteAccess;
use crate::domain::permission::RepoScope;

/// How a request authenticated.
#[derive(Clone, Debug)]
pub enum Credential {
    /// A browser session, or the account-level token a client obtained by
    /// logging in.
    ///
    /// This is the account itself: it satisfies every capability, and it is the
    /// only credential allowed on the key-management surface.
    Session,
    /// A unified API key, created in *Settings → API Keys*.
    ///
    /// A key is a subset of its owner's account: the capability set it carries,
    /// narrowed further per library by its bindings.
    Key(KeyAuthority),
    /// A repository-scoped sync token, issued by `GET /api2/repo-tokens/`.
    ///
    /// Accepted only by the sync protocol (`/seafhttp/…`), which checks its
    /// library binding itself. The API surfaces refuse it outright: a leaked
    /// library credential must not read the whole account.
    SyncToken,
}

impl Credential {
    /// The key behind the request, when there is one.
    pub fn key(&self) -> Option<&KeyAuthority> {
        match self {
            Credential::Key(authority) => Some(authority),
            Credential::Session | Credential::SyncToken => None,
        }
    }

    /// A short, stable label for logs.
    pub fn kind_id(&self) -> &'static str {
        match self {
            Credential::Session => "session",
            Credential::Key(_) => "key",
            Credential::SyncToken => "sync_token",
        }
    }

    /// Whether this credential satisfies what a route requires.
    ///
    /// This is the whole of the "which credential may reach which route" policy;
    /// [`crate::middleware::auth::enforce_route_access`] adds the per-library
    /// ceiling on top.
    pub fn allows_route(&self, access: RouteAccess) -> bool {
        match access {
            // The handler authenticates the request itself, or the answer is
            // the same for every caller that got this far.
            RouteAccess::Public | RouteAccess::AnyAuthenticated => true,
            // Key management is reserved to the account itself: a key that
            // could mint keys could widen its own access. A sync token is not
            // an account credential and is refused here too.
            RouteAccess::SessionOnly => matches!(self, Credential::Session),
            // A session is the account, so it satisfies every capability. A key
            // must actually carry the capability it is asked for.
            RouteAccess::Capability(capability) => match self {
                Credential::Key(authority) => authority.has(capability),
                Credential::Session => true,
                Credential::SyncToken => false,
            },
        }
    }

    /// Whether the per-library ceiling allows this request.
    ///
    /// Only keys carry a ceiling; every other credential is the account itself.
    pub fn allows_repo(&self, repo_id: &str, need_write: bool) -> bool {
        match self {
            Credential::Key(authority) => authority.allows_repo(repo_id, need_write),
            // A sync token is bound to exactly one library by construction, and
            // the sync extractor enforces that binding before this is reached.
            Credential::Session | Credential::SyncToken => true,
        }
    }

    /// The libraries an account-wide listing may return.
    pub fn repo_scope(&self) -> RepoScope {
        match self {
            // A key bound to specific libraries narrows every listing its owner
            // makes through it, so a "list everything" page cannot leak the ids
            // of libraries the key was not given.
            Credential::Key(authority) if !authority.all_repos => {
                RepoScope::only(authority.bound_repo_ids().map(str::to_string))
            }
            Credential::Session | Credential::Key(_) | Credential::SyncToken => RepoScope::all(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::capability::Capability;
    use crate::repository::api_key::ApiKeyLookup;
    use infra::entity::{api_key, api_key_repo};

    fn authority(capabilities: &str, all_repos: bool, bindings: &[(&str, &str)]) -> KeyAuthority {
        KeyAuthority::from_lookup(&ApiKeyLookup {
            key: api_key::Model {
                id: 1,
                user_id: 7,
                name: "k".to_string(),
                key_hash: "0".repeat(64),
                key_prefix: None,
                capabilities: capabilities.to_string(),
                all_repos,
                created_at: 0,
                expires_at: None,
                last_used_at: None,
            },
            bindings: bindings
                .iter()
                .map(|(repo_id, permission)| api_key_repo::Model {
                    id: 1,
                    key_id: 1,
                    repo_id: (*repo_id).to_string(),
                    permission: (*permission).to_string(),
                })
                .collect(),
        })
        .expect("authority")
    }

    /// The account itself satisfies every capability — the route table says
    /// what a *key* needs, not what a session needs.
    #[test]
    fn a_session_satisfies_every_capability() {
        let session = Credential::Session;
        for capability in Capability::ALL {
            assert!(
                session.allows_route(RouteAccess::Capability(capability)),
                "session must satisfy {capability:?}"
            );
        }
        assert!(session.allows_route(RouteAccess::Public));
        assert!(session.allows_route(RouteAccess::AnyAuthenticated));
    }

    /// Only the account itself may manage keys: a key must not be able to mint
    /// a key with wider access than it holds.
    #[test]
    fn only_a_session_may_manage_keys() {
        assert!(Credential::Session.allows_route(RouteAccess::SessionOnly));
        assert!(
            !Credential::Key(authority("key.read,key.write", true, &[]))
                .allows_route(RouteAccess::SessionOnly)
        );
        assert!(!Credential::SyncToken.allows_route(RouteAccess::SessionOnly));
    }

    #[test]
    fn a_key_needs_the_capability_it_is_asked_for() {
        let key = Credential::Key(authority("file.read", true, &[]));
        assert!(key.allows_route(RouteAccess::Capability(Capability::FileRead)));
        assert!(!key.allows_route(RouteAccess::Capability(Capability::FileWrite)));
        // Public/any-authenticated routes do not consume a capability.
        assert!(key.allows_route(RouteAccess::Public));
        assert!(key.allows_route(RouteAccess::AnyAuthenticated));
    }

    /// A repository-scoped token is not an account credential: if it ever
    /// reached the API surfaces it would be refused, not promoted.
    #[test]
    fn a_sync_token_satisfies_no_capability() {
        let token = Credential::SyncToken;
        for capability in Capability::ALL {
            assert!(!token.allows_route(RouteAccess::Capability(capability)));
        }
    }

    #[test]
    fn only_keys_carry_a_library_ceiling() {
        let bound = Credential::Key(authority("file.read", false, &[("a", "rw")]));
        assert!(bound.allows_repo("a", true));
        assert!(!bound.allows_repo("b", false));

        let unlimited = Credential::Key(authority("file.read", true, &[]));
        assert!(unlimited.allows_repo("anything", true));

        // Neither a session nor a sync token has a ceiling.
        assert!(Credential::Session.allows_repo("anything", true));
        assert!(Credential::SyncToken.allows_repo("anything", true));
    }

    /// A bound key narrows account-wide listings to its own libraries; every
    /// other credential leaves them unrestricted.
    #[test]
    fn only_a_bound_key_narrows_the_listing_scope() {
        let bound = Credential::Key(authority("library.read", false, &[("a", "r"), ("b", "rw")]));
        let scope = bound.repo_scope();
        assert!(scope.allows("a"));
        assert!(scope.allows("b"));
        assert!(!scope.allows("c"));
        assert!(!scope.is_unrestricted());

        assert!(
            Credential::Key(authority("library.read", true, &[]))
                .repo_scope()
                .is_unrestricted()
        );
        assert!(Credential::Session.repo_scope().is_unrestricted());
        assert!(Credential::SyncToken.repo_scope().is_unrestricted());
    }

    #[test]
    fn kinds_are_labelled_for_logs() {
        assert_eq!(Credential::Session.kind_id(), "session");
        assert_eq!(Credential::SyncToken.kind_id(), "sync_token");
        assert_eq!(
            Credential::Key(authority("file.read", true, &[])).kind_id(),
            "key"
        );
        assert!(
            Credential::Key(authority("file.read", true, &[]))
                .key()
                .is_some()
        );
        assert!(Credential::Session.key().is_none());
    }
}
