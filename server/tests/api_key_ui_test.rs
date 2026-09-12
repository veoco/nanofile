//! Server-rendered API-key settings page.
//!
//! The page has to work without JavaScript, so these drive the plain HTML forms
//! the way a browser without scripts would: fetch the CSRF token from the page,
//! POST the form, and read the result back out of the returned HTML.

mod common;

use common::TestFixture;

/// A cookie-holding client (the Web UI authenticates with a session cookie).
async fn login_client(fixture: &TestFixture) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", fixture.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login for test helper");
    client
}

/// Read the hidden CSRF token out of an authenticated page.
async fn csrf_token(client: &reqwest::Client, url: &str) -> String {
    let body = client.get(url).send().await.unwrap().text().await.unwrap();
    let marker = r#"name="csrf_token" value=""#;
    body.find(marker)
        .and_then(|i| {
            let rest = &body[i + marker.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_settings_page_links_to_api_keys() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = client
        .get(format!("{}/settings/", f.server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains(r#"href="/settings/api-keys/""#));
}

#[tokio::test]
async fn a_key_can_be_created_edited_and_revoked_from_the_page() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let page = format!("{}/settings/api-keys/", f.server.base_url);

    let csrf = csrf_token(&client, &page).await;
    assert!(!csrf.is_empty(), "the page must embed a CSRF token");

    // Create: one checkbox per granted capability, one selector per library.
    let resp = client
        .post(format!("{}/settings/api-keys/create/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("name", "page-key"),
            ("cap__file.read", "1"),
            ("cap__file.write", "1"),
            (&format!("repo__{}", f.repo_id), "rw"),
            ("expiry", "30"),
        ])
        .send()
        .await
        .unwrap();
    // POST/Redirect/GET: creation answers with a redirect, so a browser
    // refresh re-reads the list instead of minting another key.
    assert_eq!(resp.status(), 303, "creation redirects to the list");
    let location = resp
        .headers()
        .get("location")
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with("/settings/api-keys/?created="));

    // Following the redirect shows the plaintext exactly once.
    let body = client
        .get(format!("{}{}", f.server.base_url, location))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let marker = r#"id="new-key-value" type="text" readonly value=""#;
    let start = body.find(marker).expect("shown-once box") + marker.len();
    let end = body[start..].find('"').unwrap() + start;
    let secret = body[start..end].to_string();
    assert_eq!(secret.len(), 40, "the page must show the plaintext once");
    assert!(body.contains("page-key"), "the new key must be listed");

    // Re-reading the reveal URL, or the plain page, must not show it again.
    for url in [format!("{}{}", f.server.base_url, location), page.clone()] {
        let reloaded = client.get(&url).send().await.unwrap().text().await.unwrap();
        assert!(!reloaded.contains(&secret), "the secret is shown once");
        assert!(reloaded.contains("page-key"), "the key itself remains");
    }

    // Edit: narrow it to read-only and rename it.
    let reloaded = client
        .get(&page)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let key_id = reloaded
        .split(r#"/settings/api-keys/"#)
        .find_map(|chunk| {
            let chunk = chunk.strip_prefix("")?;
            let id = chunk.split('/').next()?;
            id.parse::<i32>().ok()
        })
        .expect("a key id in the page");
    let csrf = csrf_token(&client, &page).await;
    let resp = client
        .post(format!(
            "{}/settings/api-keys/{key_id}/update/",
            f.server.base_url
        ))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("name", "page-key-2"),
            ("cap__file.read", "1"),
            (&format!("repo__{}", f.repo_id), "r"),
            ("expiry", "7"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "a successful update redirects");

    let body = client
        .get(&page)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("page-key-2"));

    // Revoke: the key disappears.
    let csrf = csrf_token(&client, &page).await;
    let resp = client
        .post(format!(
            "{}/settings/api-keys/{key_id}/revoke/",
            f.server.base_url
        ))
        .form(&[("csrf_token", csrf.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);

    let body = client
        .get(&page)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!body.contains("page-key-2"), "the revoked key is gone");
}

#[tokio::test]
async fn validation_errors_are_shown_instead_of_creating_a_key() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let page = format!("{}/settings/api-keys/", f.server.base_url);
    let csrf = csrf_token(&client, &page).await;

    // No capability selected: the service rejects it and the page explains.
    let resp = client
        .post(format!("{}/settings/api-keys/create/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("name", "empty"),
            ("expiry", "7"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("capability"),
        "the reason must be shown: {body}"
    );
    assert!(
        !body.contains(r#"id="new-key-value""#),
        "no key was created"
    );
}

#[tokio::test]
async fn the_page_requires_a_session() {
    let f = TestFixture::new().await;
    // No cookie: the UI redirects to the login page rather than serving data.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client
        .get(format!("{}/settings/api-keys/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303, "the UI redirects to the login page");
}

#[tokio::test]
async fn changing_the_password_revokes_every_key() {
    let f = TestFixture::new().await;

    // A key with an `/api2` capability and a WebDAV-scoped one: both are
    // credentials the account holds, so both must die with the password.
    let resp = f
        .client
        .post_json(
            "/api2/api-keys/",
            Some(&f.api_token),
            &serde_json::json!({
                "name": "before-change",
                "capabilities": ["library.read", "webdav.read"],
                "all_repos": true,
                "expires_in_days": 7,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let key = resp.json::<serde_json::Value>().await.unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(f.client.get("/api2/repos/", Some(&key)).await.status(), 200);

    let client = login_client(&f).await;
    let csrf = csrf_token(&client, &format!("{}/settings/", f.server.base_url)).await;
    let resp = client
        .post(format!("{}/settings/password/", f.server.base_url))
        .form(&[
            ("old_password", "password"),
            ("new_password", "newpass123"),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "password change failed");

    assert_eq!(
        f.client.get("/api2/repos/", Some(&key)).await.status(),
        401,
        "a key must not survive the password change that revoked it"
    );

    // And it is gone from the list rather than merely inert. The API token the
    // fixture held was revoked with the password too, so read the page through
    // the browser session the change preserved.
    let page = client
        .get(format!("{}/settings/api-keys/", f.server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!page.contains("before-change"), "the key must be revoked");
}
