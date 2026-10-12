//! What the Sandbox page says about the host it runs on.
//!
//! The page probes the worker, so this process points it at its own binary
//! before the first fixture, and the one test below that moves the master switch
//! puts it back. The executable and the switch are process-global and set once,
//! which is why this is a binary of its own.

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

/// Save the master switch the way the form does: the write, then the hooks it
/// reports. The hooks are what flip the process-global switch and drop the cached
/// measurement, which is the half a test that only writes the row would miss.
async fn save_switch(server: &TestServer, on: bool) {
    let outcome = server
        .state
        .settings
        .save(
            Section::Sandbox,
            &form(&[("sandbox.enabled", if on { "true" } else { "false" })]),
            None,
        )
        .await
        .expect("save the master switch");
    server.state.apply_settings_hooks(&outcome.hooks);
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

/// How many times `needle` appears in `html`.
fn count(html: &str, needle: &str) -> usize {
    html.matches(needle).count()
}

/// The Sandbox page is the one page whose subject is not a setting: it says what
/// this host actually gives, one band per profile and one row per protection, and
/// warns when that is not the whole set. The values come from a confined child's
/// own report, so the page cannot advertise protection this host does not have.
#[tokio::test]
async fn the_sandbox_page_shows_the_measured_items_per_profile() {
    common::sandbox::enable_real_worker();
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (status, html) = page(&server, &admin, "/sysadmin/settings/sandbox/").await;
    assert_eq!(status, 200);

    // One band per measured profile, each with its own state. Documents and
    // images are reported together — they install the same protections — so two
    // bands and two states are the whole panel.
    assert_eq!(
        count(&html, "data-sandbox-profile=\""),
        2,
        "no profile bands"
    );
    assert_eq!(
        count(&html, "data-sandbox-state>"),
        2,
        "a band without a state"
    );
    // One row per protection per band, in the order the report carries them.
    assert_eq!(
        count(&html, "data-sandbox-item=\""),
        8,
        "not four rows per band"
    );
    for item in ["limits", "files", "network", "process"] {
        assert_eq!(
            count(&html, &format!("data-sandbox-item=\"{item}\"")),
            2,
            "the {item} protection is not shown once per band"
        );
    }

    // The master switch is a control, and it appears exactly once: in the form.
    // The page used to carry a read-only copy of it beside the measurement, and a
    // reader could not tell which of the two they were looking at.
    assert_eq!(
        count(&html, "data-sandbox-switch"),
        1,
        "the switch is not unique"
    );
    assert_eq!(count(&html, "data-setting=\"sandbox.enabled\""), 1);
    // The grade the items add up to is deliberately not on the page, and neither
    // is the strip that carried it.
    assert!(
        !html.contains("data-sandbox-grade"),
        "the page still shows a grade"
    );
    assert!(
        !html.contains("data-sandbox-status"),
        "the page still has a status strip"
    );
    // A page with one row has nothing to narrow, so the filter box is not there.
    assert!(
        !html.contains("data-settings-filter"),
        "a one-row page renders a filter box"
    );

    // No verdict above the bands: a sentence summarising them would only repeat
    // what each state badge and each item row already say, and the page is meant
    // to state one fact once.
    assert!(
        !html.contains("data-sandbox-verdict"),
        "the page still leads with a verdict"
    );

    // The media profile's process item cannot exist, and the row says so instead
    // of leaving the reader to infer it from a missing badge. The sentence behind
    // it is the longest on the page, so it stays inside the row's disclosure.
    assert!(
        html.contains("data-sandbox-media-process"),
        "the media row does not say the process item does not apply"
    );
    assert!(
        html.contains("Not applicable"),
        "no item is marked as not applying to its profile"
    );

    // Each item carries how much it is worth, and its name is a locale key rather
    // than a rendered key: the grade key is never an attribute value, so its raw
    // form could only mean a fallback.
    assert!(
        html.contains("data-sandbox-severity=\"Critical\""),
        "no item carries its severity"
    );
    let body = page_content(&html);
    for key in [
        "sandbox.state_",
        "sandbox.item_",
        "sandbox.profile_",
        "sandbox.measured_",
    ] {
        assert!(!body.contains(key), "the page rendered the raw key {key}");
    }
    for label in [
        "Resource bound",
        "File access",
        "Network access",
        "Starting programs",
    ] {
        assert!(html.contains(label), "the panel is missing {label:?}");
    }

    // The raw reports are one disclosure away, and each line names its profile:
    // two unlabelled lines were the one thing a bug report had to guess at.
    for profile in ["documents", "media"] {
        assert!(
            html.contains(&format!("data-sandbox-detail=\"{profile}\"")),
            "the {profile} raw report is unlabelled"
        );
    }
}

/// Switching the sandbox off is the only thing that disables the features, and
/// the page has to say so rather than leaving a row that looks fine next to a
/// switch that stopped it.
#[tokio::test]
async fn the_switch_off_disables_the_workers_and_the_page_says_so() {
    common::sandbox::enable_real_worker();
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    save_switch(&server, false).await;

    let (status, html) = page(&server, &admin, "/sysadmin/settings/sandbox/").await;
    assert_eq!(status, 200);
    // Both bands read the switch rather than the host: a green item under a
    // switched-off state is not a contradiction as long as the state itself says
    // which decision stopped it.
    assert_eq!(
        count(&html, "data-sandbox-state>Off</span>"),
        2,
        "a switched-off profile does not read as off"
    );
    // The cost of switching off is the switch's own sentence, in the form's help
    // text: repeating it under both bands would be the same paragraph three times
    // on one page.
    assert_eq!(
        count(&html, "data-sandbox-state-reason"),
        0,
        "a switched-off profile repeats the switch's explanation"
    );
    let t = server::i18n::I18n::get(Some("en"));
    assert!(
        page_content(&html).contains(t.tr("setting.sandbox_enabled_help")),
        "nothing on the page says what switching off costs"
    );
    // The measurement is still there, and it is still about the host: an admin
    // deciding whether to switch back on needs to know what they would get. This
    // is the one case a probe judged by the *configured* requirement gets wrong —
    // it would refuse the very host it was asked about and leave the page blank.
    assert_eq!(count(&html, "data-sandbox-item=\""), 8);

    // Leave the process-global switch where the fixture put it: the page is what
    // this test is about, and an off switch would follow the rest of this
    // binary's tests around.
    save_switch(&server, true).await;
}
