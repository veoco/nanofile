//! The first-run setup page: what it serves, and what a completed submit writes.
//!
//! The page is driven through the same connection loop the binary ships, over a
//! real socket, because the properties that matter are the ones a browser sees:
//! the token gates every route, a rejected form comes back with what was typed,
//! and a completed form leaves a configuration file, a database and exactly one
//! administrator behind.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};
use server::setup::{self, SetupDefaults, SetupState};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Harness {
    base_url: String,
    config_path: PathBuf,
    data_dir: PathBuf,
    state: Arc<SetupState>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    _tmp: tempfile::TempDir,
    client: reqwest::Client,
}

impl Harness {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".nanofile/config.toml");
        let data_dir = tmp.path().join("data");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        // The iteration count is the test's own: the default (600k PBKDF2
        // rounds) is right for a deployment and far too slow for a test binary.
        let mut config = infra::config::Config::default();
        config.auth.password_hash_iterations = 1000;
        config.auth.password_min_length = 8;

        let defaults = SetupDefaults {
            addr: addr.ip().to_string(),
            port: addr.port(),
            site_url: base_url.clone(),
            data_dir: data_dir.to_string_lossy().to_string(),
            encryption: "off".into(),
            password_min_length: config.auth.password_min_length,
            require_strong_password: false,
            language: "en".into(),
            port_in_use: false,
            // The page is already being served on this address, which is the one
            // the submitted form names by default: it counts as available.
            held_address: Some(addr.to_string()),
            existing_accounts: None,
        };

        let state = SetupState::new(TOKEN.to_string(), config_path.clone(), config, defaults);
        let app = setup::setup_router(state.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            server::serve::serve_with_timeouts(
                listener,
                app,
                shutdown_rx,
                Some(Duration::from_secs(30)),
            )
            .await
            .expect("setup server failed");
        });

        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        let harness = Self {
            base_url,
            config_path,
            data_dir,
            state,
            shutdown: Some(shutdown_tx),
            _tmp: tmp,
            client,
        };
        harness.wait_until_ready().await;
        harness
    }

    async fn wait_until_ready(&self) {
        for _ in 0..100 {
            if self
                .client
                .get(format!("{}/health", self.base_url))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the setup page never became reachable");
    }

    fn form_url(&self) -> String {
        format!("{}/setup/{TOKEN}", self.base_url)
    }

    /// A valid submission, with the fields a test wants to change replaced.
    fn form(&self) -> Vec<(String, String)> {
        vec![
            ("email".into(), "admin@example.com".into()),
            ("password".into(), "secret123".into()),
            ("password_confirm".into(), "secret123".into()),
            ("addr".into(), self.state_defaults_addr()),
            ("port".into(), self.state_defaults_port()),
            ("site_url".into(), String::new()),
            (
                "data_dir".into(),
                self.data_dir.to_string_lossy().to_string(),
            ),
            ("encryption".into(), "off".into()),
        ]
    }

    fn state_defaults_addr(&self) -> String {
        self.base_url
            .trim_start_matches("http://")
            .rsplit_once(':')
            .map(|(host, _)| host.to_string())
            .expect("the harness URL carries an authority")
    }

    fn state_defaults_port(&self) -> String {
        self.base_url
            .rsplit_once(':')
            .map(|(_, port)| port.to_string())
            .expect("the harness URL carries a port")
    }

    fn with(mut form: Vec<(String, String)>, key: &str, value: &str) -> Vec<(String, String)> {
        for (name, current) in form.iter_mut() {
            if name == key {
                *current = value.to_string();
            }
        }
        form
    }

    fn config_file(&self) -> &std::path::Path {
        &self.config_path
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

#[tokio::test]
async fn the_page_is_gated_by_the_token_and_never_leaks_it() {
    let harness = Harness::start().await;

    let root = harness
        .client
        .get(format!("{}/", harness.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(root.status(), 200);
    let body = root.text().await.unwrap();
    assert!(
        !body.contains(TOKEN),
        "the root page must not hand out the setup token"
    );

    let wrong = harness
        .client
        .get(format!("{}/setup/not-the-token", harness.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 404);

    let wrong = harness
        .client
        .post(format!("{}/setup/not-the-token", harness.base_url))
        .form(&harness.form())
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 404);
    assert!(harness.state.result().await.is_none());

    // The health endpoint exists so a container is not restarted while a person
    // is filling the form in.
    let health = harness
        .client
        .get(format!("{}/health", harness.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 204);
}

#[tokio::test]
async fn the_form_asks_for_everything_the_plan_needs() {
    let harness = Harness::start().await;
    let page = harness.client.get(harness.form_url()).send().await.unwrap();
    assert_eq!(page.status(), 200);
    let body = page.text().await.unwrap();
    for field in [
        "name=\"email\"",
        "name=\"password\"",
        "name=\"password_confirm\"",
        "name=\"addr\"",
        "name=\"port\"",
        "name=\"site_url\"",
        "name=\"data_dir\"",
        "name=\"encryption\"",
    ] {
        assert!(body.contains(field), "the form is missing {field}");
    }
    assert!(
        body.contains(&harness.config_path.display().to_string()),
        "the page names the file it is about to write"
    );
}

#[tokio::test]
async fn a_valid_submit_writes_the_configuration_and_creates_the_administrator() {
    let harness = Harness::start().await;
    let response = harness
        .client
        .post(harness.form_url())
        .form(&harness.form())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("Setup complete"), "{body}");
    assert!(body.contains("admin@example.com"), "{body}");

    // The configuration file exists, is owner-only, and loads into the values the
    // page collected.
    let text = std::fs::read_to_string(harness.config_file()).expect("the config was written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(harness.config_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the file holds the master secret");
    }
    let config = infra::config::Config::load_from(harness.config_file()).expect("it loads");
    let expected_port: u16 = harness.state_defaults_port().parse().unwrap();
    assert_eq!(config.server.port, expected_port);
    assert_eq!(
        config.server.addr,
        harness.state_defaults_addr(),
        "the form's listen address is what the file records"
    );
    assert_eq!(config.server.secret_key.len(), 64);
    assert_eq!(
        config.database.url,
        format!(
            "sqlite:{}?mode=rwc",
            harness.data_dir.join("nanofile.db").display()
        )
    );
    assert_eq!(config.storage.block_dir, harness.data_dir.join("blocks"));
    assert_eq!(config.index.index_dir, harness.data_dir.join("index"));
    assert!(text.contains("[server]"), "{text}");

    // The database holds exactly one account, and it is an administrator that can
    // sign in with the password the form carried.
    let db = infra::db::establish_connection(&config.database)
        .await
        .expect("database");
    let users = infra::entity::user::Entity::find().all(&db).await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].email, "admin@example.com");
    assert!(users[0].is_admin);
    assert!(users[0].is_active);

    // The page's completion is observable, so the runner waiting in `main` moves
    // on to serving.
    let result = harness.state.result().await.expect("the setup completed");
    assert_eq!(result.created_email.as_deref(), Some("admin@example.com"));
    assert_eq!(result.accounts_kept, 0);
    assert_eq!(result.site_url, harness.base_url);
}

#[tokio::test]
async fn a_rejected_form_keeps_the_answers_and_writes_nothing() {
    let harness = Harness::start().await;
    let form = Harness::with(harness.form(), "port", "0");
    let response = harness
        .client
        .post(harness.form_url())
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body = response.text().await.unwrap();
    // The message names the rule...
    assert!(body.contains("port between 1 and 65535"), "{body}");
    // ...and the form comes back with what the operator typed, not blank.
    assert!(body.contains("admin@example.com"), "{body}");
    assert!(!harness.config_file().exists());
    assert!(harness.state.result().await.is_none());
}

#[tokio::test]
async fn a_mismatched_confirmation_is_refused() {
    let harness = Harness::start().await;
    let form = Harness::with(harness.form(), "password_confirm", "different1");
    let response = harness
        .client
        .post(harness.form_url())
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(response.text().await.unwrap().contains("do not match"));
    assert!(!harness.config_file().exists());
}

#[tokio::test]
async fn an_existing_configuration_is_never_overwritten() {
    let harness = Harness::start().await;
    std::fs::create_dir_all(harness.config_file().parent().unwrap()).unwrap();
    std::fs::write(harness.config_file(), "[server]\nport = 1\n").unwrap();

    let response = harness
        .client
        .post(harness.form_url())
        .form(&harness.form())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        std::fs::read_to_string(harness.config_file()).unwrap(),
        "[server]\nport = 1\n",
        "a configuration that exists is somebody else's"
    );
    assert!(harness.state.result().await.is_none());
}

#[tokio::test]
async fn an_account_that_already_exists_is_kept_instead_of_replaced() {
    let harness = Harness::start().await;

    // A configuration file that was deleted while its data stayed behind: the
    // data directory is already there, with an account in it.
    std::fs::create_dir_all(&harness.data_dir).unwrap();
    let mut config = infra::config::Config::default();
    config.database.url = format!(
        "sqlite:{}?mode=rwc",
        harness.data_dir.join("nanofile.db").display()
    );
    let db = infra::db::establish_connection(&config.database)
        .await
        .unwrap();
    use sea_orm_migration::MigratorTrait;
    migration::Migrator::up(&db, None).await.unwrap();
    infra::entity::user::ActiveModel {
        id: sea_orm::NotSet,
        email: Set("existing@example.com".to_string()),
        password_hash: Set("pbkdf2_sha256$1000$aa$bb".to_string()),
        is_active: Set(true),
        is_admin: Set(true),
        created_at: Set(0),
        last_login_at: Set(None),
        email_verified: Set(true),
        invited_by: Set(None),
        storage_quota: sea_orm::NotSet,
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
        language: sea_orm::NotSet,
    }
    .insert(&db)
    .await
    .unwrap();

    let response = harness
        .client
        .post(harness.form_url())
        .form(&harness.form())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("existing account(s) were kept"), "{body}");

    let count = infra::entity::user::Entity::find()
        .count(&db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let users = infra::entity::user::Entity::find().all(&db).await.unwrap();
    assert_eq!(users[0].email, "existing@example.com");

    let result = harness.state.result().await.expect("the setup completed");
    assert_eq!(result.created_email, None);
    assert_eq!(result.accounts_kept, 1);
    assert!(harness.config_file().is_file());
}
