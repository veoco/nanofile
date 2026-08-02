use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
                    .col(ColumnDef::new(WebdavKeys::Name).string().not_null())
                    .col(
                        ColumnDef::new(WebdavKeys::KeyHash)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WebdavKeys::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WebdavKeys::LastUsedAt).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_webdav_keys_repo_id")
                            .from(WebdavKeys::Table, WebdavKeys::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_webdav_keys_user_id")
                            .from(WebdavKeys::Table, WebdavKeys::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Indexes must be created as separate statements — SeaORM's inlined
        // `Table::create().index(...)` generates SQL that SQLite rejects.
        manager
            .create_index(
                Index::create()
                    .name("idx_webdav_keys_repo_user")
                    .table(WebdavKeys::Table)
                    .col(WebdavKeys::RepoId)
                    .col(WebdavKeys::UserId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_webdav_keys_key_hash")
                    .table(WebdavKeys::Table)
                    .col(WebdavKeys::KeyHash)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WebdavKeys::Table).to_owned())
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
    KeyHash,
    CreatedAt,
    LastUsedAt,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
