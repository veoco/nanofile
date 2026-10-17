//! What the Sandbox page says on a host whose configured media helper cannot run.
//!
//! The helper is process-global, so this is a binary of its own: the one server
//! below is started with a `storage.ffmpeg_path` that names a file which is not
//! there, which is the shape a host takes when the setting is a bare command name
//! and nothing on `PATH` answers to it — and the shape a package manager's shim
//! takes when the sandbox cannot reach it.
//!
//! The subject is the media band. Its child runs and reports either way, so the
//! band is *measured*: it has to keep its four rows and let its state carry the
//! failure. A page that dropped the rows would take the one question it exists to
//! answer — is this host weak, or is the program it runs broken? — away from the
//! admin exactly when the answer matters.

mod common;

use common::TestServer;

async fn ui_login(server: &TestServer, email: &str, password: &str) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", email), ("password", password)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login should redirect");
    let _ = client
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await;
    client
}

async fn sandbox_page(server: &TestServer, client: &reqwest::Client) -> String {
    let resp = client
        .get(format!(
            "{}/sysadmin/settings/sandbox/",
            server.base_url.trim_end_matches('/')
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    resp.text().await.unwrap()
}

fn band<'a>(html: &'a str, id: &str) -> &'a str {
    let marker = format!("data-sandbox-profile=\"{id}\"");
    let start = html
        .find(&marker)
        .unwrap_or_else(|| panic!("no {id} band in: {html}"));
    let rest = &html[start + marker.len()..];
    let end = rest
        .find("data-sandbox-profile=\"")
        .or_else(|| rest.find("data-sandbox-detail="))
        .unwrap_or(rest.len());
    &rest[..end]
}

fn count(html: &str, needle: &str) -> usize {
    html.matches(needle).count()
}

/// A helper that cannot run is a failure of the media profile's own job, not a
/// reason to stop reporting the four items the host did establish.
#[tokio::test]
async fn a_helper_that_cannot_run_keeps_the_media_band_and_its_items() {
    common::sandbox::enable_real_worker();
    let absent =
        std::env::temp_dir().join(format!("nanofile-absent-helper-{}", uuid::Uuid::new_v4()));
    assert!(!absent.exists(), "the fixture must name a file that is not");
    let server = TestServer::start_with_storage_config(move |config| {
        config.ffmpeg_path = absent.to_string_lossy().into_owned();
    })
    .await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let html = sandbox_page(&server, &admin).await;

    // Both bands were measured, so both carry the whole set of four items: the
    // documents band never needed the helper, and the media child reports even
    // when the program it was told to start cannot be run.
    let measured = count(&html, "data-sandbox-measured=\"true\"");
    assert_eq!(measured, 2, "bands measured: {measured}");
    let media = band(&html, "media");
    assert!(
        media.contains("data-sandbox-measured=\"true\""),
        "the media band was not measured: {media}"
    );
    assert_eq!(
        count(media, "data-sandbox-item=\""),
        4,
        "the media band lost its rows: {media}"
    );
    for item in ["limits", "files", "network", "process"] {
        assert!(
            media.contains(&format!("data-sandbox-item=\"{item}\"")),
            "the media band is missing the {item} item: {media}"
        );
    }

    let t = server::i18n::I18n::get(Some("en"));
    assert!(
        media.contains(&format!(
            "data-sandbox-state>{}</span>",
            t.tr("sandbox.state_media_failed")
        )),
        "the media band does not read as failed: {media}"
    );
    assert!(
        media.contains("data-sandbox-state-reason"),
        "the media band carries no reason: {media}"
    );
    assert!(
        media.contains("media-unavailable(") || media.contains("media-failed("),
        "the reason does not come from the helper's own self-test: {media}"
    );

    assert!(
        media.contains("data-sandbox-media-process"),
        "the media row does not say the process item does not apply: {media}"
    );
    assert!(
        media.contains("Not applicable"),
        "no media item is marked as not applying: {media}"
    );

    assert_eq!(count(&html, "data-sandbox-switch"), 1);
}
