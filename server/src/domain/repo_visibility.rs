//! The sharing scope an administrator sets on a library.
//!
//! The database stores the lowercase string (`repos.visibility`); this enum is
//! the only accepted vocabulary at the service and handler edges.
//!
//! * `Private` — only the owner can see it.
//! * `Shared`  — the accounts listed in `repo_members` can see it.
//! * `Public`  — every active account can read it, without a member row.
//!
//! `Shared` and `Public` are mutually exclusive: the handlers clear the member
//! list when a library becomes public, so there is one answer to "who can see
//! this".

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoVisibility {
    Private,
    Shared,
    Public,
}

impl RepoVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            RepoVisibility::Private => "private",
            RepoVisibility::Shared => "shared",
            RepoVisibility::Public => "public",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "private" => Some(RepoVisibility::Private),
            "shared" => Some(RepoVisibility::Shared),
            "public" => Some(RepoVisibility::Public),
            _ => None,
        }
    }

    pub const fn is_public(self) -> bool {
        matches!(self, RepoVisibility::Public)
    }
}

impl std::fmt::Display for RepoVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_stored_strings() {
        for visibility in [
            RepoVisibility::Private,
            RepoVisibility::Shared,
            RepoVisibility::Public,
        ] {
            assert_eq!(RepoVisibility::parse(visibility.as_str()), Some(visibility));
        }
        assert_eq!(RepoVisibility::parse("shared "), None);
        assert_eq!(RepoVisibility::parse(""), None);
    }
}
