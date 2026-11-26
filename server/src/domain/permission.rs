//! The repository-access scope shared by every listing and link surface.
//!
//! The checks that read the database live in `crate::service::permission`: this
//! module holds only the value type, so `domain` stays free of repository and
//! infrastructure references.
/// The libraries an account-wide query may return. A session (or a key bound to
/// every library) is unrestricted; a bound key narrows every listing to its own
/// libraries, so `starred`, `activities`, `search` and the link lists cannot
/// describe libraries it was not given.
#[derive(Clone, Debug, Default)]
pub struct RepoScope(Option<std::collections::HashSet<String>>);

impl RepoScope {
    /// No narrowing: every library the account can reach.
    pub fn all() -> Self {
        Self(None)
    }

    /// Narrow to an explicit set of library ids.
    pub fn only(ids: impl IntoIterator<Item = String>) -> Self {
        Self(Some(ids.into_iter().collect()))
    }

    pub fn allows(&self, repo_id: &str) -> bool {
        match &self.0 {
            None => true,
            Some(allowed) => allowed.contains(repo_id),
        }
    }

    pub fn intersect(
        &self,
        ids: std::collections::HashSet<String>,
    ) -> std::collections::HashSet<String> {
        match &self.0 {
            None => ids,
            Some(allowed) => ids.into_iter().filter(|id| allowed.contains(id)).collect(),
        }
    }

    /// Whether this scope imposes any restriction at all.
    pub fn is_unrestricted(&self) -> bool {
        self.0.is_none()
    }
}
