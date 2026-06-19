use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use sea_orm_migration::MigratorTrait;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::oneshot;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::timeout::TimeoutLayer;
use tracing_subscriber::EnvFilter;

use nanofile::AppState;
use nanofile::config::Config;
use nanofile::db::establish_connection;

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

    let config = Config::load()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .parse_lossy(&config.logging.level),
        )
        .init();

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
                let count = nanofile::entity::user::Entity::find()
                    .count(state.db.as_ref())
                    .await?;
                if count == 0 {
                    tracing::info!("No users found; creating initial admin user");
                    let password_hash = nanofile::auth::password::hash_password(
                        admin_password,
                        state.config.auth.password_hash_iterations,
                    );
                    let now = chrono::Utc::now().timestamp();
                    let model = nanofile::entity::user::ActiveModel {
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
                let cors_origins = &state.config.server.cors_allowed_origins;
                let origins: Vec<_> = cors_origins.iter().filter(|o| o.as_str() != "*").collect();

                let origin_layer = if origins.is_empty() {
                    // "*" or empty → allow all origins
                    CorsLayer::new().allow_origin(Any)
                } else {
                    use tower_http::cors::AllowOrigin;
                    CorsLayer::new().allow_origin(AllowOrigin::list(
                        origins
                            .into_iter()
                            .map(|o| o.parse().expect("invalid CORS origin")),
                    ))
                };

                origin_layer.allow_methods(Any).allow_headers(Any).max_age(
                    std::time::Duration::from_secs(state.config.server.cors_max_age_secs),
                )
            };

            let api_routes = nanofile::api::api_routes();
            let sync_routes = nanofile::sync::sync_routes();
            let api_v21_routes = nanofile::api_v21::api_v21_routes();
            let api_v1_routes = nanofile::api_v1::api_v1_routes();
            let web_routes = nanofile::web::web_routes();
            let ui_routes = nanofile::ui::ui_routes();
            let notification_routes = nanofile::notification::notification_routes();

            let app = Router::new()
                .route("/health", get(health_check))
                .merge(api_routes)
                .merge(sync_routes)
                .merge(api_v21_routes)
                .merge(api_v1_routes)
                .merge(web_routes)
                .merge(ui_routes)
                .merge(notification_routes)
                .merge(nanofile::api::avatar::image_routes())
                .route(
                    "/static/{*path}",
                    get(nanofile::static_assets::serve_static),
                )
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
            use nanofile::entity::user;

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

            let password_hash = nanofile::auth::password::hash_password_legacy(&password);
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
