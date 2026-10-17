//! A roll-call of routes that a credential reached with no classification.
//!
//! The route table is fail-closed, so an unclassified endpoint answers 403; on its
//! own that is invisible, a gap being indistinguishable from a legitimate refusal.
//! Recording the hit makes it observable: the warning is emitted **once per route**
//! so a hot endpoint cannot flood the log, and the end-to-end run fails if the
//! server reached one (see `e2e/global-teardown.ts`), turning "check the logs" into
//! a gate. The set is expected to stay empty: it grows only when a registered route
//! is missing from `crate::domain::capability::ROUTES`.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

static UNCLASSIFIED: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Record a request that reached an unclassified route, returning `true` the first
/// time a `(method, path)` is seen — the caller's cue to log it.
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

    /// The warning is emitted on the first hit only, so a misclassified hot route
    /// cannot flood the log; the path is unique because the roll-call is process-wide.
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

    /// The method is part of the identity: two verbs on one path are two gaps.
    #[test]
    fn the_method_is_part_of_the_identity() {
        let path = "/api2/route-audit-methods/";
        assert!(record("GET", path));
        assert!(record("POST", path));
    }
}
