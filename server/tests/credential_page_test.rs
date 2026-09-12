//! The credential inventory page.
//!
//! The page has to work without JavaScript, so these drive the plain HTML forms
//! the way a browser without scripts would: log in for a cookie, fetch the CSRF
//! token from the page, POST the form, and read the result back out of the HTML.
//!
//! What the tests are really pinning is *visibility*: a credential the owner
//! cannot see is one they cannot revoke, which is how a 365-day sync token used
//! to be.

mod common;

use common::TestFixture;

/// A cookie-holding client (the Web UI authenticates with a session cookie).
async fn login_client(fixture: &TestFixture) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .user_agent("NanofileTestBrowser/1.0")
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

async fn page(client: &reqwest::Client, base_url: &str) -> String {
    client
        .get(format!("{base_url}/settings/devices/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

/// The page with its `<script>` blocks removed.
///
/// The base template publishes the whole translation dictionary as a JSON data
/// block, so searching the raw body for an i18n key matches whether or not
/// anything actually rendered it — an assertion written that way passes
/// vacuously, and a section lookup lands inside the dictionary.
fn visible(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<script") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</script>") {
            Some(end) => rest = &rest[start + end + "</script>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Read the hidden CSRF token out of an authenticated page.
fn csrf_token(body: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    body.find(marker)
        .and_then(|i| {
            let rest = &body[i + marker.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .unwrap_or_default()
}

/// The `value` of the hidden input named `name` in the form that posts `kind`.
fn hidden_value(body: &str, kind: &str, field: &str) -> Option<String> {
    // Anchor on the form whose `kind` matches, then read `field` from it.
    let kind_marker = format!(r#"name="kind" value="{kind}""#);
    let start = body.find(&kind_marker)?;
    let form_start = body[..start].rfind("<form")?;
    let form_end = body[form_start..].find("</form>")? + form_start;
    let form = &body[form_start..form_end];
    let marker = format!(r#"name="{field}" value=""#);
    let at = form.find(&marker)? + marker.len();
    let rest = &form[at..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[tokio::test]
async fn every_kind_of_credential_is_listed() {
    let f = TestFixture::new().await;

    // A client that reports a device, so there is one to show.
    let resp = f
        .client
        .login_multipart("test@example.com", "password")
        .await;
    assert_eq!(resp.status(), 200);

    let client = login_client(&f).await;
    let body = visible(&page(&client, &f.server.base_url).await);

    // Read each section on its own: "test-repo" also names a library in the
    // left panel, so a whole-page search would prove nothing about the sync
    // token list.
    let clients = section(&body, "Client sessions", "Browser sessions");
    let browsers = section(&body, "Browser sessions", "Sync tokens");
    let tokens = section(&body, "Sync tokens", "Devices that skip two-factor");

    assert!(
        clients.contains("Test Device"),
        "the client session reports its device name"
    );
    assert!(
        browsers.contains("NanofileTestBrowser/1.0"),
        "the browser session is labelled by its user agent"
    );
    assert!(
        browsers.contains("This session"),
        "the session being used is marked"
    );
    assert!(
        tokens.contains("test-repo"),
        "the sync token names the library it belongs to"
    );
    assert!(
        body.contains("/settings/api-keys/"),
        "API keys are linked rather than re-implemented"
    );
}

/// The text between two section headings.
fn section<'a>(body: &'a str, heading: &str, next_heading: &str) -> &'a str {
    let start = body
        .find(heading)
        .unwrap_or_else(|| panic!("missing heading {heading}"));
    let end = body
        .find(next_heading)
        .unwrap_or_else(|| panic!("missing heading {next_heading}"));
    assert!(start < end, "{heading} must come before {next_heading}");
    &body[start..end]
}

/// The regression this page exists for: a client that reports no device details
/// used to be invisible, because the old query selected on `platform`.
#[tokio::test]
async fn a_client_without_device_details_is_still_listed() {
    let f = TestFixture::new().await;
    // The fixture's own login sends only a username and password.
    let client = login_client(&f).await;
    let body = visible(&page(&client, &f.server.base_url).await);

    // It shows up in the client section, unnamed, rather than nowhere.
    let clients_at = body.find("Client sessions").expect("the client heading");
    let browsers_at = body.find("Browser sessions").expect("the browser heading");
    assert!(clients_at < browsers_at, "sections are ordered");

    let client_section = &body[clients_at..browsers_at];
    assert!(
        client_section.contains("Unnamed device"),
        "the platform-less client is listed, shown as unnamed"
    );
}

#[tokio::test]
async fn a_sync_token_can_be_revoked_by_id() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;

    let id = hidden_value(&body, "sync_token", "id").expect("a sync-token revoke form");
    let resp = client
        .post(format!("{}/settings/devices/revoke/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf_token(&body)),
            ("kind", "sync_token".to_string()),
            ("id", id),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);

    // The row is gone from the database, not just from the page.
    assert!(
        f.server
            .repos
            .sync_token
            .list_for_user(f.user_id)
            .await
            .expect("list")
            .is_empty(),
        "the sync token was revoked"
    );

    let after = visible(&page(&client, &f.server.base_url).await);
    assert!(
        after.contains("No sync tokens."),
        "the page says there are none left"
    );
}

/// Signing out of the session you are using has to land on the login page,
/// because the page you were on can no longer be loaded.
#[tokio::test]
async fn revoking_the_current_session_returns_to_the_login_page() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;

    let id = hidden_value(&body, "browser_session", "id").expect("a browser-session revoke form");
    let resp = client
        .post(format!("{}/settings/devices/revoke/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf_token(&body)),
            ("kind", "browser_session".to_string()),
            ("id", id),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/accounts/login/",
        "the reader is sent back to the login page"
    );
    assert!(
        f.server
            .repos
            .api_token
            .list_sessions(f.user_id)
            .await
            .expect("list")
            .iter()
            .all(|session| session.source != "web"),
        "the browser session is gone"
    );
}

/// The ids travel through a page, so a foreign id must not be enough — not even
/// to learn whether it exists.
#[tokio::test]
async fn another_accounts_credential_cannot_be_revoked() {
    let f = TestFixture::new().await;
    let other =
        common::create_test_user(f.server.db.as_ref(), "other@example.com", "password").await;

    // A real sync token belonging to somebody else. It needs a real library
    // because the row has a foreign key to one; whose library it is does not
    // matter, since the scoping under test is by owner.
    let resp = f.client.create_repo(&f.api_token, "second").await;
    assert_eq!(resp.status(), 201);
    let other_repo = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .expect("repo id")
        .to_string();
    f.server
        .repos
        .sync_token
        .create(
            &other_repo,
            other,
            "peer-token".to_string(),
            Some("x".into()),
            1,
            None,
        )
        .await
        .expect("seed another account's sync token");
    let foreign_id = f
        .server
        .repos
        .sync_token
        .list_for_user(other)
        .await
        .expect("list")[0]
        .id;

    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;
    let resp = client
        .post(format!("{}/settings/devices/revoke/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf_token(&body)),
            ("kind", "sync_token".to_string()),
            ("id", foreign_id.to_string()),
        ])
        .send()
        .await
        .unwrap();

    assert!(
        resp.status().is_client_error(),
        "a foreign id is refused, got {}",
        resp.status()
    );
    assert_eq!(
        f.server
            .repos
            .sync_token
            .list_for_user(other)
            .await
            .expect("list")
            .len(),
        1,
        "the other account's token is untouched"
    );
}

/// A form post without the CSRF token must not revoke anything.
#[tokio::test]
async fn revocation_requires_the_csrf_token() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;
    let id = hidden_value(&body, "sync_token", "id").expect("a sync-token revoke form");

    let resp = client
        .post(format!("{}/settings/devices/revoke/", f.server.base_url))
        .form(&[("kind", "sync_token".to_string()), ("id", id)])
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "a missing CSRF token is refused, got {}",
        resp.status()
    );
    assert_eq!(
        f.server
            .repos
            .sync_token
            .list_for_user(f.user_id)
            .await
            .expect("list")
            .len(),
        1,
        "nothing was revoked"
    );
}
