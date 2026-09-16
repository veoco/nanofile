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
        .get(format!("{base_url}/settings/credentials/"))
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
    // token list. Sections are addressed by id because the leftover ones are
    // absent unless there is something to show.
    let clients = section_by_id(&body, "devices").expect("the device list");
    let browsers = section_by_id(&body, "browsers").expect("the browser list");
    let tokens = section_by_id(&body, "sync-tokens").expect("the leftover token list");

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

/// The HTML of the `<section id="…">` block, or `None` when the page did not
/// render it at all — which is the expected state for a section that only
/// exists to list leftovers.
fn section_by_id<'a>(body: &'a str, id: &str) -> Option<&'a str> {
    let marker = format!(r#"id="{id}""#);
    let start = body.find(&marker)?;
    let end = body[start..].find("</section>")? + start;
    Some(&body[start..end])
}

/// The device the fixture's multipart login reports (`login_multipart`).
fn test_device() -> server::domain::device::PeerStamp {
    server::domain::device::PeerStamp {
        id: "test-device-123".to_string(),
        name: Some("Test Device".to_string()),
        client_version: None,
    }
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
    let clients_at = body
        .find("Desktop and mobile apps")
        .expect("the client heading");
    let browsers_at = body.find("Browser sessions").expect("the browser heading");
    assert!(clients_at < browsers_at, "sections are ordered");

    let client_section = &body[clients_at..browsers_at];
    assert!(
        client_section.contains("Unnamed device"),
        "the platform-less client is listed, shown as unnamed"
    );
}

/// The heading number of a section is the first `<span>` after its title.
fn heading_count(section: &str) -> usize {
    let span = section.find("<span").expect("a heading count span");
    let rest = &section[span..];
    let open = rest.find('>').expect("span open tag") + 1;
    let close = rest.find("</span>").expect("span close tag");
    rest[open..close].trim().parse().expect("a heading number")
}

/// A 2FA device trust owned by `device_id` (`None` means no device at all).
async fn seed_device_trust(f: &TestFixture, token: &str, device_id: Option<&str>) {
    f.server
        .repos
        .s2fa_token
        .create_s2fa_token(server::repository::s2fa_token::CreateS2faTokenParams {
            user_id: f.user_id,
            token: token.to_string(),
            device_id: device_id.map(str::to_string),
            device_name: Some("Test Device".to_string()),
            created_at: 1_000,
            expires_at: i64::MAX,
        })
        .await
        .expect("seed a device trust");
}

/// A trust whose `device_id` matches a signed-in device is shown inside that
/// device's card. The page used to render a "Devices that skip two-factor"
/// section carrying the global count and, under it, "No devices skip two-factor."
/// — the heading and the body contradicted each other.
#[tokio::test]
async fn a_trust_that_belongs_to_a_device_is_not_an_unknown_one() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .login_multipart("test@example.com", "password")
        .await;
    assert_eq!(resp.status(), 200);
    seed_device_trust(&f, "trust-known", Some("test-device-123")).await;

    let client = login_client(&f).await;
    let body = visible(&page(&client, &f.server.base_url).await);

    assert!(
        section_by_id(&body, "device-trusts").is_none(),
        "a trust with a known device must not reach the leftover section"
    );
    let devices = section_by_id(&body, "devices").expect("the device list");
    assert!(
        devices.contains("Test Device"),
        "the device card is rendered"
    );
    assert!(
        devices.contains("This device may skip verification codes"),
        "the trust is listed inside the device card"
    );
}

/// The leftover section exists for trusts with no known device, and its heading
/// counts exactly the rows underneath it.
#[tokio::test]
async fn an_unknown_trust_is_listed_and_counted() {
    let f = TestFixture::new().await;
    seed_device_trust(&f, "trust-ghost-1", Some("ghost-a")).await;
    seed_device_trust(&f, "trust-ghost-2", None).await;

    let client = login_client(&f).await;
    let body = visible(&page(&client, &f.server.base_url).await);
    let trusts = section_by_id(&body, "device-trusts").expect("the leftover trust list");

    assert_eq!(heading_count(trusts), 2, "the heading counts its own rows");
    assert_eq!(
        trusts.matches(r#"value="device_trust""#).count(),
        2,
        "and there are that many rows"
    );
}

/// A sync token pointed at a signed-in device belongs to that device; only the
/// token with no known device is listed, and the heading counts just that one.
#[tokio::test]
async fn a_sync_token_for_a_known_device_is_not_listed_as_unknown() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .login_multipart("test@example.com", "password")
        .await;
    assert_eq!(resp.status(), 200);

    // A second library, because a sync token is unique per (user, library).
    // Creating one mints its sync token, which has no peer id yet, so that is
    // the leftover; the fixture's own token is then pointed at the device that
    // signed in above, so it belongs to that card instead.
    let resp = f.client.create_repo(&f.api_token, "second").await;
    assert_eq!(resp.status(), 201);

    let attached = f
        .server
        .repos
        .sync_token
        .list_for_user(f.user_id)
        .await
        .expect("list")
        .into_iter()
        .find(|t| t.repo_id == f.repo_id)
        .expect("the fixture's token");
    f.server
        .repos
        .sync_token
        .attach_peer_if_unset(attached.id, &test_device())
        .await
        .expect("attach the token to the device");

    let client = login_client(&f).await;
    let body = visible(&page(&client, &f.server.base_url).await);
    let tokens = section_by_id(&body, "sync-tokens").expect("the leftover token list");

    assert_eq!(heading_count(tokens), 1, "only the unowned token is listed");
    assert_eq!(
        tokens.matches(r#"value="sync_token""#).count(),
        1,
        "and there is one row"
    );
    assert!(
        tokens.contains("second"),
        "the row shown is the second library's token"
    );
    let devices = section_by_id(&body, "devices").expect("the device list");
    assert!(
        devices.contains("Unlinking removes 2 credential(s)"),
        "the device owns its session and the attached token"
    );

    // Point the last leftover at the device too: with nothing unowned, the
    // section must not be rendered at all rather than shown empty.
    let second = f
        .server
        .repos
        .sync_token
        .list_for_user(f.user_id)
        .await
        .expect("list")
        .into_iter()
        .find(|t| t.repo_id != f.repo_id)
        .expect("the second library's token");
    f.server
        .repos
        .sync_token
        .attach_peer_if_unset(second.id, &test_device())
        .await
        .expect("attach the last token");

    let after = visible(&page(&client, &f.server.base_url).await);
    assert!(
        section_by_id(&after, "sync-tokens").is_none(),
        "every token belongs to a device, so the section is not rendered"
    );
}

#[tokio::test]
async fn a_sync_token_can_be_revoked_by_id() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;

    let id = hidden_value(&body, "sync_token", "id").expect("a sync-token revoke form");
    let resp = client
        .post(format!(
            "{}/settings/credentials/revoke/",
            f.server.base_url
        ))
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

    // The section holds unattributed tokens only, so it disappears with its
    // last entry rather than staying behind to announce that there are none.
    let after = visible(&page(&client, &f.server.base_url).await);
    assert!(
        !after.contains("Unattributed repository sync tokens"),
        "the section is gone once nothing is unowned"
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
        .post(format!(
            "{}/settings/credentials/revoke/",
            f.server.base_url
        ))
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
        .create(&other_repo, other, "peer-token".to_string(), None, 1, None)
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
        .post(format!(
            "{}/settings/credentials/revoke/",
            f.server.base_url
        ))
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
        .post(format!(
            "{}/settings/credentials/revoke/",
            f.server.base_url
        ))
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
