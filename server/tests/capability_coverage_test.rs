//! Route-coverage guard for the unified API-key capability table.
//!
//! `required_access` fails closed: a `/api2` or `/api/v2.1` route that is not in
//! the table is denied to API keys. That is the right default, but it is also
//! silent, so this test pins the inventory: every route the server registers
//! must be classified.

use axum::http::Method;
use server::domain::capability::{RouteAccess, required_access};

/// Every `/api2` and `/api/v2.1` route the server registers, with the method a
/// client would use. Kept literal (rather than derived) on purpose: it is the
/// independent copy that catches a table entry disappearing.
#[rustfmt::skip]
const ROUTES: &[(&str, &str)] = &[
    // Authentication plumbing.
    ("POST",   "/api2/auth-token/"),
    ("GET",    "/api2/auth/ping/"),
    ("POST",   "/api2/logout-device/"),
    ("GET",    "/api2/ping/"),
    ("GET",    "/api2/server-info/"),
    ("POST",   "/api2/device-wiped/"),
    ("POST",   "/api2/client-sso-link/"),
    ("GET",    "/api2/client-sso-link/abc/"),
    ("POST",   "/api2/client-login/"),
    // Account, avatar, devices, notifications.
    ("GET",    "/api2/account/info/"),
    ("PUT",    "/api2/account/info/"),
    ("POST",   "/api2/account/"),
    ("POST",   "/api2/accounts/"),
    ("GET",    "/api2/avatars/user/a@b.c/resized/96/"),
    ("POST",   "/api/v2.1/user-avatar/"),
    ("GET",    "/api2/devices/"),
    ("DELETE", "/api2/devices/"),
    ("GET",    "/api2/unseen_messages/"),
    // Library metadata.
    ("GET",    "/api2/repos/"),
    ("POST",   "/api2/repos/"),
    ("GET",    "/api/v2.1/repos/"),
    ("GET",    "/api2/repos/{repo_id}/"),
    ("POST",   "/api2/repos/{repo_id}/"),
    ("DELETE", "/api2/repos/{repo_id}/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/"),
    ("DELETE", "/api/v2.1/repos/{repo_id}/"),
    ("GET",    "/api2/default-repo/"),
    ("POST",   "/api2/default-repo/"),
    ("GET",    "/api2/repos/{repo_id}/dir/sub_repo/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/set-password/"),
    ("PUT",    "/api/v2.1/repos/{repo_id}/set-password/"),
    ("GET",    "/api/v2.1/deleted-repos/"),
    ("POST",   "/api/v2.1/deleted-repos/"),
    ("DELETE", "/api/v2.1/deleted-repos/"),
    ("DELETE", "/api/v2.1/deleted-repos/{repo_id}/"),
    // Directories and files.
    ("GET",    "/api2/repos/{repo_id}/dir/"),
    ("POST",   "/api2/repos/{repo_id}/dir/"),
    ("DELETE", "/api2/repos/{repo_id}/dir/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/dir/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/dir/"),
    ("DELETE", "/api/v2.1/repos/{repo_id}/dir/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/dir/detail/"),
    ("POST",   "/api2/repos/{repo_id}/dir/move/"),
    ("POST",   "/api2/repos/{repo_id}/dir/rename/"),
    ("GET",    "/api2/repos/{repo_id}/dir/shared_items/"),
    ("GET",    "/api2/repos/{repo_id}/file/"),
    ("POST",   "/api2/repos/{repo_id}/file/"),
    ("PUT",    "/api2/repos/{repo_id}/file/"),
    ("DELETE", "/api2/repos/{repo_id}/file/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/file/"),
    ("DELETE", "/api/v2.1/repos/{repo_id}/file/"),
    ("POST",   "/api2/repos/{repo_id}/file/rename/"),
    ("POST",   "/api2/repos/{repo_id}/file/move/"),
    ("GET",    "/api2/repos/{repo_id}/file/detail/"),
    ("GET",    "/api2/repos/{repo_id}/file/exif/"),
    ("GET",    "/api2/repos/{repo_id}/file/index-text/"),
    ("POST",   "/api2/repos/{repo_id}/file/reindex/"),
    ("GET",    "/api2/repos/{repo_id}/thumbnail/"),
    ("GET",    "/api2/repos/{repo_id}/file-uploaded-bytes/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/file-uploaded-bytes/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/zip-task/"),
    // Capability-URL minting.
    ("GET",    "/api2/repos/{repo_id}/upload-link/"),
    ("GET",    "/api2/repos/{repo_id}/update-link/"),
    ("GET",    "/api2/repos/{repo_id}/upload-blks-link/"),
    ("GET",    "/api2/repos/{repo_id}/update-blks-link/"),
    ("GET",    "/api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/"),
    // Batch operations.
    ("POST",   "/api2/repos/{repo_id}/fileops/delete/"),
    ("POST",   "/api2/repos/{repo_id}/fileops/copy/"),
    ("POST",   "/api2/repos/{repo_id}/fileops/move/"),
    ("POST",   "/api/v2.1/repos/sync-batch-move-item/"),
    ("POST",   "/api/v2.1/repos/sync-batch-copy-item/"),
    ("POST",   "/api/v2.1/repos/batch-delete-item/"),
    ("DELETE", "/api/v2.1/repos/batch-delete-item/"),
    ("POST",   "/api/v2.1/repos/async-batch-copy-item/"),
    ("POST",   "/api/v2.1/repos/async-batch-move-item/"),
    ("POST",   "/api/v2.1/copy-move-task/"),
    ("GET",    "/api/v2.1/query-copy-move-progress/"),
    // Trash and history.
    ("GET",    "/api/v2.1/repos/{repo_id}/trash/"),
    ("DELETE", "/api/v2.1/repos/{repo_id}/trash/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/trash/revert-dirents/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/trash2/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/trash2/search/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/trash2/revert/"),
    ("GET",    "/api2/repo_history_changes/{repo_id}/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/file/history/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/file/revision/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/file/revision/restore/"),
    // Links, members, groups.
    ("GET",    "/api2/shared-links/"),
    ("POST",   "/api2/shared-links/"),
    ("DELETE", "/api2/shared-links/{token}"),
    ("GET",    "/api/v2.1/share-links/"),
    ("POST",   "/api/v2.1/share-links/"),
    ("POST",   "/api/v2.1/multi-share-links/"),
    ("GET",    "/api/v2.1/share-links/{token}/"),
    ("PUT",    "/api/v2.1/share-links/{token}/"),
    ("DELETE", "/api/v2.1/share-links/{token}/"),
    ("GET",    "/api2/upload-links/"),
    ("POST",   "/api2/upload-links/"),
    ("DELETE", "/api2/upload-links/{token}"),
    ("GET",    "/api/v2.1/upload-links/"),
    ("POST",   "/api/v2.1/upload-links/"),
    ("DELETE", "/api/v2.1/upload-links/clean-invalid/"),
    ("GET",    "/api/v2.1/upload-links/{token}/"),
    ("PUT",    "/api/v2.1/upload-links/{token}/"),
    ("DELETE", "/api/v2.1/upload-links/{token}/"),
    ("GET",    "/api/v2.1/upload-links/{token}/upload/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/upload-links/"),
    ("GET",    "/api2/beshared-repos/{repo_id}/"),
    ("POST",   "/api2/beshared-repos/{repo_id}/"),
    ("PUT",    "/api2/beshared-repos/{repo_id}/"),
    ("DELETE", "/api2/beshared-repos/{repo_id}/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/related-users/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/custom-share-permissions/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/custom-share-permissions/{permission_id}/"),
    ("GET",    "/api2/groups/"),
    ("GET",    "/api2/groupandcontacts/"),
    ("GET",    "/api2/search-user/"),
    ("GET",    "/api/v2.1/groups/"),
    // Metadata and tags.
    ("GET",    "/api/v2.1/repos/{repo_id}/metadata/"),
    ("PUT",    "/api/v2.1/repos/{repo_id}/metadata/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/metadata/record/"),
    ("PUT",    "/api/v2.1/repos/{repo_id}/metadata/record/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/metadata/tags/"),
    ("POST",   "/api/v2.1/repos/{repo_id}/metadata/tags/"),
    ("PUT",    "/api/v2.1/repos/{repo_id}/metadata/tags/"),
    ("DELETE", "/api/v2.1/repos/{repo_id}/metadata/tags/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/metadata/tags-status/"),
    ("PUT",    "/api/v2.1/repos/{repo_id}/metadata/tags-status/"),
    ("DELETE", "/api/v2.1/repos/{repo_id}/metadata/tags-status/"),
    ("GET",    "/api/v2.1/repos/{repo_id}/metadata/tag-files/{tag_id}/"),
    ("PUT",    "/api/v2.1/repos/{repo_id}/metadata/file-tags/"),
    // Discovery.
    ("GET",    "/api2/starredfiles/"),
    ("GET",    "/api/v2.1/starred-items/"),
    ("POST",   "/api/v2.1/starred-items/"),
    ("DELETE", "/api/v2.1/starred-items/"),
    ("GET",    "/api/v2.1/activities/"),
    ("GET",    "/api2/search/"),
    ("GET",    "/api/v2.1/search-file/"),
    ("GET",    "/api/v2.1/search-file"),
    ("POST",   "/api2/reindex/"),
    ("GET",    "/api2/reindex-progress/"),
    ("POST",   "/api2/index-file-text/"),
    // Sync-token issuance and misc.
    ("GET",    "/api2/repo-tokens/"),
    ("GET",    "/api2/repos/{repo_id}/download-info/"),
    ("GET",    "/api/v2.1/smart-link/"),
    ("GET",    "/api2/admin/users/"),
    ("POST",   "/api2/admin/users/"),
    ("PUT",    "/api2/admin/users/{user_id}/"),
    ("DELETE", "/api2/admin/users/{user_id}/"),
    // Unified API-key management (session-only).
    ("GET",    "/api2/api-keys/"),
    ("POST",   "/api2/api-keys/"),
    ("GET",    "/api2/api-keys/catalog/"),
    ("GET",    "/api2/api-keys/{key_id}/"),
    ("PUT",    "/api2/api-keys/{key_id}/"),
    ("DELETE", "/api2/api-keys/{key_id}/"),
];

fn method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("valid method")
}

/// Every route in the inventory must have a classification.
#[test]
fn every_registered_route_is_classified() {
    let mut unclassified = Vec::new();
    for (verb, path) in ROUTES {
        if required_access(&method(verb), path).is_none() {
            unclassified.push(format!("{verb} {path}"));
        }
    }
    assert!(
        unclassified.is_empty(),
        "routes missing from the capability table: {unclassified:#?}"
    );
}

/// The inventory itself must stay anchored to the router: every `/api2` or
/// `/api/v2.1` literal in `routes.rs` has to match the shape of some classified
/// route. A new mount point added there without a table entry fails here.
#[test]
fn router_literals_are_covered_by_the_inventory() {
    let source = include_str!("../src/routes.rs");
    let mut missing = Vec::new();
    for literal in api_literals(source) {
        if !ROUTES.iter().any(|(_, path)| shape_covers(&literal, path)) {
            missing.push(literal);
        }
    }
    assert!(
        missing.is_empty(),
        "routes.rs literals with no classified route: {missing:#?}"
    );
}

/// Whether `literal` names a prefix of `path`, treating `{...}` on either side
/// as "any single segment".
///
/// Both sides mix conventions — `routes.rs` writes `{repo_id}` while the
/// inventory writes `{repo_id}` for path parameters but concrete values for
/// tokens — so placeholders are wildcards on both sides. This is a drift guard
/// (did a mount point gain or lose coverage?), not an equality check.
fn shape_covers(literal: &str, path: &str) -> bool {
    let literal: Vec<&str> = literal.trim_matches('/').split('/').collect();
    let path: Vec<&str> = path.trim_matches('/').split('/').collect();
    if literal.len() > path.len() {
        return false;
    }
    literal.iter().zip(path.iter()).all(|(lit, pat)| {
        let is_param = |s: &&str| s.starts_with('{') && s.ends_with('}');
        is_param(lit) || is_param(pat) || lit == pat
    })
}

/// Pull `"/api2..."` / `"/api/v2.1..."` string literals out of a Rust source.
fn api_literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            let literal = &after[..end];
            if literal.starts_with("/api2") || literal.starts_with("/api/v2.1") {
                out.push(literal.to_string());
            }
            rest = &after[end + 1..];
        }
    }
    out
}

/// The inventory must not silently shrink.
#[test]
fn inventory_size_guard() {
    assert!(
        ROUTES.len() >= 140,
        "route inventory shrank to {} entries; update it deliberately",
        ROUTES.len()
    );
}

/// Spot-check that a read-only-looking route is not classified as a write and
/// that the key API is unreachable with a key.
#[test]
fn sensitive_routes_have_the_expected_access() {
    assert_eq!(
        required_access(&Method::GET, "/api2/repos/abc/dir/"),
        Some(RouteAccess::Capability(
            server::domain::capability::Capability::FileRead
        ))
    );
    assert_eq!(
        required_access(&Method::DELETE, "/api2/repos/abc/file/"),
        Some(RouteAccess::Capability(
            server::domain::capability::Capability::FileDelete
        ))
    );
    assert_eq!(
        required_access(&Method::GET, "/api2/api-keys/"),
        Some(RouteAccess::SessionOnly)
    );
}
