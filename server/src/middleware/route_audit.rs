//! A roll-call of routes that a credential reached with no classification.
//!
//! The route table is fail-closed: a registered endpoint that nobody classified
//! answers 403. That is the right default, but on its own it is invisible — a
//! gap in the table is indistinguishable from a legitimate refusal, so the only
//! thing keeping the table complete is somebody noticing a log line.
//!
//! Recording the hit makes the gap observable. Two consumers rely on it:
//!
//! * the warning is emitted **once per route** instead of once per request, so
//!   a single unclassified hot endpoint cannot flood the log;
//! * the end-to-end run fails if the server reached one (see
//!   `e2e/global-teardown.ts`), which converts "someone should check the logs"
//!   into a gate.
//!
//! This is bookkeeping for a bug, not a feature: the set is expected to stay
//! empty, and it grows only when a registered route is missing from
//! [`crate::domain::capability::ROUTES`].

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// `(method, path)` pairs reached with no classification.
static UNCLASSIFIED: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Record a request that reached an unclassified route.
///
/// Returns `true` the first time a given `(method, path)` is seen, which is the
/// caller's cue to log it; later hits are remembered but stay quiet.
pub fn record(method: &str, path: &str) -> bool {
    let mut seen = UNCLASSIFIED.lock().unwrap_or_else(|e| e.into_inner());
    seen.insert((method.to_string(), path.to_string()))
}

/// Everything recorded so far, sorted so reports are stable.
pub fn recorded() -> Vec<(String, String)> {
    let seen = UNCLASSIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<(String, String)> = seen.iter().cloned().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The warning is emitted on the first hit and stays quiet after that, so a
    /// misclassified hot route cannot flood the log. Uses a path no other test
    /// touches, because the roll-call is process-wide.
    #[test]
    fn a_route_is_reported_once() {
        let path = "/api2/route-audit-once/";
        assert!(record("GET", path), "the first hit is the one to log");
        assert!(!record("GET", path), "later hits repeat what was logged");
        assert!(
            recorded().contains(&("GET".to_string(), path.to_string())),
            "the hit is remembered anyway"
        );
    }

    /// The method is part of the identity: two verbs on the same path are two
    /// separate gaps.
    #[test]
    fn the_method_is_part_of_the_identity() {
        let path = "/api2/route-audit-methods/";
        assert!(record("GET", path));
        assert!(record("POST", path));
    }
}
