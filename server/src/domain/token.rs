//! Credential expiry rules shared by the header extractors and the services.
//!
//! The same rule must decide "is this token still usable?" on both sides of the
//! request boundary: the middleware rejects an expired credential before a handler
//! runs, and services that resolve tokens from a request body or a stored row must
//! apply the same test. Keeping it here means neither side can drift.

/// Check whether a token has expired: `expires_at` set and in the past.
pub fn is_token_expired(expires_at: Option<i64>) -> bool {
    matches!(expires_at, Some(exp) if chrono::Utc::now().timestamp() > exp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_expiry_never_expires() {
        assert!(!is_token_expired(None));
    }

    #[test]
    fn a_past_deadline_is_expired() {
        let now = chrono::Utc::now().timestamp();
        assert!(is_token_expired(Some(now - 1)));
    }

    #[test]
    fn a_future_deadline_is_still_valid() {
        let now = chrono::Utc::now().timestamp();
        assert!(!is_token_expired(Some(now + 60)));
    }
}
