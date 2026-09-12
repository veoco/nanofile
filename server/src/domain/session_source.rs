//! Where an account token came from.
//!
//! `api_tokens` holds every account-level credential: browser sessions, client
//! logins, and the tokens handed to clients. They differ only in who holds them
//! and what they are for, and before this vocabulary existed that had to be
//! guessed from `platform`, which is set only when a client reports device
//! details. That guess was wrong in both directions — a browser session and a
//! client that sent no device info were indistinguishable, and the desktop
//! client's "view on website" handoff looked like a plain web login.
//!
//! The persisted form is [`SessionSource::id`], so the set can grow without
//! renumbering anything. A value this build does not recognise is treated as a
//! browser session by the inventory UI: the safe direction is to *show* a
//! credential in the wrong group, because it stays visible and revocable,
//! whereas hiding one leaves the owner unable to see or revoke it at all.

/// Which login produced an account token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSource {
    /// A browser login: the password form, a 2FA completion, or the automatic
    /// login that follows registration.
    Web,
    /// A browser session the desktop client opened through "view on website".
    /// It is a real browser session, but the user did not type a password to
    /// get it, so the inventory labels it.
    WebClientLogin,
    /// A client login through `POST /api2/auth-token/`.
    Client,
    /// The token minted for a client at the end of the SSO handshake.
    ClientSso,
}

impl SessionSource {
    /// Stable identifier, persisted in `api_tokens.source`.
    pub const fn id(self) -> &'static str {
        match self {
            SessionSource::Web => "web",
            SessionSource::WebClientLogin => "web_client_login",
            SessionSource::Client => "client",
            SessionSource::ClientSso => "client_sso",
        }
    }

    /// Parse a persisted identifier.
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "web" => Some(SessionSource::Web),
            "web_client_login" => Some(SessionSource::WebClientLogin),
            "client" => Some(SessionSource::Client),
            "client_sso" => Some(SessionSource::ClientSso),
            _ => None,
        }
    }

    /// Whether a browser holds this token (as a session cookie).
    pub const fn is_browser(self) -> bool {
        matches!(self, SessionSource::Web | SessionSource::WebClientLogin)
    }

    /// Whether a client application holds this token.
    pub const fn is_client(self) -> bool {
        matches!(self, SessionSource::Client | SessionSource::ClientSso)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for source in [
            SessionSource::Web,
            SessionSource::WebClientLogin,
            SessionSource::Client,
            SessionSource::ClientSso,
        ] {
            assert_eq!(SessionSource::from_id(source.id()), Some(source));
        }
        assert_eq!(SessionSource::from_id("web_sso"), None);
    }

    /// The two buckets the inventory groups by must be exhaustive and disjoint,
    /// because a token that lands in neither would silently disappear from the
    /// page — the exact failure this feature exists to fix.
    #[test]
    fn the_two_buckets_cover_every_source() {
        for source in [
            SessionSource::Web,
            SessionSource::WebClientLogin,
            SessionSource::Client,
            SessionSource::ClientSso,
        ] {
            assert_ne!(
                source.is_browser(),
                source.is_client(),
                "{} must be in exactly one bucket",
                source.id()
            );
        }
    }

    #[test]
    fn the_client_login_handoff_is_a_browser_session() {
        // It sets the `seahub-session` cookie, so it belongs with the browser
        // sessions; `is_client` would be the tempting but wrong answer.
        assert!(SessionSource::WebClientLogin.is_browser());
        assert!(SessionSource::ClientSso.is_client());
    }
}
