//! Authorization model for unified API keys.
//!
//! A key carries two orthogonal things: the *capabilities* it may exercise, and
//! the *libraries* it may touch, each with a read/write ceiling. Effective access
//! is the strictest of the three inputs involved in a request: the route's
//! required capability, the key's per-library ceiling, and the caller's library
//! membership (checked separately by the auth layer).

use std::collections::HashMap;

use base::error::AppError;

use crate::domain::capability::{Capability, CapabilitySet};
use crate::repository::api_key::ApiKeyLookup;

/// Read/write ceiling a key carries for one library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepoCeiling {
    Read,
    Write,
}

impl RepoCeiling {
    /// Parse a persisted permission. Anything that is not exactly `"r"` is
    /// treated as `"rw"`, matching how the legacy WebDAV keys were stored.
    pub fn from_permission(permission: &str) -> Self {
        if permission == "r" {
            RepoCeiling::Read
        } else {
            RepoCeiling::Write
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RepoCeiling::Read => "r",
            RepoCeiling::Write => "rw",
        }
    }

    pub fn allows_write(self) -> bool {
        self == RepoCeiling::Write
    }
}

/// The authorization a presented API key carries, resolved once per request.
#[derive(Clone, Debug)]
pub struct KeyAuthority {
    pub key_id: i32,
    pub capabilities: CapabilitySet,
    /// The key applies to every library its owner can access; per-library
    /// ceilings then do not apply.
    pub all_repos: bool,
    pub repos: HashMap<String, RepoCeiling>,
}

impl KeyAuthority {
    /// Resolve a stored row into an authority.
    ///
    /// A stored capability id that is no longer in the catalog is an internal
    /// error rather than a silently dropped grant: a key must never end up
    /// *more* permissive than what the database says, and an unreadable grant is
    /// a data bug worth surfacing.
    pub fn from_lookup(lookup: &ApiKeyLookup) -> Result<Self, AppError> {
        let ids: Vec<String> = lookup
            .key
            .capabilities
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect();
        let capabilities = CapabilitySet::parse(&ids).map_err(|error| {
            AppError::Internal(format!("api key {} has {error}", lookup.key.id))
        })?;
        let mut repos = HashMap::with_capacity(lookup.bindings.len());
        for binding in &lookup.bindings {
            repos.insert(
                binding.repo_id.clone(),
                RepoCeiling::from_permission(&binding.permission),
            );
        }
        Ok(Self {
            key_id: lookup.key.id,
            capabilities,
            all_repos: lookup.key.all_repos,
            repos,
        })
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(capability)
    }

    /// Whether the key covers `repo_id`, optionally requiring write access.
    ///
    /// This is a scope check only: a key bound to a library the caller is not a
    /// member of still passes here and is rejected by the membership check.
    pub fn allows_repo(&self, repo_id: &str, need_write: bool) -> bool {
        if self.all_repos {
            return true;
        }
        match self.repos.get(repo_id) {
            Some(ceiling) => !need_write || ceiling.allows_write(),
            None => false,
        }
    }

    pub fn ceiling(&self, repo_id: &str) -> Option<RepoCeiling> {
        if self.all_repos {
            return Some(RepoCeiling::Write);
        }
        self.repos.get(repo_id).copied()
    }

    /// The repo ids this key is explicitly bound to (empty for `all_repos`).
    pub fn bound_repo_ids(&self) -> impl Iterator<Item = &str> {
        self.repos.keys().map(String::as_str)
    }
}

/// The strictest of a set of ceilings: any `Read` makes the result `Read`.
pub fn strictest(ceilings: impl IntoIterator<Item = RepoCeiling>) -> Option<RepoCeiling> {
    let mut result: Option<RepoCeiling> = None;
    for ceiling in ceilings {
        result = Some(match result {
            Some(current) => current.min(ceiling),
            None => ceiling,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::entity::{api_key, api_key_repo};

    fn lookup(capabilities: &str, all_repos: bool, bindings: &[(&str, &str)]) -> ApiKeyLookup {
        ApiKeyLookup {
            key: api_key::Model {
                id: 1,
                user_id: 7,
                name: "k".to_string(),
                key_hash: "0".repeat(64),
                key_prefix: Some("01234567".to_string()),
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
        }
    }

    #[test]
    fn bound_keys_only_cover_their_libraries() {
        let authority =
            KeyAuthority::from_lookup(&lookup("file.read,file.write", false, &[("a", "rw")]))
                .expect("authority");
        assert!(authority.allows_repo("a", true));
        assert!(authority.allows_repo("a", false));
        assert!(!authority.allows_repo("b", false));
        assert_eq!(authority.bound_repo_ids().collect::<Vec<_>>(), vec!["a"]);
    }

    #[test]
    fn read_ceiling_blocks_writes() {
        let authority = KeyAuthority::from_lookup(&lookup("file.write", false, &[("a", "r")]))
            .expect("authority");
        assert!(authority.allows_repo("a", false));
        assert!(!authority.allows_repo("a", true));
        assert!(authority.has(Capability::FileWrite));
    }

    #[test]
    fn all_repos_keys_cover_any_library() {
        let authority =
            KeyAuthority::from_lookup(&lookup("file.read", true, &[])).expect("authority");
        assert!(authority.allows_repo("anything", false));
        assert!(authority.allows_repo("anything", true));
        assert_eq!(authority.ceiling("anything"), Some(RepoCeiling::Write));
    }

    #[test]
    fn unknown_stored_capability_is_an_error() {
        assert!(
            KeyAuthority::from_lookup(&lookup("file.rede", false, &[("a", "rw")])).is_err(),
            "an unreadable grant must not be ignored"
        );
    }

    #[test]
    fn strictest_takes_the_most_restrictive() {
        assert_eq!(
            strictest([RepoCeiling::Write, RepoCeiling::Read]),
            Some(RepoCeiling::Read)
        );
        assert_eq!(strictest([RepoCeiling::Write]), Some(RepoCeiling::Write));
        assert_eq!(strictest([]), None);
    }
}
