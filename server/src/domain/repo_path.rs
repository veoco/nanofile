//! Repository id extraction from request paths.
//!
//! Two callers need different things from the same path shape:
//!
//! * `SyncAuth` (see [`crate::middleware::auth`]) resolves the repo a
//!   `/seafhttp/...` request is bound to, and must fail closed for anything
//!   that is not a known shape.
//! * The account-key guard needs a cheap "does this path mention a library?"
//!   answer so it can apply the key's library bindings before the handler runs.
//!
//! [`candidates`] keeps the sync-specific positions, [`find_repo_id`] answers
//! the general question. Both percent-decode first: axum's `Path` extractor
//! hands the handler the decoded segment, so comparing an undecoded value would
//! let `%63cab3e0-...` look like "not a repo id" while the handler still sees
//! the real library.

/// True when `s` is a UUID-formatted string (8-4-4-4-12 hex).
pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, &c)| i == 8 || i == 13 || i == 18 || i == 23 || c.is_ascii_hexdigit())
}

/// Candidate repo-id positions for a **trimmed** (no leading `/`) path, most
/// specific first: the full `seafhttp/repo/{id}` form, the `repo/{id}` form and
/// the stripped `{id}/...` form.
///
/// The last entry is always the path's first segment, which is what the caller
/// inspects to decide whether a non-UUID path is a known non-repo endpoint.
pub fn candidates(trimmed_path: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::with_capacity(3);
    for prefix in ["seafhttp/repo/", "repo/"] {
        if let Some(rest) = trimmed_path.strip_prefix(prefix)
            && let Some(seg) = rest.split('/').next()
        {
            out.push(seg);
        }
    }
    out.push(trimmed_path.split('/').next().unwrap_or(""));
    out
}

/// Percent-decode `path` and return its first UUID-formatted segment.
///
/// Account-key bindings are enforced on every `/api2` and `/api/v2.1` route
/// whose path carries a library id. In every such route the repo id is the
/// first UUID segment (later UUIDs are file ids); non-UUID parameters
/// (`{token}`, `{user_id}`, `{tag_id}`, a 40-hex `{block_id}`) never take that
/// shape, so this does not manufacture false positives.
///
/// Returns `None` when the path cannot be decoded or contains no UUID.
pub fn find_repo_id(path: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    decoded
        .split('/')
        .find(|seg| is_uuid(seg))
        .map(|seg| seg.to_string())
}

#[cfg(test)]
mod tests {
    use super::{candidates, find_repo_id, is_uuid};

    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";
    const FILE: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn uuid_shape_is_strict() {
        assert!(is_uuid(REPO));
        assert!(is_uuid(&REPO.to_uppercase()));
        for bad in [
            "",
            "not-a-uuid",
            "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290",  // 35 chars
            "cfcab3e0_9eb4_4c4f_92d0_87db2cd8290d", // wrong separators
            "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290g", // non-hex
        ] {
            assert!(!is_uuid(bad), "must reject {bad:?}");
        }
    }

    #[test]
    fn candidates_cover_the_three_sync_shapes() {
        // A matched prefix contributes the repo id, and the leading path segment
        // is appended last: callers read `[0]` for the id and the tail for the
        // "is this a known repo-less endpoint" check.
        assert_eq!(
            candidates(&format!("seafhttp/repo/{REPO}/commit/HEAD")),
            vec![REPO, "seafhttp"]
        );
        assert_eq!(
            candidates(&format!("repo/{REPO}/commit")),
            vec![REPO, "repo"]
        );
        assert_eq!(candidates(&format!("{REPO}/commit")), vec![REPO]);
        // No repo shape: only the leading segment, so the caller can still see
        // whether this is one of the protocol's repo-less endpoints.
        assert_eq!(candidates("accessible-repos"), vec!["accessible-repos"]);
    }

    #[test]
    fn find_repo_id_reads_api_paths() {
        assert_eq!(
            find_repo_id(&format!("/api2/repos/{REPO}/dir/")),
            Some(REPO.to_string())
        );
        assert_eq!(
            find_repo_id(&format!("/api/v2.1/repos/{REPO}/trash2/revert/")),
            Some(REPO.to_string())
        );
        // The library id comes before any file id, so the first UUID wins.
        assert_eq!(
            find_repo_id(&format!(
                "/api2/repos/{REPO}/files/{FILE}/blks/{}/download-link/",
                "a".repeat(40)
            )),
            Some(REPO.to_string())
        );
    }

    #[test]
    fn find_repo_id_percent_decodes() {
        let encoded = format!("%63{}", &REPO[1..]);
        assert_ne!(encoded, REPO);
        assert_eq!(
            find_repo_id(&format!("/api2/repos/{encoded}/dir/")),
            Some(REPO.to_string())
        );
        // Double encoding must not resolve to a library.
        assert_eq!(
            find_repo_id(&format!("/api2/repos/%25{}/dir/", &REPO[1..])),
            None
        );
    }

    #[test]
    fn find_repo_id_ignores_non_repo_parameters() {
        for path in [
            "/api2/account/info/",
            "/api2/admin/users/7/",
            "/api2/repos/{repo_id}/metadata/tag-files/3/",
            "/api/v2.1/share-links/AbCdEfGhIjKlMnOpQrStUv",
            "/api2/server-info/",
        ] {
            assert_eq!(find_repo_id(path), None, "path={path}");
        }
    }
}
