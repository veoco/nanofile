use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use sea_orm_migration::MigratorTrait;
use std::io::Write;
use std::sync::Arc;
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

            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any);

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
                .route("/static/{*path}", get(nanofile::static_assets::serve_static))
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
                .with_state(state);

            let addr = format!("{}:{}", config.server.addr, config.server.port);
            tracing::info!("listening on {}", addr);

            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;

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
            };

            model.insert(&db).await?;
            println!("user '{}' created successfully", email);
            Ok(())
        }
    }
}
