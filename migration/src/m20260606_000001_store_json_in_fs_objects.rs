use sea_orm::Statement;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let backend = db.get_database_backend();

        // Query all existing fs_objects to migrate compressed data to JSON
        let rows = db
            .query_all(Statement::from_string(
                backend,
                "SELECT id, data FROM fs_objects".to_string(),
            ))
            .await?;

        for row in &rows {
            let id: i64 = row
                .try_get("", "id")
                .map_err(|e| DbErr::Custom(e.to_string()))?;
            let data: Vec<u8> = row
                .try_get("", "data")
                .map_err(|e| DbErr::Custom(e.to_string()))?;

            // Try to decompress; if it's already plain JSON, skip
            if let Ok(decompressed) = (|| -> Result<Vec<u8>, std::io::Error> {
                let mut decoder = flate2::read::ZlibDecoder::new(&data[..]);
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut decoder, &mut out)?;
                Ok(out)
            })() {
                let json_str = String::from_utf8_lossy(&decompressed);
                db.execute(Statement::from_sql_and_values(
                    backend,
                    "UPDATE fs_objects SET data = ? WHERE id = ?",
                    [json_str.into(), id.into()],
                ))
                .await?;
            }
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let backend = db.get_database_backend();

        // Re-compress all fs_objects back to zlib
        let rows = db
            .query_all(Statement::from_string(
                backend,
                "SELECT id, data FROM fs_objects".to_string(),
            ))
            .await?;

        for row in &rows {
            let id: i64 = row
                .try_get("", "id")
                .map_err(|e| DbErr::Custom(e.to_string()))?;
            let data: String = row
                .try_get("", "data")
                .map_err(|e| DbErr::Custom(e.to_string()))?;

            // Try to compress; skip if already zlib
            use std::io::Write;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            if encoder.write_all(data.as_bytes()).is_ok() {
                if let Ok(compressed) = encoder.finish() {
                    db.execute(Statement::from_sql_and_values(
                        backend,
                        "UPDATE fs_objects SET data = ? WHERE id = ?",
                        [compressed.into(), id.into()],
                    ))
                    .await?;
                }
            }
        }

        Ok(())
    }
}
