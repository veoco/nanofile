use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use rand::Rng;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use sea_orm_migration::MigratorTrait;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::oneshot;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::timeout::TimeoutLayer;
use tracing_subscriber::EnvFilter;

use infra::config::Config;
use infra::db::establish_connection;
use server::AppState;

/// Nanofile — a Seafile-compatible sync server
#[derive(Parser)]
#[command(name = "nanofile", version, about)]
struct Cli {
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut config = Config::load()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .parse_lossy(&config.logging.level),
        )
        .init();

    // Auto-generate notification JWT secret if empty or still the well-known default.
    if config.notification.private_key.is_empty()
        || config.notification.private_key == "nanofile-notification-secret"
    {
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        config.notification.private_key = hex::encode(key);
        tracing::warn!(
            "Auto-generated notification secret key. \
             Set NANOFILE_NOTIFICATION_PRIVATE_KEY to persist across restarts."
        );
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

            let state = Arc::new(AppState::new(db, config.clone()));

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
                    let password_hash = server::auth::password::hash_password(
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
                        name: sea_orm::NotSet,
                        display_name: sea_orm::NotSet,
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
                let origins = state.config.server.cors_origins();

                let origin_layer = if origins.is_empty() {
                    // Empty list — deny all cross-origin requests.
                    CorsLayer::new().allow_origin(AllowOrigin::predicate(|_, _| false))
                } else {
                    CorsLayer::new().allow_origin(AllowOrigin::list(
                        origins.into_iter().filter_map(|o| {
                            o.parse()
                                .map_err(|e| {
                                    tracing::warn!("Skipping invalid CORS origin '{}': {:?}", o, e)
                                })
                                .ok()
                        }),
                    ))
                };

                origin_layer
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

            let sync_routes = server::sync::sync_routes();
            let sdoc_routes = server::sdoc::sdoc_routes();
            let web_routes = server::web::web_routes();
            let ui_routes = server::ui::ui_routes();
            let notification_routes = server::notification::notification_routes();

            let app = Router::new()
                .route("/health", get(health_check))
                .merge(server::routes::api_routes())
                .merge(sync_routes)
                .merge(sdoc_routes)
                .merge(web_routes)
                .merge(ui_routes)
                .merge(notification_routes)
                .merge(server::user::handler::avatar::image_routes())
                .route("/static/{*path}", get(server::static_assets::serve_static))
                .layer(NormalizePathLayer::trim_trailing_slash())
                .layer(cors)
                .layer(DefaultBodyLimit::max(
                    (config.server.max_upload_size_mb * 1024 * 1024) as usize,
                ))
                .layer(RequestBodyLimitLayer::new(
                    (config.server.max_upload_size_mb * 1024 * 1024) as usize,
                ))
                .layer(tower_http::trace::TraceLayer::new_for_http())
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    std::time::Duration::from_secs(config.server.request_timeout_secs),
                ))
                .with_state(state.clone());

            let addr = format!("{}:{}", config.server.addr, config.server.port);
            tracing::info!("listening on {}", addr);

            let listener = tokio::net::TcpListener::bind(&addr).await?;

            // ── Start server with graceful shutdown via oneshot ─────────────
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

            let serve_fut = axum::serve(listener, app).with_graceful_shutdown(async {
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

            let password_hash = server::auth::password::hash_password(
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
                name: sea_orm::NotSet,
                display_name: sea_orm::NotSet,
            };

            model.insert(&db).await?;
            println!("user '{}' created successfully", email);
            Ok(())
        }
    }
}
