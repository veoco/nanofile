use sea_orm_migration::prelude::*;

/// Make WebDAV keys addressable by hash.
///
/// `webdav_keys` was created with two *non-unique* indexes and the lookup is
/// `.one()` on `(repo_id, user_id, key_hash)`. Duplicate rows are therefore
/// permitted, and with duplicates `.one()` returns an arbitrary row while
/// `delete_by_id` removes only one — so a key the user deleted could keep
/// authenticating. Keys are 160-bit random values, so a collision is not
/// reachable; this is about the schema enforcing the invariant the code assumes.
///
/// Existing duplicates (only possible through a restore plus a regenerate, never
/// through the API) are collapsed first: the oldest row per group is kept.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Collapse duplicates before adding the constraint, keeping the earliest
        // row of each (repo_id, user_id, key_hash) group.
        db.execute_unprepared(
            "DELETE FROM webdav_keys WHERE id NOT IN (
                 SELECT MIN(id) FROM webdav_keys GROUP BY repo_id, user_id, key_hash
             )",
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("uidx_webdav_keys_repo_user_hash")
                    .table(WebdavKeys::Table)
                    .col(WebdavKeys::RepoId)
                    .col(WebdavKeys::UserId)
                    .col(WebdavKeys::KeyHash)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("uidx_webdav_keys_repo_user_hash")
                    .table(WebdavKeys::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum WebdavKeys {
    Table,
    RepoId,
    UserId,
    KeyHash,
}
