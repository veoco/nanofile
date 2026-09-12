use std::collections::HashMap;

use sea_orm::Statement;
use sea_orm_migration::prelude::*;

/// Move the legacy per-library WebDAV keys into the unified API-key tables and
/// drop the old table.
///
/// One-way move: every `webdav_keys` row becomes an `api_keys` row whose
/// capabilities are exactly `webdav.read` (plus `webdav.write` for an `rw` key)
/// and one `api_key_repos` binding, so a migrated key keeps precisely the access
/// it had — including the fact that it never worked on `/api2`.
///
/// The secret is not re-hashed: `webdav_keys.key_hash` is already
/// `hex(sha256(raw))`, the same digest the unified store compares against, so
/// migrated keys keep working with the value their client already holds.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let backend = db.get_database_backend();

        if !manager.has_table("webdav_keys").await? {
            // A database that never had the legacy table (a fresh install runs
            // the creating migration first, so this only happens if the table
            // was already dropped).
            return Ok(());
        }

        // A previous failed run of this migration may have left partial rows
        // in api_keys / api_key_repos (each INSERT auto-commits in SQLite, so
        // a mid-loop failure does not roll them back). Re-running would then
        // hit api_keys.key_hash's unique constraint on the first row already
        // copied. Remove any migrated rows — key_prefix IS NULL is the marker
        // for a migrated key, since API-created keys always carry a prefix —
        // before starting the copy. api_key_repos is cleared first as the
        // child side of the FK.
        db.execute_unprepared("DELETE FROM api_key_repos").await?;
        db.execute_unprepared("DELETE FROM api_keys WHERE key_prefix IS NULL").await?;

        // SQLite does not enforce FKs by default, so past deletes of users or
        // repos may have left dangling webdav_keys rows whose user_id or
        // repo_id points at a record that no longer exists. Filtering at
        // SELECT time (rather than DELETE) is robust against transaction
        // rollbacks: even if a previous run's orphan-cleanup DELETE was
        // rolled back, this query only returns rows whose FK targets exist,
        // so the INSERTs below cannot trip the new tables' FK constraints.
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

        // The old schema's unique index was on (repo_id, user_id, key_hash),
        // so the same digest could appear on multiple rows — one per library.
        // The unified api_keys.key_hash is globally unique, so the first
        // occurrence of a digest creates the key and later ones add another
        // api_key_repos binding to the same key.
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
                // Same secret already migrated from another library: the
                // unified key is globally unique on key_hash, so widen the
                // capability set if this row grants broader access (rw
                // supersedes r) rather than inserting a duplicate row.
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

                let id: i32 = db
                    .query_one_raw(Statement::from_string(
                        backend,
                        "SELECT last_insert_rowid() AS id".to_string(),
                    ))
                    .await?
                    .ok_or_else(|| DbErr::Custom("api_keys insert returned no rowid".into()))?
                    .try_get("", "id")
                    .map_err(|e| DbErr::Custom(e.to_string()))?;

                key_id_by_hash.insert(key_hash, id);
                id
            };

            // The binding carries the same ceiling as the legacy key, so a
            // read-only key stays read-only even if its capabilities are later
            // widened. INSERT OR IGNORE covers the rare case of the same
            // (key_id, repo_id) pair arising from two users sharing a digest.
            db.execute_raw(Statement::from_sql_and_values(
                backend,
                "INSERT OR IGNORE INTO api_key_repos (key_id, repo_id, permission) \
                 VALUES (?, ?, ?)",
                [key_id.into(), repo_id.into(), permission.into()],
            ))
            .await?;

            migrated += 1;
        }

        // Verify the copy before destroying the source. A shortfall aborts the
        // migration, leaving the legacy table (and the data) in place.
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
        // The move is one-way: recreate the empty table so the schema matches
        // this migration's predecessor, but do not attempt to restore rows (the
        // unified rows are left in place and would otherwise be lost).
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
