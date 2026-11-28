//! Contract for the Nanofile-only `/api/` namespace.
//!
//! Historical background: this server used to serve its own additions (API keys,
//! reindex controls, rename/move helpers, library visibility, …) on the Seafile
//! prefixes `/api2/` and `/api/v2.1/`. That mixed two contracts on one namespace:
//! a client reading the official Seafile API could not tell an upstream route from
//! a Nanofile invention. The routes now live under `/api/`.
//!
//! The transfer surface got the same treatment: the session upload/update forms,
//! the token download and the progress shims used to sit at the root beside the
//! fileserver's own paths; they live under `/api/transfers/` now. Two shape
//! fixes followed the same rule: the avatar image moved to `/api/avatars/` (it
//! is not a Seafile path), and the block URL took the fileserver's two-segment
//! shape, with the earlier self-contained spelling moved to
//! `/api/transfers/blks/`.
//!
//! These tests pin both splits:
//! * every moved method+path answers `404` (no alias, no redirect); the avatar
//!   image is the exception, answering a permanent redirect because its URLs are
//!   embedded in pages and clients,
//! * none of them is still listed in the capability route table,
//! * every new method+path exists and still demands its credential,
//! * the Seafile paths that merely shared a prefix (`/api2/account/info/`,
//!   `/api2/repo-tokens/`, …) or the root (`/upload-api/{token}`, …) are untouched.

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
    ("DELETE", "/api/shared-links/{token}"),
    ("GET", "/api/upload-links"),
    ("POST", "/api/upload-links"),
    ("DELETE", "/api/upload-links/{token}"),
    ("POST", "/api/accounts/"),
    ("GET", "/api/admin/users/"),
    ("POST", "/api/admin/users/"),
    ("PUT", "/api/admin/users/{user_id}/"),
    ("DELETE", "/api/admin/users/{user_id}/"),
    ("PUT", "/api/repos/{repo_id}/visibility/"),
];

/// Routes this server invented and has since deleted in favour of the Seafile
/// route that already does the same thing. They answer 404: no alias.
const REPLACED_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/repos/{repo_id}/file/rename/"),
    ("POST", "/api/repos/{repo_id}/file/move/"),
    ("POST", "/api/repos/{repo_id}/dir/rename/"),
    ("POST", "/api/repos/{repo_id}/dir/move/"),
    ("GET", "/api/repos/{repo_id}/file/revision/"),
    ("POST", "/api/repos/{repo_id}/file/revision/restore/"),
];

/// The Seafile-compatible routes those paths were replaced by. Each is added as
/// its replacement lands; the list stays empty only while a phase touches no
/// classified route.
const REPLACEMENT_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api2/repos/{repo_id}/file/revision/"),
    ("POST", "/api2/repos/{repo_id}/file/"),
];

/// Every method+path that moved off the root transfer surface.
///
/// These were Nanofile inventions staged beside the fileserver's own routes
/// (`/upload-api/{token}`, `/zip/{token}`, `/repos/.../files/...`); no Seafile
/// route table registers them.
const MOVED_TRANSFER_ROUTES: &[(&str, &str)] = &[
    ("GET", "/download-api/{token}"),
    ("POST", "/upload-aj/"),
    ("POST", "/update-aj/"),
    ("POST", "/update-api/"),
    ("GET", "/upload_progress"),
    ("GET", "/idx_progress"),
];

/// The same endpoints under `/api/transfers/`; the two session update spellings
/// were merged into one handler.
const NEW_TRANSFER_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/transfers/download/{token}"),
    ("POST", "/api/transfers/upload/"),
    ("POST", "/api/transfers/update/"),
    ("GET", "/api/transfers/upload-progress/"),
    ("GET", "/api/transfers/index-progress/"),
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
    for (method, path) in REPLACED_ROUTES {
        assert!(
            !has(method, path),
            "{method} {path} is still classified after it was replaced"
        );
    }
    for (method, path) in REPLACEMENT_ROUTES {
        assert!(
            has(method, path),
            "{method} {path} (the replacement route) is missing from the capability table"
        );
    }
}

/// The replaced routes are gone: the Seafile route that does the same thing is
/// the only spelling, and a stale client gets the router's 404 rather than a
/// silent alias.
#[tokio::test]
async fn replaced_routes_answer_not_found() {
    let f = TestFixture::new().await;
    for (method, template) in REPLACED_ROUTES {
        let path = expand(template, &f.repo_id);
        let resp = send(&f, method, &path, Some(&f.api_token)).await;
        assert_eq!(
            resp.status(),
            404,
            "{method} {path} must be gone after its replacement landed, body: {:?}",
            resp.text().await
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

/// The fileserver's own transfer paths stay at the root: moving the Nanofile-only
/// endpoints must not have dragged the Seafile ones along.
#[tokio::test]
async fn seafile_transfer_paths_stay_at_the_root() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?from=web", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-link is a seahub route");
    let upload_url: String = resp.json().await.unwrap();
    assert!(
        upload_url.contains("/upload-aj/"),
        "the browser upload link still points at the fileserver path, got: {upload_url}"
    );

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "seed.txt", b"seed")
        .await;
    assert_eq!(resp.status(), 200, "seeding the file must succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/seed.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "file download link is a seahub route");
    let download_url: String = resp.json().await.unwrap();
    assert!(
        download_url.contains("/api/transfers/download/"),
        "the download URL is minted in the Nanofile namespace, got: {download_url}"
    );
}

/// The avatar image bytes are this server's own (seahub serves them from
/// `/media/avatars/…`), so the image route moved under `/api/avatars/`: the JSON
/// endpoint mints the new path, and the old root spelling answers a permanent
/// redirect rather than the 404 the other moves answer — the URLs are embedded
/// in pages and clients, and a browser holding one must keep rendering it.
#[tokio::test]
async fn the_avatar_image_lives_on_the_api_namespace() {
    let f = TestFixture::new().await;

    let inventory: Vec<(&str, &str)> = route_inventory().collect();
    let classified = inventory.iter().any(|(method, path)| {
        *method == "GET" && template(path) == template("/api/avatars/user/{email}/resized/{size}/")
    });
    assert!(
        classified,
        "the avatar image route must be classified, or the fail-closed guard denies it"
    );

    let resp = f
        .client
        .get(
            &format!("/api2/avatars/user/{}/resized/80/", f.email),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let url = body["url"].as_str().expect("the response carries the url");
    assert!(
        url.contains("/api/avatars/user/"),
        "the avatar URL must live in the API namespace, got: {url}"
    );

    let ui = common::client::TestClient::new_ui_session(&f.server.base_url);
    let resp = ui
        .get(&format!("/avatars/user/{}/resized/80/", f.email), None)
        .await;
    assert_eq!(resp.status(), 308, "the old image path must redirect");
    assert!(
        resp.headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|location| location.starts_with("/api/avatars/user/")),
        "the redirect must land on the /api/ avatar route, got: {:?}",
        resp.headers().get("location")
    );
}

/// The moved transfer routes are gone: the root belongs to the fileserver's own
/// route table again, and no alias or redirect answers for them.
#[tokio::test]
async fn moved_transfer_routes_answer_not_found() {
    let f = TestFixture::new().await;
    for (method, template) in MOVED_TRANSFER_ROUTES {
        let path = expand(template, &f.repo_id);
        let resp = send(&f, method, &path, Some(&f.api_token)).await;
        assert_eq!(
            resp.status(),
            404,
            "{method} {path} must be gone from the root transfer surface, body: {:?}",
            resp.text().await
        );
    }
}

/// The new transfer routes are classified; the old spellings must not linger in
/// the table, or the fail-closed default would deny the new paths.
#[test]
fn transfer_routes_are_classified_on_the_new_namespace() {
    let inventory: Vec<(&str, &str)> = route_inventory().collect();
    let has = |method: &str, path: &str| {
        inventory
            .iter()
            .any(|(m, p)| *m == method && template(p) == template(path))
    };
    for (method, path) in MOVED_TRANSFER_ROUTES {
        assert!(
            !has(method, path),
            "{method} {path} is still classified after the move"
        );
    }
    for (method, path) in NEW_TRANSFER_ROUTES {
        assert!(
            has(method, path),
            "{method} {path} is missing from the capability table"
        );
    }
}

/// Each new transfer route answers on `/api/transfers/` with its own credential
/// contract: the invalid capability token is a 400 (not the fallback 404), the
/// session forms redirect to the login page, and the progress shims answer 200.
#[tokio::test]
async fn new_transfer_routes_exist_on_the_new_namespace() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get("/api/transfers/download/not-a-token", None)
        .await;
    assert_eq!(
        resp.status(),
        400,
        "the download endpoint must validate its capability token"
    );

    // A browser without a cookie is redirected to the login page; the shared test
    // client follows redirects, so use the no-redirect flavour here.
    let ui = common::client::TestClient::new_ui_session(&f.server.base_url);
    for path in ["/api/transfers/upload/", "/api/transfers/update/"] {
        let resp = ui.post_form(path, None, &[]).await;
        assert_eq!(
            resp.status(),
            303,
            "{path} must reject a sessionless request with the login redirect"
        );
        assert!(
            resp.headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|loc| loc.starts_with("/accounts/login/")),
            "{path} must redirect to the login page, got: {:?}",
            resp.headers().get("location")
        );
    }

    for path in [
        "/api/transfers/upload-progress/",
        "/api/transfers/index-progress/",
    ] {
        let resp = f.client.get(path, None).await;
        assert_eq!(resp.status(), 200, "{path} must serve the progress shim");
    }
}
