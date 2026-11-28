//! Contract for the Nanofile-only `/api/` namespace.
//!
//! Historical background: this server used to serve its own additions (API keys,
//! reindex controls, rename/move helpers, library visibility, …) on the Seafile
//! prefixes `/api2/` and `/api/v2.1/`. That mixed two contracts on one namespace:
//! a client reading the official Seafile API could not tell an upstream route from
//! a Nanofile invention. The routes now live under `/api/`.
//!
//! These tests pin the split:
//! * every moved method+path answers `404` (no alias, no redirect),
//! * none of them is still listed in the capability route table,
//! * every new method+path exists and still demands a credential,
//! * the Seafile paths that merely shared a prefix (`/api2/account/info/`,
//!   `/api2/repo-tokens/`, …) are untouched.

use crate::common;

use common::TestFixture;
use serde_json::json;
use server::domain::capability::route_inventory;

/// Every method+path that moved out of `/api2/` or `/api/v2.1/`.
///
/// `{repo_id}`/`{key_id}`/`{kind}`/`{id}`/`{user_id}`/`{token}` are expanded by
/// [`expand`] to values a handler would accept if the route still existed.
const OLD_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api2/api-keys/"),
    ("POST", "/api2/api-keys/"),
    ("GET", "/api2/api-keys/catalog/"),
    ("GET", "/api2/api-keys/{key_id}/"),
    ("PUT", "/api2/api-keys/{key_id}/"),
    ("DELETE", "/api2/api-keys/{key_id}/"),
    ("GET", "/api2/credentials/"),
    ("DELETE", "/api2/credentials/{kind}/{id}/"),
    ("POST", "/api2/reindex/"),
    ("GET", "/api2/reindex-progress/"),
    ("POST", "/api2/index-file-text/"),
    ("GET", "/api2/repos/{repo_id}/file/index-text/"),
    ("POST", "/api2/repos/{repo_id}/file/reindex/"),
    ("GET", "/api2/repos/{repo_id}/file/exif/"),
    ("POST", "/api2/repos/{repo_id}/file/rename/"),
    ("POST", "/api2/repos/{repo_id}/file/move/"),
    ("POST", "/api2/repos/{repo_id}/dir/rename/"),
    ("POST", "/api2/repos/{repo_id}/dir/move/"),
    ("DELETE", "/api2/shared-links/{token}"),
    ("GET", "/api2/upload-links"),
    ("POST", "/api2/upload-links"),
    ("DELETE", "/api2/upload-links/{token}"),
    ("POST", "/api2/account/"),
    ("POST", "/api2/accounts/"),
    ("GET", "/api2/admin/users/"),
    ("POST", "/api2/admin/users/"),
    ("PUT", "/api2/admin/users/{user_id}/"),
    ("DELETE", "/api2/admin/users/{user_id}/"),
    ("GET", "/api/v2.1/repos/{repo_id}/file/revision/"),
    ("POST", "/api/v2.1/repos/{repo_id}/file/revision/restore/"),
    ("PUT", "/api/v2.1/repos/{repo_id}/visibility/"),
];

/// The same 31 endpoints on their new home (the singular account spelling was
/// folded into `/api/accounts/`, so this list has 30 entries).
const NEW_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/keys/"),
    ("POST", "/api/keys/"),
    ("GET", "/api/keys/catalog/"),
    ("GET", "/api/keys/{key_id}/"),
    ("PUT", "/api/keys/{key_id}/"),
    ("DELETE", "/api/keys/{key_id}/"),
    ("GET", "/api/credentials/"),
    ("DELETE", "/api/credentials/{kind}/{id}/"),
    ("POST", "/api/reindex/"),
    ("GET", "/api/reindex-progress/"),
    ("POST", "/api/index-file-text/"),
    ("GET", "/api/repos/{repo_id}/file/index-text/"),
    ("POST", "/api/repos/{repo_id}/file/reindex/"),
    ("GET", "/api/repos/{repo_id}/file/exif/"),
    ("POST", "/api/repos/{repo_id}/file/rename/"),
    ("POST", "/api/repos/{repo_id}/file/move/"),
    ("POST", "/api/repos/{repo_id}/dir/rename/"),
    ("POST", "/api/repos/{repo_id}/dir/move/"),
    ("DELETE", "/api/shared-links/{token}"),
    ("GET", "/api/upload-links"),
    ("POST", "/api/upload-links"),
    ("DELETE", "/api/upload-links/{token}"),
    ("POST", "/api/accounts/"),
    ("GET", "/api/admin/users/"),
    ("POST", "/api/admin/users/"),
    ("PUT", "/api/admin/users/{user_id}/"),
    ("DELETE", "/api/admin/users/{user_id}/"),
    ("GET", "/api/repos/{repo_id}/file/revision/"),
    ("POST", "/api/repos/{repo_id}/file/revision/restore/"),
    ("PUT", "/api/repos/{repo_id}/visibility/"),
];

/// Expand the template placeholders to concrete segments.
fn expand(path: &str, repo_id: &str) -> String {
    path.replace("{repo_id}", repo_id)
        .replace("{key_id}", "1")
        .replace("{kind}", "session")
        .replace("{id}", "1")
        .replace("{user_id}", "1")
        .replace("{token}", "AbCdEf123456")
}

/// Send one request for the given method, using a JSON body for `POST`/`PUT`.
async fn send(f: &TestFixture, method: &str, path: &str, token: Option<&str>) -> reqwest::Response {
    match method {
        "GET" => f.client.get(path, token).await,
        "POST" => f.client.post_json(path, token, &json!({})).await,
        "PUT" => f.client.put_json(path, token, &json!({})).await,
        "DELETE" => f.client.delete(path, token).await,
        other => panic!("unexpected method {other} in the route table"),
    }
}

/// Collapse `{param}` names and the trailing slash: the capability table and the
/// axum router disagree on some names (`{id}` vs `{key_id}`), and a nested
/// collection router matches with or without the trailing slash depending on
/// how it was registered.
fn template(path: &str) -> String {
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
            }
            out.push_str("{}");
        } else {
            out.push(c);
        }
    }
    out
}

/// The moved routes are gone: no alias, no redirect, just the router's 404.
#[tokio::test]
async fn moved_routes_answer_not_found() {
    let f = TestFixture::new().await;
    for (method, template) in OLD_ROUTES {
        let path = expand(template, &f.repo_id);
        let resp = send(&f, method, &path, Some(&f.api_token)).await;
        assert_eq!(
            resp.status(),
            404,
            "{method} {path} must be gone from the Seafile namespace, body: {:?}",
            resp.text().await
        );
    }
}

/// The move is not shadowed by a stale capability-table entry.
#[test]
fn the_route_table_has_no_moved_path() {
    let inventory: Vec<(&str, &str)> = route_inventory().collect();
    let has = |method: &str, path: &str| {
        inventory
            .iter()
            .any(|(m, p)| *m == method && template(p) == template(path))
    };
    for (method, path) in OLD_ROUTES {
        assert!(
            !has(method, path),
            "{method} {path} is still classified after the move"
        );
    }
    // Every new route is classified, or API keys would be denied by the
    // fail-closed default instead of getting the documented capability.
    for (method, path) in NEW_ROUTES {
        assert!(
            has(method, path),
            "{method} {path} is missing from the capability table"
        );
    }
}

/// Each new route exists and is still protected: an anonymous request is a 401,
/// not the 404 a missing route would produce.
#[tokio::test]
async fn new_routes_exist_and_require_authentication() {
    let f = TestFixture::new().await;
    for (method, template) in NEW_ROUTES {
        let path = expand(template, &f.repo_id);
        let resp = send(&f, method, &path, None).await;
        assert_eq!(
            resp.status(),
            401,
            "{method} {path} must be registered and demand a credential, body: {:?}",
            resp.text().await
        );
    }
}

/// The handlers still work on the new namespace (a couple of safe spot checks).
#[tokio::test]
async fn new_namespace_serves_the_old_handlers() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api/keys/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200, "GET /api/keys/ must list keys");

    let resp = f.client.get("/api/credentials/", Some(&f.api_token)).await;
    assert_eq!(
        resp.status(),
        200,
        "GET /api/credentials/ must list the credential inventory"
    );

    // Account creation is admin-only; this fixture's account is a normal user,
    // so the handler rejects it — which still proves the route was reached.
    let resp = f
        .client
        .post_form("/api/accounts/", Some(&f.api_token), &[])
        .await;
    assert!(
        resp.status().is_client_error() && resp.status() != 404,
        "POST /api/accounts/ must reach the account-creation handler, got {}",
        resp.status()
    );
}

/// Seafile paths that merely shared the old prefixes stay exactly where they were.
#[tokio::test]
async fn seafile_paths_on_the_old_prefixes_are_untouched() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200, "account info is a Seafile route");

    let resp = f
        .client
        .get(
            &format!("/api2/repo-tokens/?repos={}", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "repo-tokens is a seahub legacy route");

    let resp = f.client.get("/api2/shared-links", Some(&f.api_token)).await;
    assert_eq!(
        resp.status(),
        200,
        "the v1 shared-links collection is a seahub route"
    );

    let resp = f
        .client
        .put_json(
            &format!("/api/repos/{}/visibility/", f.repo_id),
            Some(&f.api_token),
            &json!({ "visibility": "private" }),
        )
        .await;
    assert!(
        resp.status().is_client_error() && resp.status() != 404,
        "the visibility handler must live on /api/ now, got {}",
        resp.status()
    );
}
