//! Deciding whether an event is *new* enough to notify about.
//!
//! Both predicates are deliberately conservative: when the evidence cannot tell one device/browser
//! from another, nothing is sent — a missed notification is a nuisance, one per login trains the
//! owner to ignore them. They take the account's sessions as a slice, so they stay pure and testable.

use infra::entity::api_token;

use crate::domain::session_source::SessionSource;
use crate::ui::user_agent;

/// A stored session's source; an unrecognised value counts as a browser session — the safe
/// direction, since a browser session is the common case and the two checks never overlap.
fn source_of(row: &api_token::Model) -> SessionSource {
    SessionSource::from_id(&row.source).unwrap_or(SessionSource::Web)
}

fn trimmed(value: Option<&str>) -> &str {
    value.unwrap_or("").trim()
}

/// Whether this client device has never signed in before, matched on `(platform, device_id)` — the
/// same pair the credentials page groups sessions by.
///
/// A missing field means the client did not identify itself, and an unidentified client is not
/// evidence of a new device: the desktop "view on website" handoff and older clients land here.
pub fn is_new_device(prior: &[api_token::Model], platform: &str, device_id: &str) -> bool {
    let platform = platform.trim();
    let device_id = device_id.trim();
    if platform.is_empty() || device_id.is_empty() {
        return false;
    }
    !prior.iter().any(|row| {
        trimmed(row.platform.as_deref()) == platform
            && trimmed(row.device_id.as_deref()) == device_id
    })
}

/// Whether this browser has never signed in before.
///
/// Compares the human-readable label the credentials page shows (`user_agent::describe`), not the raw
/// header: a browser update changes that string on nearly every release, and notifying on it would be
/// spam. Only browser sessions count — a client's `User-Agent` is its HTTP library.
pub fn is_new_browser(prior: &[api_token::Model], current_agent: Option<&str>) -> bool {
    let Some(current_agent) = current_agent.filter(|value| !value.trim().is_empty()) else {
        // No `User-Agent` at all: every session would look alike, so "new" is undecidable.
        return false;
    };
    let current = user_agent::describe(current_agent);
    !prior
        .iter()
        .filter(|row| source_of(row).is_browser())
        .filter_map(|row| row.user_agent.as_deref())
        .filter(|agent| !agent.trim().is_empty())
        .any(|agent| user_agent::describe(agent) == current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(
        source: SessionSource,
        platform: Option<&str>,
        device_id: Option<&str>,
        user_agent: Option<&str>,
    ) -> api_token::Model {
        api_token::Model {
            id: 1,
            user_id: 1,
            token: "t".to_string(),
            created_at: 0,
            expires_at: None,
            device_id: device_id.map(str::to_string),
            platform: platform.map(str::to_string),
            device_name: None,
            client_version: None,
            is_pending: false,
            source: source.id().to_string(),
            user_agent: user_agent.map(str::to_string),
        }
    }

    const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:129.0) Gecko/20100101 Firefox/129.0";
    const CHROME: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/127.0.0.0 Safari/537.36";

    #[test]
    fn a_device_is_new_only_the_first_time() {
        let prior = vec![session(
            SessionSource::Client,
            Some("android"),
            Some("d1"),
            None,
        )];
        assert!(!is_new_device(&prior, "android", "d1"));
        assert!(is_new_device(&prior, "android", "d2"));
        assert!(is_new_device(&prior, "ios", "d1"));
        assert!(is_new_device(&[], "android", "d1"));
    }

    #[test]
    fn an_unidentified_client_is_never_a_new_device() {
        let prior = vec![session(
            SessionSource::Client,
            Some("android"),
            Some("d1"),
            None,
        )];
        assert!(!is_new_device(&prior, "", "d1"));
        assert!(!is_new_device(&prior, "android", ""));
        assert!(!is_new_device(&prior, "   ", "  "));
    }

    #[test]
    fn a_browser_is_new_only_the_first_time() {
        let prior = vec![session(SessionSource::Web, None, None, Some(FIREFOX))];
        let newer = FIREFOX.replace("129.0", "131.0");
        assert!(!is_new_browser(&prior, Some(&newer)));
        assert!(is_new_browser(&prior, Some(CHROME)));
        assert!(is_new_browser(&[], Some(CHROME)));
    }

    #[test]
    fn a_client_session_never_makes_a_browser_look_known() {
        // A client stores its HTTP library's agent; it must not pass for a browser fingerprint.
        let prior = vec![session(
            SessionSource::Client,
            Some("linux"),
            Some("d1"),
            Some(CHROME),
        )];
        assert!(is_new_browser(&prior, Some(CHROME)));
    }

    #[test]
    fn a_missing_user_agent_is_never_new() {
        let prior = vec![session(SessionSource::Web, None, None, Some(CHROME))];
        assert!(!is_new_browser(&prior, None));
        assert!(!is_new_browser(&prior, Some("   ")));
        assert!(!is_new_browser(&[], None));
    }

    #[test]
    fn the_client_login_handoff_counts_as_a_browser_session() {
        // It sets the session cookie from a real browser, so its agent does describe one.
        let prior = vec![session(
            SessionSource::WebClientLogin,
            None,
            None,
            Some(FIREFOX),
        )];
        assert!(!is_new_browser(&prior, Some(FIREFOX)));
    }
}
