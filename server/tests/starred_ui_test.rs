//! The server-rendered starred page must reflect current library access.
//!
//! A starred row outlives the membership that made it possible: after a member
//! is removed from a library, the row stays in the table. The page is built on
//! the server, so unlike the API it had no place to intersect the rows with the
//! libraries the viewer can still reach — and rendered the removed library's
//! name, its id and the starred path.

use crate::common;

use common::{TestFixture, add_repo_member, create_test_user, remove_repo_member};

/// Log in through the web login form, keeping the session cookie.
async fn login_client(fixture: &TestFixture, email: &str, password: &str) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", fixture.server.base_url))
        .form(&[("email", email), ("password", password)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login for {email}");
    client
}

#[tokio::test]
async fn a_starred_item_disappears_when_library_access_is_revoked() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "starred.txt", b"content")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let member_id = create_test_user(db, "member@example.com", "password2").await;
    add_repo_member(db, &f.repo_id, member_id, "rw").await;

    // Star it as the member while the membership still exists.
    let login = f.client.login("member@example.com", "password2").await;
    assert_eq!(login.status(), 200);
    let member_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let starred = f
        .client
        .post_json(
            "/api/v2.1/starred-items/",
            Some(&member_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/starred.txt"}),
        )
        .await;
    assert_eq!(starred.status(), 200, "star as the member");

    let browser = login_client(&f, "member@example.com", "password2").await;
    let page = browser
        .get(format!("{}/starred/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200, "starred page");
    let body = page.text().await.unwrap();
    assert!(
        body.contains("starred.txt"),
        "the member's starred file must be listed: {body}"
    );

    // Revoke the grant: the stale starred row must stop being rendered.
    remove_repo_member(db, &f.repo_id, member_id).await;
    let page = browser
        .get(format!("{}/starred/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200, "starred page after revocation");
    let body = page.text().await.unwrap();
    assert!(
        !body.contains("starred.txt"),
        "a library the member lost must not be rendered: {body}"
    );
}

#[tokio::test]
async fn the_starred_page_still_lists_accessible_items() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "visible.txt", b"content")
        .await;
    assert!(up.status().is_success(), "upload failed");
    let starred = f
        .client
        .post_json(
            "/api/v2.1/starred-items/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/visible.txt"}),
        )
        .await;
    assert_eq!(starred.status(), 200, "star failed");

    let browser = login_client(&f, &f.email, &f.password).await;
    let page = browser
        .get(format!("{}/starred/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200, "starred page");
    let body = page.text().await.unwrap();
    assert!(
        body.contains("visible.txt"),
        "an accessible starred file must be listed: {body}"
    );
}
