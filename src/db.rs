use sea_orm::{Database, DatabaseConnection};

use crate::config::DatabaseConfig;

pub async fn establish_connection(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    let db = Database::connect(&config.url).await?;
    Ok(db)
}
