//! Compatibility contracts for the duplicate-removal work.
//!
//! These tests pin the client-observable behavior that a refactor must preserve.
//! Where a test records a known Seafile divergence, the follow-up change that
//! aligns it updates the expectation in the same commit as the code change.

use crate::common::{TestFixture, create_test_user};

async fn extra_user_token(f: &TestFixture, email: &str) -> String {
    create_test_user(&f.server.db, email, "password").await;
    let resp = f.client.login(email, "password").await;
    assert_eq!(resp.status(), 200, "login for {email} failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// seahub's `FileView.delete` returns `{"success": true}` for a path that no
/// longer exists (`api2/endpoints/file.py:806`). nanofile currently answers 404;
/// the delete-status alignment changes this to the upstream idempotent success.
#[tokio::test]
async fn file_delete_of_a_missing_path_is_not_found_today() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/file/?p=/does-not-exist.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        404,
        "today deleting a missing file answers 404 instead of upstream success"
    );
}

/// seahub's `DirView.delete` returns 404 for a missing folder
/// (`api2/endpoints/dir.py:593-601`); the asymmetry with files is upstream
/// behavior, not an bug.
#[tokio::test]
async fn directory_delete_of_a_missing_path_is_not_found() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/dir/?p=/does-not-exist", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        404,
        "deleting a missing directory must answer 404"
    );
}

/// `ShareLink.delete` treats a missing token as success
/// (`api2/endpoints/share_links.py:794-800`).
#[tokio::test]
async fn share_link_delete_of_a_missing_token_succeeds() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .delete("/api/v2.1/share-links/no-such-token/", Some(&f.api_token))
        .await;
    assert_eq!(
        resp.status(),
        200,
        "deleting a missing share link must be treated as success"
    );
}

/// `ShareLink.delete` answers 403 for a foreign owner
/// (`api2/endpoints/share_links.py:801-804`).
#[tokio::test]
async fn share_link_delete_by_a_non_owner_is_forbidden() {
    let f = TestFixture::new().await;
    let upload = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "owned.txt", b"x")
        .await;
    assert_eq!(upload.status(), 200, "upload failed");

    let token = f.create_share_link("/owned.txt").await;
    let other = extra_user_token(&f, "compat-share@example.com").await;

    let resp = f
        .client
        .delete(&format!("/api/v2.1/share-links/{token}/"), Some(&other))
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a foreign share-link delete must be forbidden"
    );

    // The link must still resolve after the rejected delete.
    let view = f.client.get(&format!("/f/{token}/?dl=1"), None).await;
    assert_eq!(
        view.status(),
        200,
        "the link must survive the rejected delete"
    );
}

/// Upload links follow the same idempotent delete: a missing token is success.
#[tokio::test]
async fn upload_link_delete_of_a_missing_token_succeeds() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .delete("/api/v2.1/upload-links/no-such-token/", Some(&f.api_token))
        .await;
    assert_eq!(
        resp.status(),
        200,
        "deleting a missing upload link must be treated as success"
    );
}

/// Upload links already answer 403 for a foreign owner, matching
/// `UploadLink.delete` (`api2/endpoints/upload_links.py:446-470`). The two link
/// kinds must converge on this one.
#[tokio::test]
async fn upload_link_delete_by_a_non_owner_is_forbidden() {
    let f = TestFixture::new().await;
    let token = f.create_upload_link("/", None).await;
    let other = extra_user_token(&f, "compat-upload@example.com").await;

    let resp = f
        .client
        .delete(&format!("/api/v2.1/upload-links/{token}/"), Some(&other))
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a foreign upload-link delete must be forbidden"
    );
}

/// For a link without a password, seahub's `check_share_link_common` opens the
/// link before looking at the request method
/// (`seahub/share/models.py:119-124`). The POST path must redirect without
/// verifying anything or charging the password limiter.
#[tokio::test]
async fn passwordless_share_link_post_redirects_without_a_password() {
    let f = TestFixture::new().await;
    let upload = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "open.txt", b"x")
        .await;
    assert_eq!(upload.status(), 200, "upload failed");

    let token = f.create_share_link("/open.txt").await;
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let resp = browser
        .post(format!("{}/f/{token}/", f.server.base_url))
        .form(&[] as &[(&str, &str)])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        302,
        "a passwordless link POST is already open and redirects"
    );
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(location.ends_with(&format!("/f/{token}/")));

    // Repeated POSTs must not consume the per-IP password budget.
    for _ in 0..15 {
        let resp = f.client.post_form(&format!("/f/{token}/"), None, &[]).await;
        assert_ne!(
            resp.status(),
            429,
            "an open link must not consume the budget"
        );
    }
}

/// A protected link submitted with no password re-renders the form (200), like
/// upstream's `check_share_link_common`, rather than answering a raw 400.
#[tokio::test]
async fn protected_share_link_post_without_a_password_re_renders_the_form() {
    let f = TestFixture::new().await;
    let upload = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "secret.txt", b"x")
        .await;
    assert_eq!(upload.status(), 200, "upload failed");

    let token = f
        .create_share_link_with_password("/secret.txt", Some("pw"))
        .await;
    let resp = f.client.post_form(&format!("/f/{token}/"), None, &[]).await;
    assert_eq!(
        resp.status(),
        200,
        "an empty password submission re-renders the validation form"
    );
}

/// Object ids in the sync protocol are 40 lowercase hex (Seafile's
/// `is_object_id_valid`, `seafile-server/lib/utils.c:1132-1158`). The sync
/// entrypoints share the block-id predicate so an uppercase spelling cannot
/// reach an id lookup that only the lower-case store can serve.
#[tokio::test]
async fn sync_fs_id_list_rejects_uppercase_object_ids() {
    let f = TestFixture::new().await;
    let server_head = "A".repeat(40);

    let resp = f
        .client
        .get_sync(
            &format!(
                "/seafhttp/repo/{}/fs-id-list/?server-head={server_head}",
                f.repo_id
            ),
            &f.sync_token,
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "an uppercase object id must be rejected"
    );
}

/// `/api2/device-wiped/` accepts a form token when the Authorization header is
/// absent (the desktop Qt shape). A header that names our scheme but carries no
/// usable token is a credential failure: it must not silently switch to the
/// body field. This pins the stricter behavior the shared authorization parser
/// introduced.
#[tokio::test]
async fn device_wiped_rejects_a_malformed_authorization_header() {
    let f = TestFixture::new().await;

    let login = f
        .client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", &f.email),
                ("password", &f.password),
                ("platform", "android"),
                ("device_id", "compat-device"),
                ("device_name", "phone"),
            ],
        )
        .await;
    assert_eq!(login.status(), 200, "device login failed");
    let device_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let raw = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = raw
        .post(format!("{}/api2/device-wiped/", f.server.base_url))
        // "Token" with no separator: present but not a usable credential.
        .header("authorization", "Token")
        .form(&[("token", device_token.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "a malformed authorization header must not fall back to the form token"
    );
}
