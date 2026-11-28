//! Path redaction for request logs.
//!
//! Capability URLs carry their credential in the path, so the tracing span and the
//! route-audit warning both go through this one function: a path logged verbatim
//! would be a replay.
/// Path segments immediately preceding a capability token. Routes such as
/// `/f/{token}` and `/zip/{token}` carry the credential itself in the path, and
/// it is deliberately unpaired with a session, so a logged URI is a replay.
const TOKEN_PATH_PREFIXES: &[&str] = &[
    "f",
    "d",
    "u",
    "zip",
    "blks",
    // `/api/transfers/download/{token}`; the generic segment also catches a
    // logged download path, which is a harmless over-redaction.
    "download",
    "download-api",
    "upload-api",
    "upload-aj",
    "update-api",
    "update-aj",
    "upload-blks-api",
    "update-blks-api",
    "upload-raw-blks-api",
    "client-sso",
    "client-login",
    "client-sso-link",
    // Capability URLs the API itself mints and returns.
    "share-links",
    "shared-links",
    "upload-links",
    // One-shot account credentials carried in a browser path.
    "reset",
    "verify-email",
];

/// Replace capability-token path segments with `{token}` for logging. The query
/// string is dropped by the caller; it may carry a share-link password or other
/// credentials.
pub fn redact_request_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let mut out = String::with_capacity(path.len());
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        let prev = if i > 0 { segs[i - 1] } else { "" };
        if !seg.is_empty() && TOKEN_PATH_PREFIXES.contains(&prev) {
            out.push_str("{token}");
        } else {
            out.push_str(seg);
        }
    }
    out
}
