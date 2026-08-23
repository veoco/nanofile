use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header::HeaderValue;
use axum::http::{Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use rand::Rng;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use sea_orm_migration::MigratorTrait;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tracing_subscriber::EnvFilter;

use infra::config::Config;
use infra::db::establish_connection;
use server::AppState;

/// Nanofile — a Seafile-compatible sync server
#[derive(Parser)]
#[command(name = "nanofile", version, about)]
struct Cli {
    /// Path to config.toml (overrides NANOFILE_CONFIG and the default).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Parser)]
enum Command {
    /// Start the HTTP server (default)
    Server,
    /// Create a new user account (admin by default, use --regular for non-admin)
    Adduser {
        /// Email address (also used as login name)
        #[arg(long)]
        email: Option<String>,
        /// Password (prompted interactively if not provided)
        #[arg(long)]
        password: Option<String>,
        /// Create a regular (non-admin) user
        #[arg(long, default_value_t = false)]
        regular: bool,
    },
}

async fn health_check() -> impl IntoResponse {
    StatusCode::OK
}

/// Add baseline security headers to every response.
///
/// `script-src 'self' 'unsafe-inline'` keeps the inline dark-mode guard,
/// view-mode probe and `window.__T` i18n injection in `base.html` working,
/// while still blocking remote / third-party script, image, frame and font
/// loading (`default-src 'self'` — no fallback to `*`). `style-src
/// 'unsafe-inline'` is needed for the inline `<style>` block and `style=""`
/// attributes. A strict nonce-based `script-src` (dropping `'unsafe-inline'`)
/// is a possible follow-up.
async fn security_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
             font-src 'self'; connect-src 'self' ws: wss:; \
             object-src 'none'; frame-ancestors 'none'; base-uri 'self'; \
             form-action 'self'",
        ),
    );
    response
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut config = match &cli.config {
        Some(path) => Config::load_from(path)?,
        None => Config::load()?,
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .parse_lossy(&config.logging.level),
        )
        .init();

    // ── Server secret key: auto-generate if empty ──────────────────────
    if config.server.secret_key.is_empty() || config.server.secret_key == "nanofile-server-secret" {
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        config.server.secret_key = hex::encode(key);
        tracing::warn!(
            "Auto-generated server secret key. \
             Set NANOFILE_SERVER_SECRET_KEY to persist across restarts."
        );
    }

    // ── Derive notification private key from secret_key if not set ─────
    if config.notification.private_key.is_empty()
        || config.notification.private_key == "nanofile-notification-secret"
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"notify-v1:");
        hasher.update(config.server.secret_key.as_bytes());
        config.notification.private_key = hex::encode(hasher.finalize());
    }

    let db = establish_connection(&config.database).await?;
    migration::Migrator::up(&db, None).await?;

    match cli.command.unwrap_or(Command::Server) {
        Command::Server => {
            tracing::info!(
                "starting nanofile server on {}:{}",
                config.server.addr,
                config.server.port
            );

            // Ensure data directories exist with restrictive permissions so
            // other local users cannot read the SQLite DB or file blocks.
            for dir in [&config.storage.block_dir, &config.storage.temp_dir] {
                std::fs::create_dir_all(dir)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                }
            }

            let temp_file_manager = server::handler::web::temp_file::TempFileManager::new(
                config.storage.temp_dir.clone(),
            )
            .await;

            let state = Arc::new(AppState::new(db, config.clone(), temp_file_manager));

            // ── Auto-create admin user from config/env on first startup ──────
            if let (Some(admin_email), Some(admin_password)) = (
                &state.config.admin_init.email,
                &state.config.admin_init.password,
            ) {
                let count = infra::entity::user::Entity::find()
                    .count(state.db.as_ref())
                    .await?;
                if count == 0 {
                    tracing::info!("No users found; creating initial admin user");
                    let password_hash = server::service::auth::password::hash_password(
                        admin_password,
                        state.config.auth.password_hash_iterations,
                    );
                    let now = chrono::Utc::now().timestamp();
                    let model = infra::entity::user::ActiveModel {
                        id: sea_orm::NotSet,
                        email: Set(admin_email.clone()),
                        password_hash: Set(password_hash),
                        is_active: Set(true),
                        is_admin: Set(true),
                        created_at: Set(now),
                        last_login_at: Set(None),
                        invited_by: Set(None),
                        storage_quota: sea_orm::NotSet,
                        name: sea_orm::NotSet,
                        display_name: sea_orm::NotSet,
                        language: sea_orm::NotSet,
                    };
                    model.insert(state.db.as_ref()).await?;
                    tracing::info!("Admin user '{}' created", admin_email);
                } else {
                    tracing::debug!(
                        "Users already exist (count={}), skipping admin auto-creation",
                        count,
                    );
                }
            }

            let cors = {
                // `cors_origins()` returns `[site_url_origin()]` when the
                // configured list is empty, so this always allows the same-origin
                // site (and any explicitly configured origins).
                let origins = state.config.server.cors_origins();

                CorsLayer::new()
                    .allow_origin(AllowOrigin::list(origins.into_iter().filter_map(|o| {
                        o.parse()
                            .map_err(|e| {
                                tracing::warn!("Skipping invalid CORS origin '{}': {:?}", o, e)
                            })
                            .ok()
                    })))
                    .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                    .allow_headers([
                        header::AUTHORIZATION,
                        header::CONTENT_TYPE,
                        header::HeaderName::from_static("x-requested-with"),
                        header::HeaderName::from_static("x-seafile-otp"),
                        header::HeaderName::from_static("x-seafile-s2fa"),
                        header::HeaderName::from_static("x-seafile-sharelink-password"),
                        header::HeaderName::from_static("x-seafile-2fa-trust-device"),
                    ])
                    .max_age(std::time::Duration::from_secs(
                        state.config.server.cors_max_age_secs,
                    ))
            };

            let sync_routes = server::handler::sync::sync_routes();
            let web_routes = server::handler::web::web_routes();
            let ui_routes = server::ui::ui_routes();
            let notification_routes = server::notification::notification_routes();
            let webdav_routes = server::webdav::webdav_routes();

            // CORS is applied only to the REST API routes. It must not wrap
            // the WebDAV endpoints: tower-http's CorsLayer answers OPTIONS
            // requests itself, which would shadow the `DAV:`/`Allow:` response
            // WebDAV clients expect. WebDAV clients are not browsers, so CORS
            // does not apply to them.
            let api_with_cors = server::routes::api_routes().layer(cors);

            let app = Router::new()
                .route("/health", get(health_check))
                .merge(api_with_cors)
                .merge(sync_routes)
                .merge(web_routes)
                .merge(ui_routes)
                .merge(notification_routes)
                .merge(webdav_routes)
                .merge(server::handler::avatar::image_routes())
                .route("/static/{*path}", get(server::static_assets::serve_static))
                .layer(DefaultBodyLimit::max(
                    (config.server.max_upload_size_mb * 1024 * 1024) as usize,
                ))
                .layer(RequestBodyLimitLayer::new(
                    (config.server.max_upload_size_mb * 1024 * 1024) as usize,
                ))
                .layer(
                    tower_http::trace::TraceLayer::new_for_http()
                        .on_request(
                            tower_http::trace::DefaultOnRequest::new().level(tracing::Level::INFO),
                        )
                        .on_response(
                            tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO),
                        ),
                )
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    std::time::Duration::from_secs(config.server.request_timeout_secs),
                ))
                .layer(axum::middleware::from_fn(security_headers))
                .with_state(state.clone());

            let addr = format!("{}:{}", config.server.addr, config.server.port);
            tracing::info!("listening on {}", addr);

            let listener = tokio::net::TcpListener::bind(&addr).await?;

            // ── Start server with graceful shutdown via oneshot ─────────────
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

            // ConnectInfo exposes the TCP peer address to handlers so rate
            // limiting can use the real client IP instead of the spoofable
            // X-Forwarded-For header.
            let serve_fut = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });

            // Spawn the server in the background so it starts accepting immediately.
            let server_handle = tokio::spawn(async move { serve_fut.await });

            // ── Wait for Ctrl+C or SIGTERM ─────────────────────────────────
            let ctrl_c = tokio::signal::ctrl_c();
            let terminate = async {
                #[cfg(unix)]
                {
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("failed to install SIGTERM handler")
                        .recv()
                        .await;
                }
                #[cfg(not(unix))]
                std::future::pending::<()>().await;
            };

            tokio::select! {
                _ = ctrl_c => tracing::info!("Received SIGINT (Ctrl+C)"),
                _ = terminate => tracing::info!("Received SIGTERM"),
            }

            tracing::info!("Shutdown signal received, starting graceful shutdown...");

            // ── Signal the server to drain, bounded to 25 seconds ──────────
            let _ = shutdown_tx.send(());
            match tokio::time::timeout(std::time::Duration::from_secs(25), server_handle).await {
                Ok(Ok(Ok(()))) => tracing::info!("Server finished normally"),
                Ok(Ok(Err(e))) => tracing::error!("Server error: {e}"),
                Ok(Err(e)) => tracing::error!("Server task panicked: {e}"),
                Err(_) => tracing::warn!("Drain timed out after 25s, proceeding with cleanup"),
            }

            // ── Graceful shutdown sequence ──────────────────────────────────
            tracing::info!("Stopping background tasks...");
            state.shutdown_token.cancel();

            // Close WebSocket connections cleanly.
            if let Some(ref mgr) = state.notification_manager {
                mgr.shutdown().await;
            }

            // Commit Tantivy indexer.
            if let Some(ref indexer) = state.indexer
                && let Err(e) = indexer.commit()
            {
                tracing::error!("Failed to commit indexer during shutdown: {e}");
            }

            // DB connection is dropped when `state` goes out of scope;
            // the OS handles final file descriptor cleanup.

            tracing::info!("Server shutdown complete");

            Ok(())
        }
        Command::Adduser {
            email,
            password,
            regular,
        } => {
            use infra::entity::user;

            let email = match email {
                Some(e) => e,
                None => {
                    print!("email: ");
                    std::io::stdout().flush()?;
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;
                    input.trim().to_owned()
                }
            };

            let password = match password {
                Some(p) => p,
                None => rpassword::prompt_password("password: ")?,
            };

            let exists = user::Entity::find()
                .filter(user::Column::Email.eq(&email))
                .one(&db)
                .await?;

            if exists.is_some() {
                anyhow::bail!("user '{}' already exists", email);
            }

            let password_hash = server::service::auth::password::hash_password(
                &password,
                config.auth.password_hash_iterations,
            );
            let now = chrono::Utc::now().timestamp();

            let is_admin = !regular;
            let model = user::ActiveModel {
                id: sea_orm::NotSet,
                email: Set(email.clone()),
                password_hash: Set(password_hash),
                is_active: Set(true),
                is_admin: Set(is_admin),
                created_at: Set(now),
                last_login_at: Set(None),
                invited_by: Set(None),
                storage_quota: sea_orm::NotSet,
                name: sea_orm::NotSet,
                display_name: sea_orm::NotSet,
                language: sea_orm::NotSet,
            };

            model.insert(&db).await?;
            println!("user '{}' created successfully", email);
            Ok(())
        }
    }
}
