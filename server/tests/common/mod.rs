#![allow(dead_code, unused_imports)]

pub mod client;

use axum::Router;
use sea_orm::{ActiveModelTrait, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use server::AppState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Cache PBKDF2 password hashes so each unique password is hashed only once
/// across all test fixtures (~200+ calls → ~3-5 calls, saving ~2s).
static PASSWORD_HASH_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// PBKDF2 iterations used for password hashing in tests.
/// Uses a low value to keep test execution fast.
const TEST_PBKDF2_ITERATIONS: u32 = 1000;

fn cached_hash_password(password: &str) -> String {
    let mut cache = PASSWORD_HASH_CACHE.lock().unwrap();
    if let Some(h) = cache.get(password) {
        return h.clone();
    }
    let h = server::auth::password::hash_password(password, TEST_PBKDF2_ITERATIONS);
    cache.insert(password.to_string(), h.clone());
    h
}

pub struct TestServer {
    pub base_url: String,
    pub db: Arc<DatabaseConnection>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with_config(false, false, 30, 90).await
    }

    pub async fn start_with_notification() -> Self {
        Self::start_with_config(true, false, 30, 90).await
    }

    pub async fn start_with_index() -> Self {
        Self::start_with_config(false, true, 30, 90).await
    }

    /// Start a server with notification and custom keepalive intervals.
    /// Useful for testing keepalive/ping-pong behavior with short timeouts.
    pub async fn start_with_custom_keepalive(ping_interval: u64, client_timeout: u64) -> Self {
        Self::start_with_config(true, false, ping_interval, client_timeout).await
    }

    async fn start_with_config(
        enable_notification: bool,
        enable_index: bool,
        ping_interval: u64,
        client_timeout: u64,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{}", port);

        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("failed to connect to test db");

        migration::Migrator::up(&db, None)
            .await
            .expect("failed to run migrations");

        // Build minimal config for AppState — only block_dir matters for tests.
        let config = server::config::Config {
            server: server::config::ServerConfig {
                addr: "127.0.0.1".to_string(),
                port,
                site_url: format!("http://127.0.0.1:{}", port),
                max_upload_size_mb: 512,
                request_timeout_secs: 36000,
                cors_allowed_origins: vec![],
                cors_max_age_secs: 86400,
            },
            database: server::config::DatabaseConfig {
                url: "sqlite::memory:".to_string(),
            },
            storage: server::config::StorageConfig {
                block_dir: std::env::temp_dir()
                    .join(format!("nf-test-{}", port))
                    .join("blocks"),
                temp_dir: std::env::temp_dir()
                    .join(format!("nf-test-{}", port))
                    .join("tmp"),
                max_storage_bytes: 10_737_418_240,
            },
            auth: server::config::AuthConfig {
                password_hash_iterations: 1000,
                api_token_ttl_days: 180,
                sync_token_ttl_days: 180,
                max_login_attempts: 5,
                lockout_duration_secs: 300,
                enable_invitations: true,
                enable_password_reset: true,
                password_min_length: 8,
                require_strong_password: false,
                password_reset_max_per_hour: 10,
                registration_max_per_hour: 10,
                totp_max_attempts: 10,
            },
            logging: server::config::LoggingConfig {
                level: "debug".to_string(),
            },
            gc: server::config::GcConfig {
                enabled: false,
                interval_hours: 24,
                keep_commits: 10,
            },
            notification: server::config::NotificationConfig {
                enabled: enable_notification,
                private_key: if enable_notification {
                    "test-notification-secret".to_string()
                } else {
                    String::new()
                },
                ping_interval,
                client_timeout,
            },
            index: server::config::IndexConfig {
                enabled: enable_index,
                index_dir: std::env::temp_dir().join(format!("nf-test-{}-index", port)),
            },
            admin_init: Default::default(),
        };
        // Ensure block directory exists
        std::fs::create_dir_all(&config.storage.block_dir).unwrap();

        let state = Arc::new(AppState::new(db, config));

        let sync_routes = server::sync::sync_routes();
        let sdoc_routes = server::sdoc::sdoc_routes();
        let web_routes = server::web::web_routes();
        let ui_routes = server::ui::ui_routes();
        let notification_routes = server::notification::notification_routes();

        let app = Router::new()
            .merge(server::routes::api_routes())
            .merge(sync_routes)
            .merge(sdoc_routes)
            .merge(web_routes)
            .merge(ui_routes)
            .merge(notification_routes)
            .route(
                "/static/{*path}",
                axum::routing::get(server::static_assets::serve_static),
            )
            .layer(axum::extract::DefaultBodyLimit::max(512 * 1024 * 1024))
            .with_state(state.clone());

        // Debug: Print info about the server
        tracing::info!("TestServer started on port {port}");

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("server failed");
        });

        // Poll until the server is actually ready (typically <1ms).
        // Use no_proxy() to avoid proxy resolution delays even when
        // http_proxy is set in the environment.
        let health_url = format!("{}/api2/ping/", base_url);
        let health_client = reqwest::Client::builder().no_proxy().build().unwrap();
        for _ in 0..50 {
            if health_client.get(&health_url).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        Self {
            base_url,
            db: state.db.clone(),
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn client(&self) -> client::TestClient {
        client::TestClient::new(&self.base_url)
    }

    /// Create a client with cookie store enabled (for Web UI tests).
    pub fn client_ui(&self) -> client::TestClient {
        client::TestClient::new_with_cookies(&self.base_url)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn create_test_user(db: &DatabaseConnection, email: &str, password: &str) -> i32 {
    let password_hash = cached_hash_password(password);
    let now = chrono::Utc::now().timestamp();

    let user = server::entity::user::ActiveModel {
        id: sea_orm::NotSet,
        email: sea_orm::Set(email.to_string()),
        password_hash: sea_orm::Set(password_hash),
        is_active: sea_orm::Set(true),
        is_admin: sea_orm::Set(false),
        created_at: sea_orm::Set(now),
        last_login_at: sea_orm::NotSet,
        invited_by: sea_orm::Set(None),
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
    };

    user.insert(db).await.unwrap().id
}

pub async fn create_test_admin(db: &DatabaseConnection, email: &str, password: &str) -> i32 {
    let password_hash = server::auth::password::hash_password(password, TEST_PBKDF2_ITERATIONS);
    let now = chrono::Utc::now().timestamp();

    let user = server::entity::user::ActiveModel {
        id: sea_orm::NotSet,
        email: sea_orm::Set(email.to_string()),
        password_hash: sea_orm::Set(password_hash),
        is_active: sea_orm::Set(true),
        is_admin: sea_orm::Set(true),
        created_at: sea_orm::Set(now),
        last_login_at: sea_orm::NotSet,
        invited_by: sea_orm::Set(None),
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
    };

    user.insert(db).await.unwrap().id
}

pub async fn create_test_repo(client: &client::TestClient, token: &str, name: &str) -> String {
    let resp = client.create_repo(token, name).await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

pub async fn get_sync_token(client: &client::TestClient, api_token: &str, repo_id: &str) -> String {
    let resp = client.download_info(api_token, repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// Opinionated test fixture that sets up a server, user, repo, and tokens.
///
/// ```
/// let f = TestFixture::new().await;
/// // f.client, f.api_token, f.repo_id, f.sync_token, f.user_id are all ready.
/// ```
pub struct TestFixture {
    pub server: TestServer,
    pub client: client::TestClient,
    pub email: String,
    pub password: String,
    pub api_token: String,
    pub repo_id: String,
    pub sync_token: String,
    pub user_id: i32,
}

impl TestFixture {
    /// Create a full test environment with one user and one repo.
    ///
    /// Default user: `test@example.com` / `password`
    /// Default repo: `test-repo`
    pub async fn new() -> Self {
        Self::with("test@example.com", "password", "test-repo").await
    }

    /// Create a test environment with notification server enabled.
    pub async fn new_with_notification() -> Self {
        let server = TestServer::start_with_notification().await;
        let client = server.client();
        let db = &*server.db;

        let user_id = create_test_user(db, "test@example.com", "password").await;

        let resp = client.login("test@example.com", "password").await;
        assert_eq!(resp.status(), 200, "login failed");
        let body: serde_json::Value = resp.json().await.unwrap();
        let api_token = body["token"].as_str().unwrap().to_string();

        let repo_id = create_test_repo(&client, &api_token, "test-repo").await;
        let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

        Self {
            server,
            client,
            email: "test@example.com".to_string(),
            password: "password".to_string(),
            api_token,
            repo_id,
            sync_token,
            user_id,
        }
    }

    /// Create with a custom user and repo name.
    pub async fn with(email: &str, password: &str, repo_name: &str) -> Self {
        let server = TestServer::start().await;
        let client = server.client();
        let db = &*server.db;

        let user_id = create_test_user(db, email, password).await;

        // Login to get API token
        let resp = client.login(email, password).await;
        assert_eq!(resp.status(), 200, "login failed for {email}");
        let body: serde_json::Value = resp.json().await.unwrap();
        let api_token = body["token"].as_str().unwrap().to_string();

        // Create a repo and get its sync token
        let repo_id = create_test_repo(&client, &api_token, repo_name).await;
        let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

        Self {
            server,
            client,
            email: email.to_string(),
            password: password.to_string(),
            api_token,
            repo_id,
            sync_token,
            user_id,
        }
    }

    /// Create a test environment with indexer enabled.
    pub async fn new_with_index() -> Self {
        let server = TestServer::start_with_index().await;
        let client = server.client();
        let db = &*server.db;

        let user_id = create_test_user(db, "test@example.com", "password").await;

        let resp = client.login("test@example.com", "password").await;
        assert_eq!(resp.status(), 200, "login failed");
        let body: serde_json::Value = resp.json().await.unwrap();
        let api_token = body["token"].as_str().unwrap().to_string();

        let repo_id = create_test_repo(&client, &api_token, "test-repo").await;
        let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

        Self {
            server,
            client,
            email: "test@example.com".to_string(),
            password: "password".to_string(),
            api_token,
            repo_id,
            sync_token,
            user_id,
        }
    }

    /// Create a test environment with a user but no repo (for tests that don't need one).
    pub async fn no_repo(email: &str, password: &str) -> Self {
        let server = TestServer::start().await;
        let client = server.client();
        let db = &*server.db;

        let user_id = create_test_user(db, email, password).await;

        let resp = client.login(email, password).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        let api_token = body["token"].as_str().unwrap().to_string();

        Self {
            server,
            client,
            email: email.to_string(),
            password: password.to_string(),
            api_token,
            repo_id: String::new(),
            sync_token: String::new(),
            user_id,
        }
    }
}

/// Lightweight environment returned by `SharedFixture::new_env()`.
/// Does NOT own the server — the server is shared across all test_envs.
pub struct TestEnv {
    pub client: client::TestClient,
    pub base_url: String,
    pub db: Arc<DatabaseConnection>,
    pub email: String,
    pub password: String,
    pub api_token: String,
    pub repo_id: String,
    pub sync_token: String,
    pub user_id: i32,
}

/// Shared test fixture that starts a single server per test binary.
///
/// Use via `tokio::sync::OnceCell` for lazy initialization:
///
/// ```ignore
/// static SHARED: tokio::sync::OnceCell<SharedFixture> = tokio::sync::OnceCell::const_new();
///
/// #[tokio::test]
/// async fn my_test() {
///     let shared = SHARED.get_or_init(|| async { SharedFixture::new().await }).await;
///     let env = shared.new_env().await;
///     // env.client, env.api_token, env.repo_id are ready
/// }
/// ```
pub struct SharedFixture {
    server: TestServer,
    counter: AtomicU32,
}

impl SharedFixture {
    /// Start a single server and prepare for multiple test environments.
    pub async fn new() -> Self {
        Self {
            server: TestServer::start().await,
            counter: AtomicU32::new(1),
        }
    }

    /// Create an isolated (user, repo) pair on the shared server.
    /// Each call returns a unique email to guarantee isolation.
    pub async fn new_env(&self) -> TestEnv {
        let uid = self.counter.fetch_add(1, Ordering::Relaxed);
        let email = format!("test{}@example.com", uid);
        let password = "password";
        let repo_name = format!("test-repo-{}", uid);

        let client = self.server.client();
        let db = &*self.server.db;

        let user_id = create_test_user(db, &email, password).await;
        let resp = client.login(&email, password).await;
        assert_eq!(resp.status(), 200, "login failed for {email}");
        let body: serde_json::Value = resp.json().await.unwrap();
        let api_token = body["token"].as_str().unwrap().to_string();
        let repo_id = create_test_repo(&client, &api_token, &repo_name).await;
        let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

        TestEnv {
            client,
            base_url: self.server.base_url.clone(),
            db: self.server.db.clone(),
            email,
            password: password.to_string(),
            api_token,
            repo_id,
            sync_token,
            user_id,
        }
    }

    /// Create an isolated user (without a repo) on the shared server.
    pub async fn new_user(&self) -> TestEnv {
        let uid = self.counter.fetch_add(1, Ordering::Relaxed);
        let email = format!("test{}@example.com", uid);
        let password = "password";

        let client = self.server.client();
        let db = &*self.server.db;

        let user_id = create_test_user(db, &email, password).await;
        let resp = client.login(&email, password).await;
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        let api_token = body["token"].as_str().unwrap().to_string();

        TestEnv {
            client,
            base_url: self.server.base_url.clone(),
            db: self.server.db.clone(),
            email,
            password: password.to_string(),
            api_token,
            repo_id: String::new(),
            sync_token: String::new(),
            user_id,
        }
    }
}
