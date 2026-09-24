//! Tests for per-user UI language preference and the i18n machinery.

mod common;

use common::{TestServer, create_test_user};
use server::i18n::I18n;

/// Log in via the Web UI and return a cookie-tracking client plus the
/// `sfcsrftoken` cookie value (needed for CSRF-protected form posts).
async fn login_ui(server: &TestServer, email: &str, password: &str) -> (reqwest::Client, String) {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", email), ("password", password)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login should redirect on success");

    let mut csrf = String::new();
    for cookie in resp.headers().get_all("set-cookie") {
        let s = cookie.to_str().unwrap_or("");
        if let Some(v) = s.strip_prefix("sfcsrftoken=") {
            csrf = v.split(';').next().unwrap_or("").to_string();
        }
    }
    assert!(!csrf.is_empty(), "login should set the sfcsrftoken cookie");
    (client, csrf)
}

/// POST /settings/language/ persists the user's UI language preference.
#[tokio::test]
async fn test_update_language_persists() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "i18n@test.com", "password").await;
    let (client, csrf) = login_ui(&server, "i18n@test.com", "password").await;

    let resp = client
        .post(format!("{}/settings/language/", server.base_url))
        .form(&[("language", "zh"), ("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        302,
        "language update should redirect to /settings/"
    );

    let user = server
        .repos
        .user
        .find_by_email("i18n@test.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.language.as_deref(), Some("zh"));
}

/// Switching back to English clears the stored preference to a supported value.
#[tokio::test]
async fn test_update_language_back_to_en() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "i18n2@test.com", "password").await;
    let (client, csrf) = login_ui(&server, "i18n2@test.com", "password").await;

    let resp = client
        .post(format!("{}/settings/language/", server.base_url))
        .form(&[("language", "en"), ("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    let user = server
        .repos
        .user
        .find_by_email("i18n2@test.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.language.as_deref(), Some("en"));
}

/// Unsupported language values are rejected with 400.
#[tokio::test]
async fn test_update_language_rejects_unsupported() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "i18n3@test.com", "password").await;
    let (client, csrf) = login_ui(&server, "i18n3@test.com", "password").await;

    let resp = client
        .post(format!("{}/settings/language/", server.base_url))
        .form(&[("language", "de"), ("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "unsupported language should be rejected"
    );

    let user = server
        .repos
        .user
        .find_by_email("i18n3@test.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.language, None, "preference should remain unchanged");
}

/// The settings page renders in the user's chosen language.
#[tokio::test]
async fn test_settings_page_uses_chosen_language() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "i18n4@test.com", "password").await;
    let (client, csrf) = login_ui(&server, "i18n4@test.com", "password").await;

    let resp = client
        .post(format!("{}/settings/language/", server.base_url))
        .form(&[("language", "zh"), ("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);

    let page = client
        .get(format!("{}/settings/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let body = page.text().await.unwrap();
    assert!(
        body.contains("界面语言"),
        "settings page should render in Chinese: {}",
        body.chars().take(200).collect::<String>()
    );
}

/// A language preference is picked up by the WebUser extractor on the next
/// request even without going through the settings page again.
#[tokio::test]
async fn test_libraries_page_uses_language() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "i18n5@test.com", "password").await;
    let (client, csrf) = login_ui(&server, "i18n5@test.com", "password").await;

    // Default is English (Accept-Language not sent).
    let page = client
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await
        .unwrap();
    let body = page.text().await.unwrap();
    assert!(body.contains("New Library"), "default should be English");

    // Switch to Chinese then verify the layout switches.
    client
        .post(format!("{}/settings/language/", server.base_url))
        .form(&[("language", "zh"), ("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();

    let page = client
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await
        .unwrap();
    let body = page.text().await.unwrap();
    assert!(
        body.contains("新建资料库"),
        "libraries page should render in Chinese"
    );
}

/// The HTML a fact row puts between its label and the end of that row.
fn fact<'a>(body: &'a str, label: &str) -> &'a str {
    let key = format!(r#"<span class="k">{label}</span>"#);
    let start = body
        .find(&key)
        .unwrap_or_else(|| panic!("no {label:?} fact on the page"))
        + key.len();
    let rest = &body[start..];
    &rest[..rest.find("</div>").unwrap_or(rest.len())]
}

/// A key that does not expire is a property, not an event that never happened:
/// the row and both expiry forms have to use the same words for it, and only the
/// "last used" fact may say "never" in the other sense.
#[tokio::test]
async fn test_a_key_without_an_expiry_is_not_read_as_never_happened() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "i18n6@test.com", "password").await;
    let (client, csrf) = login_ui(&server, "i18n6@test.com", "password").await;

    client
        .post(format!("{}/settings/language/", server.base_url))
        .form(&[("language", "zh"), ("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();

    let created = client
        .post(format!("{}/settings/api-keys/create/", server.base_url))
        .form(&[
            ("name", "never-key"),
            ("expiry", "never"),
            ("all_repos", "1"),
            ("cap__file.read", "1"),
            ("csrf_token", &csrf),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        created.status(),
        303,
        "creating a key redirects to the list"
    );

    let page = client
        .get(format!("{}/settings/api-keys/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let body = page.text().await.unwrap();

    let t = I18n::get(Some("zh"));
    let expires = fact(&body, t.tr("apikey.expires"));
    assert!(
        expires.contains(t.tr("common.no_expiry")),
        "the expiry row must say it does not expire, got {expires:?}"
    );
    assert!(
        !expires.contains(t.tr("common.never")),
        "the expiry row must not read as an event, got {expires:?}"
    );
    let last_used = fact(&body, t.tr("apikey.last_used"));
    assert!(
        last_used.contains(t.tr("common.never")),
        "an unused key still reads as never used, got {last_used:?}"
    );
    // The row and the create/edit expiry selects must agree, or the page
    // contradicts itself about the same property.
    assert!(
        body.matches(t.tr("common.no_expiry")).count() >= 3,
        "row plus two forms should all say {:?}",
        t.tr("common.no_expiry")
    );
}
