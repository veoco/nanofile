//! The two admin task pages, end to end.
//!
//! These drive the real router rather than the view models: what a page renders
//! from a live task system — which jobs exist, which of them this server did not
//! register, and what the journal holds — is not something a unit test can
//! stand in for.

mod common;

use common::TestServer;
use server::i18n::I18n;
use server::repository::job_run::NewJobRun;
use server::tasks::spec::JobKey;

const RUNS: &str = "/sysadmin/tasks/";
const REGISTRY: &str = "/sysadmin/tasks/registered/";

/// Sign in through the Web UI form and keep the session cookies. Redirects are
/// not followed, so a page that turns a request away reports its own status.
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

/// Everything the page renders, without the translation dictionary and the
/// bundle script that follow it. The dictionary ships every key with its
/// translation, so a raw key there is normal; a raw key in the body is not.
fn page_content(html: &str) -> &str {
    html.split("id=\"__i18n\"").next().unwrap_or(html)
}

/// The CSRF token embedded in the registry's trigger form.
fn csrf_of(html: &str) -> String {
    html.split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the registry must embed a csrf_token")
        .to_string()
}

/// One row of a list, from its own handle to the next row's, so a test can look
/// inside one row without matching the rest of the page. `next` is the prefix
/// every row of that list shares.
fn row_of<'h>(html: &'h str, marker: &str, next: &str) -> &'h str {
    let at = html
        .find(marker)
        .unwrap_or_else(|| panic!("no {marker} in the page"));
    let start = html[..at].rfind('<').unwrap_or(at);
    let after = at + marker.len();
    let end = html[after..]
        .find(next)
        .map(|i| after + i)
        .unwrap_or(html.len());
    &html[start..end]
}

/// The locale key naming a job, derived here the same way the page derives it:
/// an independent copy of the rule is what catches the rule changing.
fn job_name_key(slug: &str) -> String {
    format!("admin.job_{}", slug.replace('-', "_"))
}

async fn admin_server() -> (TestServer, reqwest::Client) {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let admin = ui_login(&server, "root@example.com", "password123").await;
    (server, admin)
}

/// Both pages render for an administrator, are one tab apart, and print no
/// locale key: a missing string falls back to its own identifier, which is a
/// defect only a reader would notice.
#[tokio::test]
async fn both_task_pages_render_for_an_admin() {
    let (server, admin) = admin_server().await;

    for path in [RUNS, REGISTRY] {
        let (status, html) = page(&server, &admin, path).await;
        assert_eq!(status, 200, "{path}");
        assert!(html.contains("class=\"page-title\""), "{path} has no title");
        assert!(
            !page_content(&html).contains("admin."),
            "{path} rendered a raw locale key"
        );
        assert_eq!(
            html.matches("class=\"nf-tab\"").count(),
            2,
            "{path} is not one of two task pages"
        );
        assert_eq!(
            html.matches("aria-current=\"page\"").count(),
            1,
            "{path} does not say which page it is"
        );
    }
}

/// Neither page is reachable by an account that is not an administrator.
#[tokio::test]
async fn a_non_admin_is_sent_away_from_both_pages() {
    let server = TestServer::start().await;
    common::create_test_user(&server.db, "user@example.com", "password123").await;
    let user = ui_login(&server, "user@example.com", "password123").await;

    for path in [RUNS, REGISTRY] {
        let (status, _) = page(&server, &user, path).await;
        // `Redirect::to` answers with 303 See Other.
        assert_eq!(status, 303, "{path} was served to a non-admin");
    }
}

/// Every job this server registered has a row, and the row is named in the
/// reader's language rather than by the slug the page is keyed on.
#[tokio::test]
async fn the_registry_names_every_registered_job() {
    let (server, admin) = admin_server().await;
    let (_, html) = page(&server, &admin, REGISTRY).await;
    let body = page_content(&html);
    let t = I18n::get(None);

    let mut registered = 0;
    for key in JobKey::ALL {
        if server.state.tasks.job(*key).is_none() {
            continue;
        }
        registered += 1;
        let slug = key.as_str();
        assert!(
            body.contains(&format!("data-task=\"{slug}\"")),
            "{slug} has no row"
        );
        let key = job_name_key(slug);
        assert!(
            body.contains(t.tr(&key)),
            "{slug} is not named in the reader's language"
        );
    }
    assert!(registered > 0, "a server with no jobs registers nothing");

    let services = server.state.tasks.services();
    for key in &services {
        assert!(
            body.contains(&format!("data-task=\"{}\"", key.as_str())),
            "{key:?} has no row"
        );
    }
}

/// A declared job this server did not register is named with the switch that
/// would bring it back, rather than left out of the page.
#[tokio::test]
async fn the_registry_names_the_jobs_it_did_not_register() {
    let (server, admin) = admin_server().await;
    let skipped = server.state.tasks.skipped();
    assert!(
        !skipped.is_empty(),
        "the default configuration does not run every declared job, so there is \
         something to report"
    );

    let (_, html) = page(&server, &admin, REGISTRY).await;
    let body = page_content(&html);
    for (key, _) in &skipped {
        let marker = format!("data-task-skipped=\"{}\"", key.as_str());
        assert!(body.contains(&marker), "{key:?} is not reported as missing");
        let row = row_of(body, &marker, "data-task-skipped=\"");
        assert!(
            !row.contains("admin."),
            "{key:?} reports its reason by locale key"
        );
    }
    assert!(body.contains("Not registered on this server"));
}

/// A journalled run is named, labelled with its verdict in the reader's
/// language, and never printed as the slug and wire phase the database holds.
#[tokio::test]
async fn a_journal_row_is_named_and_state_labelled() {
    let (server, admin) = admin_server().await;

    // A run as the journal would hold it: queued, then finished.
    let journal = server.state.repos.job_run.clone();
    let now = chrono::Utc::now().timestamp();
    journal
        .enqueue(
            NewJobRun {
                id: "journalled-run".to_string(),
                kind: JobKey::GarbageCollection.as_str().to_string(),
                owner: None,
                summary: "nothing to remove".to_string(),
                params: None,
                created_at: now,
            },
            now + 300,
        )
        .await
        .unwrap();
    journal
        .finish("journalled-run", "succeeded", None, Some(4), now + 1)
        .await
        .unwrap();

    let (_, html) = page(&server, &admin, RUNS).await;
    let row = row_of(
        page_content(&html),
        "data-run=\"journalled-run\"",
        // The load panel starts the next block; there is no second run row.
        "class=\"nf-list\"",
    );

    assert!(row.contains("Garbage collection"), "{row}");
    assert!(
        row.contains("badge-green") && row.contains(">Succeeded</span>"),
        "the verdict is not labelled: {row}"
    );
    assert!(
        !row.contains("succeeded"),
        "the wire phase reached the page: {row}"
    );
    assert!(!row.contains(">gc<"), "the slug reached the page: {row}");
    // The submitter is named, and a scheduled run has no account to name.
    assert!(row.contains("The server"), "{row}");
    // The run's own report and its id are one disclosure away.
    assert!(row.contains("journalled-run"), "{row}");
    assert!(row.contains("nothing to remove"), "{row}");
}

/// Triggering a job answers the browser with the page its button lives on,
/// carrying the confirmation.
#[tokio::test]
async fn triggering_redirects_back_to_the_registry() {
    let (server, admin) = admin_server().await;
    let (_, html) = page(&server, &admin, REGISTRY).await;
    let csrf = csrf_of(&html);

    let resp = admin
        .post(format!(
            "{}/sysadmin/tasks/share-link-cleanup/trigger/",
            server.base_url
        ))
        .form(&[("csrf_token", &csrf)])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers()["location"],
        "/sysadmin/tasks/registered/?action=triggered"
    );

    let (status, html) = page(
        &server,
        &admin,
        "/sysadmin/tasks/registered/?action=triggered",
    )
    .await;
    assert_eq!(status, 200);
    assert!(html.contains("nf-banner is-ok"), "no confirmation banner");
    assert!(html.contains("Task triggered"));
}

/// A browser form gets a page back whatever happens: a slug no job answers to,
/// and a job that cannot be run by hand, both report on the registry rather
/// than answering with the API's JSON error body.
#[tokio::test]
async fn a_refused_trigger_reports_on_the_registry() {
    let (server, admin) = admin_server().await;
    let (_, html) = page(&server, &admin, REGISTRY).await;
    let csrf = csrf_of(&html);

    for (slug, expected) in [
        ("no-such-task", "No task named"),
        ("copy", "cannot be triggered by hand"),
    ] {
        let resp = admin
            .post(format!(
                "{}/sysadmin/tasks/{slug}/trigger/",
                server.base_url
            ))
            .form(&[("csrf_token", &csrf)])
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200, "{slug}");
        assert!(
            resp.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains("text/html"),
            "{slug} answered with something other than a page"
        );
        let body = resp.text().await.unwrap();
        assert!(body.contains("nf-banner is-err"), "{slug} has no banner");
        assert!(body.contains(expected), "{slug} did not say why: {body}");
        assert!(
            !body.contains("error_msg"),
            "{slug} answered with the API's error body"
        );
    }
}
