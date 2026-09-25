//! The layered settings service, end to end.
//!
//! These tests drive the service directly (through the `AppState` the running
//! test server holds) rather than through a page: what the admin UI does is
//! parse a form and call exactly these methods, so this is where the layering
//! contract — what wins, what is stored, and what takes effect when — is pinned.

mod common;

use common::TestServer;
use infra::settings::{ConfigPolicy, Section, SettingRow, SettingsPolicy};
use server::settings::{SecretState, SettingsForm, SettingsLayers, SettingsService};

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

/// A form that replaces (or clears) one secret.
fn secret_form(key: &str, value: Option<&str>) -> SettingsForm {
    SettingsForm {
        values: Default::default(),
        secrets: [(key.to_string(), value.map(str::to_string))]
            .into_iter()
            .collect(),
    }
}

/// The row currently held for a key, if any.
fn stored(server: &TestServer, key: &str) -> Option<SettingRow> {
    server.state.settings.row(key)
}

#[tokio::test]
async fn a_live_setting_takes_effect_without_a_restart() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    assert!(
        server.state.config().server.share_link_enabled,
        "default is on"
    );

    let outcome = settings
        .save(
            Section::Security,
            &form(&[("server.share_link_enabled", "false")]),
            None,
        )
        .await
        .expect("save");
    assert_eq!(outcome.changed, vec!["server.share_link_enabled"]);
    assert!(outcome.restart_pending.is_empty(), "this one is live");

    // The very next read sees it: no restart, and no second code path.
    assert!(!server.state.config().server.share_link_enabled);
    // ...and it is stored, so it survives the process.
    assert_eq!(
        stored(&server, "server.share_link_enabled").map(|row| row.value),
        Some("false".to_string())
    );
}

#[tokio::test]
async fn a_restart_only_setting_is_stored_but_not_applied() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;
    let before = server.state.config().server.max_json_body_mb;

    let outcome = settings
        .save(
            Section::Server,
            &form(&[("server.max_json_body_mb", "7")]),
            None,
        )
        .await
        .expect("save");

    assert_eq!(outcome.restart_pending, vec!["server.max_json_body_mb"]);
    assert_eq!(
        server.state.config().server.max_json_body_mb,
        before,
        "the running process keeps the value it started with"
    );
    // The page has to be able to say so...
    assert!(
        settings
            .pending_restart()
            .contains("server.max_json_body_mb")
    );
    // ...and the next start picks the saved value up.
    assert_eq!(
        settings
            .resolved_one("server.max_json_body_mb")
            .expect("resolved")
            .value,
        "7"
    );
}

#[tokio::test]
async fn a_config_override_key_beats_the_database() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    settings
        .save(
            Section::Security,
            &form(&[("server.sso_enabled", "false")]),
            None,
        )
        .await
        .expect("save");
    assert!(!server.state.config().server.sso_enabled);

    // Rebuild the service against the same rows but with a policy that gives the
    // config file the key: the operator's "leave this one to the deployment
    // file" escape hatch.
    let mut base = settings.base().clone();
    base.server.sso_enabled = true;
    let layers = SettingsLayers {
        base,
        env_keys: Default::default(),
        rows: settings
            .stored_keys()
            .into_iter()
            .filter_map(|key| settings.row(&key).map(|row| (key, row)))
            .collect(),
        policy: SettingsPolicy {
            config_policy: ConfigPolicy::Bootstrap,
            config_override_keys: ["server.sso_enabled".to_string()].into_iter().collect(),
            refresh_interval_secs: 30,
        },
    };
    let service = SettingsService::new(
        server.repos.settings.clone(),
        server.state.token_cipher.clone(),
        layers,
    );

    assert!(
        service.startup().server.sso_enabled,
        "the config file wins for a key listed in config_override_keys"
    );
    assert_eq!(
        service
            .resolved_one("server.sso_enabled")
            .expect("resolved")
            .origin
            .id(),
        "config"
    );
}

#[tokio::test]
async fn a_rejected_value_is_not_stored() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    // A port of 0 parses as a number but could never be bound.
    let error = settings
        .save(Section::Server, &form(&[("server.port", "0")]), None)
        .await
        .expect_err("port 0 must be refused");
    assert!(matches!(error, base::error::AppError::BadRequest(_)));
    assert!(settings.stored_keys().is_empty(), "nothing was written");

    // A value that is not a number at all.
    let error = settings
        .save(Section::Server, &form(&[("server.port", "eighty")]), None)
        .await
        .expect_err("a non-number must be refused");
    assert!(matches!(error, base::error::AppError::BadRequest(_)));
    assert!(settings.stored_keys().is_empty());
}

#[tokio::test]
async fn read_only_and_unknown_keys_are_refused() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    for key in ["server.secret_key", "database.url", "no.such.setting"] {
        let error = settings
            .save(Section::Advanced, &form(&[(key, "x")]), None)
            .await
            .expect_err(key);
        assert!(
            matches!(error, base::error::AppError::BadRequest(_)),
            "{key} must be refused"
        );
    }
    assert!(settings.stored_keys().is_empty());
}

#[tokio::test]
async fn a_key_outside_the_page_is_refused() {
    let server = TestServer::start().await;
    let error = server
        .state
        .settings
        .save(
            Section::Server,
            &form(&[("server.share_link_enabled", "false")]),
            None,
        )
        .await
        .expect_err("a Security key cannot be saved from the Server page");
    assert!(matches!(error, base::error::AppError::BadRequest(_)));
}

#[tokio::test]
async fn a_secret_is_stored_encrypted_and_used_in_the_clear() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    settings
        .save(
            Section::Email,
            &secret_form("email.password", Some("hunter2")),
            None,
        )
        .await
        .expect("save");

    // At rest it is ciphertext...
    let row = stored(&server, "email.password").expect("a row");
    assert!(row.value.starts_with("enc1:"), "stored: {}", row.value);
    assert!(!row.value.contains("hunter2"));
    // ...while the running server holds the plaintext, so delivery can
    // authenticate.
    assert_eq!(
        server.state.config().email.password.as_deref(),
        Some("hunter2")
    );
    assert_eq!(
        settings.secret_state("email.password"),
        Some(SecretState::Stored)
    );

    // A blank field means "leave it alone": the page never received the value.
    settings
        .save(Section::Email, &secret_form("email.password", None), None)
        .await
        .expect("save");
    assert_eq!(
        server.state.config().email.password.as_deref(),
        Some("hunter2")
    );

    // An explicit clear removes the row, so the config file supplies it again.
    settings
        .save(
            Section::Email,
            &secret_form("email.password", Some("")),
            None,
        )
        .await
        .expect("save");
    assert!(stored(&server, "email.password").is_none());
    assert_eq!(
        settings.secret_state("email.password"),
        Some(SecretState::Unset)
    );
}

#[tokio::test]
async fn clearing_a_key_falls_back_to_the_config_file() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    settings
        .save(
            Section::Security,
            &form(&[("server.webdav_enabled", "false")]),
            None,
        )
        .await
        .expect("save");
    assert!(!server.state.config().server.webdav_enabled);

    settings
        .clear(&["server.webdav_enabled".to_string()])
        .await
        .expect("clear");
    assert!(stored(&server, "server.webdav_enabled").is_none());
    assert!(
        server.state.config().server.webdav_enabled,
        "the config file (or the built-in default) is back in charge"
    );
    assert_eq!(
        settings
            .resolved_one("server.webdav_enabled")
            .expect("resolved")
            .origin
            .id(),
        "default"
    );
}

#[tokio::test]
async fn a_reload_picks_up_a_change_made_elsewhere() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    assert!(server.state.config().server.share_link_enabled);

    // Write a row the way another instance (or a restored backup) would.
    server
        .repos
        .settings
        .upsert_many(
            &[("server.share_link_enabled".to_string(), "false".to_string())]
                .into_iter()
                .collect(),
            None,
            chrono::Utc::now().timestamp(),
        )
        .await
        .expect("seed a row");

    let outcome = settings.reload().await.expect("reload");
    assert!(
        outcome
            .changed
            .contains(&"server.share_link_enabled".to_string()),
        "changed: {:?}",
        outcome.changed
    );
    assert!(!server.state.config().server.share_link_enabled);

    // Nothing changed the second time, so no work is done and no hook fires.
    assert!(settings.reload().await.expect("reload").is_empty());
}

#[tokio::test]
async fn a_saved_bool_accepts_the_spellings_an_operator_types() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    for (typed, expected) in [("on", true), ("off", false), ("TRUE", true), ("0", false)] {
        settings
            .save(
                Section::Security,
                &form(&[("server.share_link_enabled", typed)]),
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("{typed}: {e:?}"));
        assert_eq!(server.state.config().server.share_link_enabled, expected);
        // The stored form is canonical, so what the page renders cannot differ
        // from what the server parsed.
        assert_eq!(
            stored(&server, "server.share_link_enabled")
                .expect("a row")
                .value,
            if expected { "true" } else { "false" }
        );
    }
}

#[tokio::test]
async fn an_unknown_row_is_ignored_and_reported() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    server
        .repos
        .settings
        .upsert_many(
            &[("no.such.setting".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            None,
            1,
        )
        .await
        .expect("seed an unknown row");

    // Reading the effective values must not panic or apply it.
    settings.reload().await.expect("reload");
    assert!(settings.resolved_one("no.such.setting").is_none());
    assert!(settings.stored_keys().contains("no.such.setting"));
    settings.log_startup_diagnostics();
}

#[tokio::test]
async fn a_setting_captured_at_startup_reports_the_hook_that_applies_it() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    // Each of these was copied into a long-lived object at startup, so the save
    // has to say which hook the caller must run — and the catalog must not
    // mislabel any of them as plain "live".
    for (section, key, value, expected) in [
        (
            Section::RateLimits,
            "auth.max_login_attempts",
            "3",
            infra::settings::Hook::RateLimits,
        ),
        (
            Section::Maintenance,
            "tasks.max_active_tasks",
            "5",
            infra::settings::Hook::TaskSystem,
        ),
        (
            Section::Maintenance,
            "tasks.load_aware",
            "true",
            infra::settings::Hook::TaskLoad,
        ),
        (
            Section::Notifications,
            "notification.max_connections",
            "10",
            infra::settings::Hook::NotificationManager,
        ),
        (
            Section::Advanced,
            "sync.max_tree_depth",
            "8",
            infra::settings::Hook::SyncStatics,
        ),
        (
            Section::Email,
            "email.enabled",
            "true",
            infra::settings::Hook::MailDrain,
        ),
    ] {
        let outcome = settings
            .save(section, &form(&[(key, value)]), None)
            .await
            .unwrap_or_else(|e| panic!("{key}: {e:?}"));
        assert!(
            outcome.hooks.contains(&expected),
            "{key} must report {expected:?}, got {:?}",
            outcome.hooks
        );
        assert!(
            outcome.restart_pending.is_empty(),
            "{key} is not restart-only"
        );
    }

    // Running every hook must be idempotent and must not panic.
    server
        .state
        .apply_settings_hooks(&infra::settings::Hook::ALL.into_iter().collect());
}

#[tokio::test]
async fn the_outbox_drainer_stays_registered_while_mail_is_off() {
    let server = TestServer::start().await;
    // `TestServer::start` leaves `[email] enabled` false. The job still has to
    // exist, or turning mail on from the page would need the restart this
    // release removes.
    assert!(!server.state.config().email.enabled);
    assert!(
        server
            .state
            .tasks
            .job(server::tasks::spec::JobKey::MailDelivery)
            .is_some(),
        "the mail drainer must be registered regardless of the switch"
    );
}

// ── The pages themselves ─────────────────────────────────────────────────

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

/// Everything after the embedded `window.__T` dictionary.
///
/// Everything the page renders, without the translation dictionary and the
/// bundle script that follow it. The dictionary ships every key with its
/// translation, so a raw key there is normal; a raw key in the body is not.
fn page_content(html: &str) -> &str {
    html.split("id=\"__i18n\"").next().unwrap_or(html)
}

/// The CSRF token embedded in a settings page's form.
fn csrf_of(html: &str) -> String {
    html.split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the settings page must embed a csrf_token")
        .to_string()
}

#[tokio::test]
async fn every_settings_page_renders_for_an_admin_only() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    // Every page of the nav, plus the first page's old id: a bookmark from
    // before it was renamed still has to land on a page with rows on it.
    for section in infra::settings::Section::ALL {
        let path = format!("/sysadmin/settings/{}/", section.id());
        let (status, html) = page(&server, &admin, &path).await;
        assert_eq!(status, 200, "{path}");
        assert!(html.contains("class=\"page-title\""), "{path} has no title");
        let body = page_content(&html);
        // Any untranslated string — a label, a help sentence, a unit, a page
        // title, a group heading — renders as its own locale id.
        assert!(
            !body.contains("setting."),
            "{path} rendered a raw locale key"
        );
        assert!(
            html.matches("class=\"badge").count() >= 2,
            "{path} rendered no setting rows"
        );
        // The page is rendered as its groups, one `<section>` each, and the
        // layering policy is not one of them.
        assert_eq!(
            html.matches(r#"data-setting-group=""#).count(),
            infra::settings::groups(section).count(),
            "{path} did not render one section per group"
        );
        assert_eq!(
            body.contains("config_policy"),
            section == infra::settings::Section::Advanced,
            "{path} rendered the layering policy on the wrong page"
        );
    }
    {
        let path = "/sysadmin/settings/general/";
        let (status, html) = page(&server, &admin, path).await;
        assert_eq!(status, 200, "the first page's old id must still render");
        assert!(html.matches("class=\"badge").count() >= 2, "{path}");
    }
    // An unknown section lands on the first page rather than 404ing.
    let (status, _) = page(&server, &admin, "/sysadmin/settings/nope/").await;
    assert_eq!(status, 200);

    // A regular account is redirected away, never served the page.
    common::create_test_user(&server.db, "pleb@example.com", "password123").await;
    let user = ui_login(&server, "pleb@example.com", "password123").await;
    let resp = user
        .get(format!("{}/sysadmin/settings/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert!(
        resp.headers()
            .get("location")
            .is_some_and(|l| l.to_str().unwrap().contains("/libraries/"))
    );
}

/// The markup of one row, from its opening tag to the next row's.
fn row_of<'a>(html: &'a str, key: &str) -> &'a str {
    let marker = format!(r#"data-setting="{key}""#);
    let at = html
        .find(&marker)
        .unwrap_or_else(|| panic!("{key} did not render"));
    // The element that owns the attribute, so the opening tag is part of the
    // slice and `text_of` can tell markup from text.
    let start = html[..at].rfind('<').expect("a row is an element");
    let rest = &html[start..];
    // The row's own opening tag carries the marker too, so the next row begins
    // only after it closes.
    let open_end = rest.find('>').expect("the opening tag closes") + 1;
    let end = rest[open_end..]
        .find(r#"data-setting=""#)
        .map(|i| open_end + i)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The filter matches a row against its key, its name and its description, so
/// the row has to carry all three.
#[tokio::test]
async fn a_row_carries_the_text_the_filter_matches_on() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    assert!(
        html.contains("data-settings-filter"),
        "the page must have a filter box"
    );
    assert!(
        html.contains("data-settings-empty"),
        "and a note for a query that matches nothing"
    );

    let row = row_of(&html, "server.max_upload_size_mb");
    let search = row
        .split(r#"data-setting-search=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the row carries its search text");
    assert!(search.contains("server.max_upload_size_mb"), "{search}");
    assert!(search.contains("Max upload size"), "{search}");
    assert!(search.contains("transport cap"), "{search}");
}

/// The text a reader sees in a fragment: the markup removed, so a key that only
/// lives in a `title` or a `data-` attribute does not count as printed.
fn text_of(fragment: &str) -> String {
    let mut out = String::new();
    let mut tags = 0usize;
    for ch in fragment.chars() {
        match ch {
            '<' => tags += 1,
            '>' => tags = tags.saturating_sub(1),
            _ if tags == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A row shows one state, not all of them. A setting that needs a restart and is
/// also read-only used to say both, which is how a badge strip stops meaning
/// anything.
#[tokio::test]
async fn a_row_shows_at_most_one_state_badge() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;

    // The port is restart-only, so the badge says so; the chunk size is live and
    // carries nothing but its origin.
    let restart = row_of(&html, "server.port");
    assert!(restart.contains("Needs a restart"), "{restart}");
    assert_eq!(
        restart.matches(r#"class="badge"#).count(),
        2,
        "an origin badge and exactly one state badge: {restart}"
    );
    let live = row_of(&html, "server.max_chunk_size_mb");
    assert_eq!(
        live.matches(r#"class="badge"#).count(),
        1,
        "origin only: {live}"
    );

    // A saved value waiting for the next start is the more urgent state, and its
    // row says that instead of repeating that a change would need a start.
    server
        .state
        .settings
        .save(
            Section::Server,
            &form(&[("server.max_json_body_mb", "7")]),
            None,
        )
        .await
        .expect("save");
    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    let pending = row_of(&html, "server.max_json_body_mb");
    assert!(pending.contains("Waiting for a restart"), "{pending}");
    assert!(
        !pending.contains(">Needs a restart<"),
        "the weaker state must not also render: {pending}"
    );

    // A read-only row says that, and nothing else.
    let (_, html) = page(&server, &admin, "/sysadmin/settings/advanced/").await;
    let read_only = row_of(&html, "database.url");
    assert!(read_only.contains("Read-only"), "{read_only}");
    assert_eq!(
        read_only.matches(r#"class="badge"#).count(),
        2,
        "{read_only}"
    );
}

/// The internal key is not page furniture: it stays in `data-setting` and in the
/// label's tooltip, where a bug report can find it, and out of the text.
#[tokio::test]
async fn a_row_keeps_its_key_out_of_the_text() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    for key in [
        "server.addr",
        "server.max_upload_size_mb",
        "ui.tray_language",
    ] {
        let row = row_of(&html, key);
        assert!(
            row.contains(&format!(r#"title="{key}""#)),
            "{key} must stay discoverable: {row}"
        );
        assert!(
            !text_of(row).contains(key),
            "{key} must not be printed as text: {}",
            text_of(row)
        );
    }

    // The config file's key is in the origin badge's tooltip, not in the row's
    // text; the environment's variable is the one thing that is printed, because
    // it is the one an operator has to change.
    let row = row_of(&html, "server.addr");
    assert!(row.contains(r#"title="config.toml: server.addr""#), "{row}");
    assert!(!text_of(row).contains("config.toml"), "{}", text_of(row));
}

/// A numeric value carries its unit beside the field, not inside its label:
/// "Max upload size" plus `MB` reads as one statement, and the label alone
/// still works in a sentence.
#[tokio::test]
async fn a_numeric_row_shows_its_unit_outside_the_label() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    let row = row_of(&html, "server.max_upload_size_mb");
    assert!(row.contains("Max upload size"), "{row}");
    assert!(
        !row.contains("Max upload size (MB)"),
        "the label must not repeat the unit: {row}"
    );
    assert!(row.contains(">MB<"), "the unit must be rendered: {row}");
    assert!(
        row.contains(r#"type="number""#) && row.contains(r#"min="0""#),
        "the control is a number field: {row}"
    );

    // A setting with no unit gets none, so a bare "0" is not followed by a
    // stray word.
    let row = row_of(&html, "server.port");
    assert!(!row.contains(">MB<"), "{row}");
}

/// An enum's choices are named in the reader's language; the value the server
/// stores is not what an operator is shown.
#[tokio::test]
async fn an_enum_lists_translated_choices() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    let row = row_of(&html, "ui.tray_language");
    assert!(
        row.contains("Follow the operating system"),
        "the choice must be named: {row}"
    );
    assert!(
        row.contains(r#"value="auto""#),
        "the submitted value stays the wire value: {row}"
    );
    assert!(
        !row.contains(">auto<"),
        "the wire value must not double as a label: {row}"
    );
}

#[tokio::test]
async fn saving_from_the_page_applies_and_reports_the_source() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/security/").await;
    let csrf = csrf_of(&html);

    let resp = admin
        .post(format!(
            "{}/sysadmin/settings/security/save/",
            server.base_url
        ))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("section", "security"),
            // A checkbox submits the hidden false too; last one wins.
            ("server.share_link_enabled", "false"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert!(!server.state.config().server.share_link_enabled);

    // The page now reports where the value came from.
    let (_, html) = page(&server, &admin, "/sysadmin/settings/security/").await;
    assert!(
        html.contains("Database") || html.contains("数据库"),
        "the saved row must be shown as the source"
    );

    // And clearing it puts the config file (here: the built-in default) back.
    let csrf = csrf_of(&html);
    let resp = admin
        .post(format!("{}/sysadmin/settings/reset/", server.base_url))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("section", "security"),
            ("key", "server.share_link_enabled"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert!(server.state.config().server.share_link_enabled);
}

#[tokio::test]
async fn a_settings_post_without_csrf_is_rejected() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let resp = admin
        .post(format!(
            "{}/sysadmin/settings/security/save/",
            server.base_url
        ))
        .form(&[("server.share_link_enabled", "false")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert!(
        server.state.config().server.share_link_enabled,
        "nothing may be saved without the token"
    );
}

#[tokio::test]
async fn a_secret_that_cannot_be_decrypted_falls_back_instead_of_being_used() {
    let server = TestServer::start().await;
    let settings = &server.state.settings;

    settings
        .save(
            Section::Email,
            &secret_form("email.password", Some("hunter2")),
            None,
        )
        .await
        .expect("save");
    assert_eq!(
        server.state.config().email.password.as_deref(),
        Some("hunter2")
    );

    // A service over the same rows but a different master secret: the server
    // secret was rotated, so the ciphertext can no longer be read.
    let layers = SettingsLayers {
        base: settings.base().clone(),
        env_keys: Default::default(),
        rows: settings
            .stored_keys()
            .into_iter()
            .filter_map(|key| settings.row(&key).map(|row| (key, row)))
            .collect(),
        policy: settings.policy().clone(),
    };
    let rotated = SettingsService::new(
        server.repos.settings.clone(),
        std::sync::Arc::new(
            infra::crypto::token_encryption::TokenCipher::from_master_key(
                b"a different master secret",
            ),
        ),
        layers,
    );

    // The page says "unreadable, enter it again", and the effective password is
    // *not* the ciphertext — delivery must fail loudly rather than authenticate
    // with a value nobody typed.
    assert_eq!(
        rotated.secret_state("email.password"),
        Some(SecretState::StoredUnreadable)
    );
    assert_eq!(
        rotated.startup().email.password.as_deref(),
        None,
        "the undecryptable value must not become the password"
    );
}

/// The restart button lives on the settings page and nowhere else, so the page
/// has to be reachable and rendered before it can be clicked. It sits in the
/// page header with a form of its own: restarting is not a save, so the button
/// must not be part of the settings form.
#[tokio::test]
async fn the_settings_page_offers_a_restart_button() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (status, html) = page(&server, &admin, "/sysadmin/settings/").await;
    assert_eq!(status, 200);
    // Everything before the first row is the page head, the section bar, the
    // banners and the filter.
    let rows_at = html.find(r#"data-setting=""#).expect("the page has rows");
    let head = &html[..rows_at];
    assert!(
        head.contains(r#"action="/sysadmin/settings/restart/""#),
        "the page header must carry the restart form"
    );
    assert!(
        head.contains("data-restart-button"),
        "the button needs the hook the confirmation script binds to"
    );
}

/// The settings form is the page, not a panel inside one: it must not re-add
/// the padding that would push its rows out of line with the title and the
/// filter above them. And the group heading already draws the rule that closes
/// it, so the first row under it draws no second hairline.
#[tokio::test]
async fn the_settings_form_is_flush_with_the_page() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/").await;
    let form_at = html
        .find(r#"action="/sysadmin/settings/server/save/""#)
        .expect("the save form is on the page");
    let tag_start = html[..form_at].rfind('<').expect("the form is an element");
    let tag_end = form_at + html[form_at..].find('>').expect("the tag closes");
    assert!(
        html[tag_start..tag_end].contains("is-flush"),
        "the settings form must opt out of the panel padding: {}",
        &html[tag_start..tag_end]
    );

    // `server.addr` is the first key of the Server page's first group; the row
    // after it is a normal one.
    let first = row_of(&html, "server.addr");
    let first_tag = &first[..first.find('>').expect("the row's tag closes")];
    assert!(
        !first_tag.contains("border-t"),
        "a group's first row must not draw a top border: {first_tag}"
    );
    let second = row_of(&html, "server.port");
    let second_tag = &second[..second.find('>').expect("the row's tag closes")];
    assert!(
        second_tag.contains("border-t"),
        "every other row keeps its separator: {second_tag}"
    );
}

#[tokio::test]
async fn restarting_from_the_page_returns_a_waiting_page_and_signals_the_runner() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    let csrf = csrf_of(&html);

    let resp = admin
        .post(format!("{}/sysadmin/settings/restart/", server.base_url))
        .form(&[("csrf_token", csrf.as_str()), ("section", "server")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    // The page is a place to wait: the watch hook, the URL to come back to, and
    // no settings form that the browser could submit into a server that is down.
    assert!(body.contains("data-restart-watch"), "{body}");
    assert!(
        body.contains(r#"data-return="/sysadmin/settings/?action=restarted""#),
        "{body}"
    );
    assert!(
        !body.contains(r#"action="/sysadmin/settings/server/save/""#),
        "the restarting page must not render the form"
    );
    // The content is everything before the embedded i18n data block.
    let content = body
        .split(r#"id="__i18n""#)
        .next()
        .expect("the page has a content block");
    assert!(
        !content.contains("setting.restarting_"),
        "a raw locale key reached the page"
    );

    // The signal is raised shortly after the response (so the response is on the
    // wire first); the test server does not consume it, which is what makes it
    // observable here.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !server.state.restart.is_requested() {
        assert!(
            std::time::Instant::now() < deadline,
            "the restart request never reached the run loop"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn restarting_needs_an_admin_and_a_csrf_token() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    // No CSRF token at all.
    let resp = admin
        .post(format!("{}/sysadmin/settings/restart/", server.base_url))
        .form(&[("section", "server")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert!(!server.state.restart.is_requested());

    // A regular account with a valid token for its own session.
    common::create_test_user(&server.db, "pleb@example.com", "password123").await;
    let user = ui_login(&server, "pleb@example.com", "password123").await;
    let (_, html) = page(&server, &user, "/settings/").await;
    let csrf = csrf_of(&html);
    let resp = user
        .post(format!("{}/sysadmin/settings/restart/", server.base_url))
        .form(&[("csrf_token", csrf.as_str()), ("section", "server")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    assert!(
        !server.state.restart.is_requested(),
        "a non-admin must never be able to restart the server"
    );
}

#[tokio::test]
async fn restarting_does_not_save_the_submitted_form() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/security/").await;
    let csrf = csrf_of(&html);

    let before = server.state.config().server.share_link_enabled;
    let resp = admin
        .post(format!("{}/sysadmin/settings/restart/", server.base_url))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("section", "security"),
            ("server.share_link_enabled", "false"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        server.state.config().server.share_link_enabled,
        before,
        "restarting is not a save"
    );
    assert!(
        stored(&server, "server.share_link_enabled").is_none(),
        "no row may be written by the restart request"
    );
}

/// The settings page has to distinguish what the restart button applies from
/// what only a full process restart does — the whole reason the button can be
/// offered honestly.
#[tokio::test]
async fn the_page_separates_in_place_from_process_restarts() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    // `server.tray` and `ui.tray_language` are read before the server loop
    // exists; a stored value for one of them is pending, but *not* pending in
    // the in-place sense.
    server
        .state
        .settings
        .save(Section::Server, &form(&[("ui.tray_language", "zh")]), None)
        .await
        .expect("save");

    assert!(
        server
            .state
            .settings
            .pending_process_restart()
            .contains("ui.tray_language")
    );
    assert!(
        !server
            .state
            .settings
            .pending_restart()
            .contains("ui.tray_language"),
        "the in-app restart cannot apply a process-bootstrap value"
    );

    let (_, html) = page(&server, &admin, "/sysadmin/settings/server/").await;
    let row = html
        .split(r#"data-setting="ui.tray_language""#)
        .nth(1)
        .expect("the row must render");
    assert!(
        row.contains("Needs a full process restart"),
        "the row must say the button cannot apply it"
    );
    // The banner names the key, and it is translated rather than a raw locale id.
    // `page_content` is everything before the embedded dictionary, which is
    // where the rendered body ends.
    let body = html
        .split(r#"id="__i18n""#)
        .next()
        .expect("the page has a content block");
    assert!(
        body.contains("applied only after a full process restart"),
        "the page must explain which values the button cannot reach"
    );
    assert!(body.contains("ui.tray_language"));
}
