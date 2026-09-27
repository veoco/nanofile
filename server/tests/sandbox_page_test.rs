//! What the Sandbox page says about the host it runs on.
//!
//! The page probes the worker, so this process points it at its own binary
//! before the first fixture, and both tests below move `sandbox.min_level` and
//! put it back. The executable and the requirement are process-global and set
//! once, which is why this is a binary of its own.

mod common;

use common::TestServer;
use infra::settings::Section;
use server::settings::SettingsForm;

/// A form that submits non-secret values.
fn form(pairs: &[(&str, &str)]) -> SettingsForm {
    SettingsForm {
        values: pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        secrets: Default::default(),
    }
}

/// Sign in through the Web UI form and keep the session cookies.
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

/// Fetch a page as the signed-in client, and its status.
async fn page(server: &TestServer, client: &reqwest::Client, path: &str) -> (u16, String) {
    let resp = client
        .get(format!(
            "{}/{}",
            server.base_url,
            path.trim_start_matches('/')
        ))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap())
}

/// The rendered body, without the translation dictionary and bundle that follow
/// it: the dictionary ships every key with its translation, so a raw key there
/// is normal, and a raw key in the body is not.
fn page_content(html: &str) -> &str {
    html.split("id=\"__i18n\"").next().unwrap_or(html)
}

/// The Sandbox page is the one page whose subject is not a setting: it says
/// what this host actually gives, item by item, and warns when that is not the
/// whole set. The values come from a confined child's own report, so the page
/// cannot advertise protection this host does not have.
#[tokio::test]
async fn the_sandbox_page_shows_the_measured_grade_and_items() {
    let _ = common::sandbox::configure_real_worker();
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (status, html) = page(&server, &admin, "/sysadmin/settings/sandbox/").await;
    assert_eq!(status, 200);
    assert!(html.contains("data-sandbox-status"), "no status panel");
    assert!(html.contains("data-sandbox-grade"), "no grade");
    // One row per protection, in the order the report carries them.
    for item in ["limits", "files", "network", "process"] {
        assert!(
            html.contains(&format!("data-sandbox-item=\"sandbox.item_{item}\"")),
            "the {item} protection is not shown"
        );
    }
    // The media worker is its own child with its own grade: the profile that
    // cannot have the process item has to say so rather than borrow the grade
    // above it, and the facts that explain it are notes, not prose in a log.
    assert!(html.contains("data-sandbox-media"), "no media row");
    assert!(
        html.contains("data-sandbox-media-state"),
        "the media row shows no state"
    );
    // A profile that answered shows its own grade, and — when the helper it may
    // start actually ran — the note that says which program that is. A probe
    // that could not run says why instead, which is the other half of the same
    // contract; a host with no helper reaches this row with neither.
    if !html.contains("data-sandbox-media-reason") {
        assert!(
            html.contains("data-sandbox-media-grade"),
            "the media row shows no grade of its own"
        );
        if html.contains("parse=media-ok") {
            assert!(
                html.contains("data-sandbox-media-note"),
                "the media row carries no note for the helper it may start"
            );
        }
    }
    // The verdict is what the page leads with, and whether the features run is
    // a separate question from what the host could give: an admin has to be able
    // to tell "this machine is weak" from "the setting you chose disabled it".
    assert!(html.contains("data-sandbox-verdict"), "no verdict banner");
    assert!(
        html.contains("data-sandbox-verdict-label"),
        "the verdict has no label"
    );
    assert!(html.contains("data-sandbox-decision"), "no decision row");
    assert!(
        html.contains("data-sandbox-decision-state"),
        "the decision row shows no state"
    );
    // Each item carries how much it is worth, and what its absence opens rather
    // than a restatement of its name. Either the item is in place — and then
    // there is nothing to explain — or the impact sentence is there.
    assert!(
        html.contains("data-sandbox-severity=\"Critical\""),
        "no item carries its severity"
    );
    // The media profile's process item cannot exist, and the row says so instead
    // of leaving the reader to infer it from a missing badge.
    assert!(
        html.contains("data-sandbox-media-process"),
        "the media row does not say the process item does not apply"
    );
    // The panel names a grade and labels each item, rather than rendering the
    // locale key it came from. The grade key is never an attribute value, so
    // its raw form can only mean a fallback.
    let body = page_content(&html);
    assert!(
        !body.contains("sandbox.grade_"),
        "the panel rendered a raw locale key"
    );
    for label in [
        "Resource limits",
        "File access",
        "Network access",
        "Starting programs",
    ] {
        assert!(html.contains(label), "the panel is missing {label:?}");
    }
}

/// The media profile grades below the document profile on every platform, so a
/// minimum of `full` refuses it — and the page has to say that, rather than
/// showing a row that looks fine next to a setting that stopped it.
#[tokio::test]
async fn a_full_minimum_disables_the_media_worker_and_says_so() {
    let _ = common::sandbox::configure_real_worker();
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    server
        .state
        .settings
        .save(
            Section::Sandbox,
            &form(&[("sandbox.min_level", "full")]),
            None,
        )
        .await
        .expect("raise the minimum");

    let (status, html) = page(&server, &admin, "/sysadmin/settings/sandbox/").await;
    assert_eq!(status, 200);
    assert!(
        html.contains("Disabled by the minimum grade"),
        "the media row does not say why it is off: {html}"
    );
    assert!(
        html.contains("data-sandbox-media-reason"),
        "the media row carries no reason"
    );

    // Leave the process-global requirement where the fixture put it: this test
    // is about the page, and a minimum of `full` would follow the rest of this
    // binary's tests around.
    server
        .state
        .settings
        .save(
            Section::Sandbox,
            &form(&[("sandbox.min_level", "partial")]),
            None,
        )
        .await
        .expect("restore the minimum");
}
