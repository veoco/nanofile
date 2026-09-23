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
            Section::General,
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
        .save(Section::General, &form(&[("server.port", "0")]), None)
        .await
        .expect_err("port 0 must be refused");
    assert!(matches!(error, base::error::AppError::BadRequest(_)));
    assert!(settings.stored_keys().is_empty(), "nothing was written");

    // A value that is not a number at all.
    let error = settings
        .save(Section::General, &form(&[("server.port", "eighty")]), None)
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
            Section::General,
            &form(&[("server.share_link_enabled", "false")]),
            None,
        )
        .await
        .expect_err("a Security key cannot be saved from the General page");
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
            Section::Security,
            "auth.max_login_attempts",
            "3",
            infra::settings::Hook::RateLimits,
        ),
        (
            Section::Storage,
            "tasks.max_active_tasks",
            "5",
            infra::settings::Hook::TaskManager,
        ),
        (
            Section::Email,
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
    // `TestServer::start` leaves `[email] enabled` false. The task still has to
    // exist, or turning mail on from the page would need the restart this
    // release removes.
    assert!(!server.state.config().email.enabled);
    assert!(
        server
            .state
            .scheduler
            .handles()
            .iter()
            .any(|handle| handle.name == server::service::mail::TASK_NAME),
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
/// The dictionary ships every key with its translation, so a raw key there is
/// normal; a raw key in the rendered body is not.
fn strip_i18n_dictionary(html: &str) -> &str {
    let Some(start) = html.find("id=\"__i18n\"") else {
        return html;
    };
    match html[start..].find("</script>") {
        Some(end) => &html[start + end..],
        None => html,
    }
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

    for path in [
        "/sysadmin/settings/",
        "/sysadmin/settings/security/",
        "/sysadmin/settings/storage/",
        "/sysadmin/settings/email/",
        "/sysadmin/settings/advanced/",
    ] {
        let (status, html) = page(&server, &admin, path).await;
        assert_eq!(status, 200, "{path}");
        // The catalog is what fills the page, so a page with no rows would mean
        // the section filter is wrong.
        assert!(html.contains("class=\"page-title\""), "{path} has no title");
        // The translation dictionary is embedded in every page, so only the
        // rendered body can be checked for a label that fell back to its key.
        let body = strip_i18n_dictionary(&html);
        for raw in ["setting.section_", "setting.origin_", "setting.server_"] {
            assert!(
                !body.contains(raw),
                "{path} rendered the raw locale key {raw}"
            );
        }
        assert!(
            html.matches("class=\"badge").count() >= 2,
            "{path} rendered no setting rows"
        );
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
/// has to be reachable and rendered before it can be clicked.
#[tokio::test]
async fn the_settings_page_offers_a_restart_button() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (status, html) = page(&server, &admin, "/sysadmin/settings/").await;
    assert_eq!(status, 200);
    assert!(
        html.contains(r#"formaction="/sysadmin/settings/restart/""#),
        "the footer must carry the restart button"
    );
    assert!(
        html.contains("data-restart-button"),
        "the button needs the hook the confirmation script binds to"
    );
}

#[tokio::test]
async fn restarting_from_the_page_returns_a_waiting_page_and_signals_the_runner() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;

    let (_, html) = page(&server, &admin, "/sysadmin/settings/general/").await;
    let csrf = csrf_of(&html);

    let resp = admin
        .post(format!("{}/sysadmin/settings/restart/", server.base_url))
        .form(&[("csrf_token", csrf.as_str()), ("section", "general")])
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
        !body.contains(r#"action="/sysadmin/settings/general/save/""#),
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
        .form(&[("section", "general")])
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
        .form(&[("csrf_token", csrf.as_str()), ("section", "general")])
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
        .save(Section::General, &form(&[("ui.tray_language", "zh")]), None)
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

    let (_, html) = page(&server, &admin, "/sysadmin/settings/general/").await;
    let row = html
        .split(r#"data-setting="ui.tray_language""#)
        .nth(1)
        .expect("the row must render");
    assert!(
        row.contains("Needs a full process restart"),
        "the row must say the button cannot apply it"
    );
    // The banner names the key, and it is translated rather than a raw locale id.
    // `strip_i18n_dictionary` returns the *tail* (the embedded dictionary and the
    // bundle script), so the content is everything before the data block.
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
