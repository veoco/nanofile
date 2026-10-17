use std::collections::HashMap;

use sea_orm::Statement;
use sea_orm_migration::prelude::*;

/// Move the legacy per-library WebDAV keys into the unified API-key tables and drop
/// the old table.
///
/// Every `webdav_keys` row becomes an `api_keys` row with exactly `webdav.read` (plus
/// `webdav.write` for `rw`) and one binding, so access is preserved — including that it
/// never worked on `/api2`. The digest is copied, not re-hashed: it is already
/// `hex(sha256(raw))`, the form the unified store compares.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let backend = db.get_database_backend();

        if !manager.has_table("webdav_keys").await? {
            // A database that never had the legacy table (already dropped) has nothing to do.
            return Ok(());
        }

        // A previous failed run may have left partial rows: each INSERT auto-commits in SQLite,
        // so a mid-loop failure does not roll back, and re-running would hit the unique key_hash
        // constraint. Remove migrated rows (`key_prefix IS NULL` is the marker; API-created keys
        // always have a prefix) and the bindings first.
        db.execute_unprepared("DELETE FROM api_key_repos").await?;
        db.execute_unprepared("DELETE FROM api_keys WHERE key_prefix IS NULL").await?;

        // SQLite does not enforce FKs, so past deletes may have left dangling rows. Filter at
        // SELECT time rather than DELETE: even after a rolled-back orphan cleanup, this returns
        // only rows whose FK targets exist, so the inserts cannot trip the new constraints.
        let legacy = db
            .query_all_raw(Statement::from_string(
                backend,
                "SELECT w.id, w.repo_id, w.user_id, w.name, w.permission, w.key_hash, w.created_at, w.last_used_at \
                 FROM webdav_keys w \
                 WHERE EXISTS (SELECT 1 FROM users WHERE users.id = w.user_id) \
                    AND EXISTS (SELECT 1 FROM repos WHERE repos.id = w.repo_id)"
                    .to_string(),
            ))
            .await?;

        // The old unique index was on (repo_id, user_id, key_hash), so one digest could appear
        // once per library. `api_keys.key_hash` is globally unique, so the first occurrence
        // creates the key and later ones add a binding.
        let mut key_id_by_hash: HashMap<String, i32> = HashMap::new();

        let mut migrated = 0usize;
        for row in &legacy {
            let get = |column: &str| {
                row.try_get::<String>("", column)
                    .map_err(|e| DbErr::Custom(e.to_string()))
            };
            let repo_id = get("repo_id")?;
            let name = get("name")?;
            let permission = get("permission")?;
            let key_hash = get("key_hash")?;
            let user_id: i32 = row
                .try_get("", "user_id")
                .map_err(|e| DbErr::Custom(e.to_string()))?;
            let created_at: i64 = row
                .try_get("", "created_at")
                .map_err(|e| DbErr::Custom(e.to_string()))?;
            let last_used_at: Option<i64> = row
                .try_get("", "last_used_at")
                .map_err(|e| DbErr::Custom(e.to_string()))?;

            let capabilities = if permission == "r" {
                "webdav.read"
            } else {
                "webdav.read,webdav.write"
            };

            let key_id = if let Some(&id) = key_id_by_hash.get(&key_hash) {
                // Same secret already migrated from another library: widen the capability set if this
                // row grants more (rw supersedes r) rather than inserting a duplicate.
                if permission != "r" {
                    db.execute_raw(Statement::from_sql_and_values(
                        backend,
                        "UPDATE api_keys SET capabilities = 'webdav.read,webdav.write' \
                         WHERE id = ? AND capabilities = 'webdav.read'",
                        [id.into()],
                    ))
                    .await?;
                }
                id
            } else {
                db.execute_raw(Statement::from_sql_and_values(
                    backend,
                    "INSERT INTO api_keys \
                       (user_id, name, key_hash, key_prefix, capabilities, all_repos, created_at, expires_at, last_used_at) \
                     VALUES (?, ?, ?, NULL, ?, 0, ?, NULL, ?)",
                    [
                        user_id.into(),
                        name.into(),
                        key_hash.clone().into(),
                        capabilities.into(),
                        created_at.into(),
                        last_used_at.into(),
                    ],
                ))
                .await?;

                // Look up by key_hash, not last_insert_rowid(): the migration connection is a pool, so
                // the insert and the lookup may land on different connections. key_hash is globally
                // unique, so the SELECT always finds the row.
                let id: i32 = db
                    .query_one_raw(Statement::from_sql_and_values(
                        backend,
                        "SELECT id FROM api_keys WHERE key_hash = ?",
                        [key_hash.clone().into()],
                    ))
                    .await?
                    .ok_or_else(|| {
                        DbErr::Custom(format!(
                            "api_keys insert for key_hash {key_hash} could not be read back"
                        ))
                    })?
                    .try_get("", "id")
                    .map_err(|e| DbErr::Custom(e.to_string()))?;

                key_id_by_hash.insert(key_hash, id);
                id
            };

            // The binding carries the same ceiling, so a read-only key stays read-only. INSERT OR
            // IGNORE covers the same (key_id, repo_id) arising from two users sharing a digest.
            db.execute_raw(Statement::from_sql_and_values(
                backend,
                "INSERT OR IGNORE INTO api_key_repos (key_id, repo_id, permission) \
                 VALUES (?, ?, ?)",
                [key_id.into(), repo_id.into(), permission.into()],
            ))
            .await?;

            migrated += 1;
        }

        // Verify the copy before destroying the source; a shortfall aborts the migration.
        if migrated != legacy.len() {
            return Err(DbErr::Custom(format!(
                "migrated {migrated} of {} webdav keys",
                legacy.len()
            )));
        }

        db.execute_unprepared("DROP TABLE webdav_keys").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One-way: recreate the empty table to match the predecessor's schema, but do not
        // restore rows (the unified rows are left in place).
        manager
            .create_table(
                Table::create()
                    .table(WebdavKeys::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WebdavKeys::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WebdavKeys::RepoId).string_len(36).not_null())
                    .col(ColumnDef::new(WebdavKeys::UserId).integer().not_null())
                    .col(ColumnDef::new(WebdavKeys::Name).text().not_null())
                    .col(ColumnDef::new(WebdavKeys::Permission).string_len(2).not_null())
                    .col(ColumnDef::new(WebdavKeys::KeyHash).string_len(64).not_null())
                    .col(ColumnDef::new(WebdavKeys::CreatedAt).big_integer().not_null())
                    .col(ColumnDef::new(WebdavKeys::LastUsedAt).big_integer())
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum WebdavKeys {
    Table,
    Id,
    RepoId,
    UserId,
    Name,
    Permission,
    KeyHash,
    CreatedAt,
    LastUsedAt,
}
