use sea_orm_migration::prelude::*;

/// Create the unified API-key tables.
///
/// `api_keys` holds the credential itself (hashed secret, capability set,
/// optional expiry); `api_key_repos` holds the per-library bindings together
/// with each library's read/write ceiling. The legacy `webdav_keys` table is
/// migrated into these by a later migration.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ApiKeys::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ApiKeys::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ApiKeys::UserId).integer().not_null())
                    .col(ColumnDef::new(ApiKeys::Name).text().not_null())
                    .col(ColumnDef::new(ApiKeys::KeyHash).string_len(64).not_null())
                    .col(ColumnDef::new(ApiKeys::KeyPrefix).string_len(16))
                    .col(ColumnDef::new(ApiKeys::Capabilities).text().not_null())
                    .col(
                        ColumnDef::new(ApiKeys::AllRepos)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(ApiKeys::CreatedAt).big_integer().not_null())
                    .col(ColumnDef::new(ApiKeys::ExpiresAt).big_integer())
                    .col(ColumnDef::new(ApiKeys::LastUsedAt).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_keys_user_id")
                            .from(ApiKeys::Table, ApiKeys::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ApiKeyRepos::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ApiKeyRepos::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ApiKeyRepos::KeyId).integer().not_null())
                    .col(
                        ColumnDef::new(ApiKeyRepos::RepoId)
                            .string_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ApiKeyRepos::Permission)
                            .string_len(2)
                            .not_null()
                            .default("rw"),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_key_repos_key_id")
                            .from(ApiKeyRepos::Table, ApiKeyRepos::KeyId)
                            .to(ApiKeys::Table, ApiKeys::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_key_repos_repo_id")
                            .from(ApiKeyRepos::Table, ApiKeyRepos::RepoId)
                            .to(Repos::Table, Repos::Id)
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
                    .name("uidx_api_keys_key_hash")
                    .table(ApiKeys::Table)
                    .col(ApiKeys::KeyHash)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_api_keys_user_id")
                    .table(ApiKeys::Table)
                    .col(ApiKeys::UserId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uidx_api_key_repos_key_repo")
                    .table(ApiKeyRepos::Table)
                    .col(ApiKeyRepos::KeyId)
                    .col(ApiKeyRepos::RepoId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        // Reverse lookup: "which keys may touch this library?" (used when a
        // library is deleted and when a member is removed).
        manager
            .create_index(
                Index::create()
                    .name("idx_api_key_repos_repo")
                    .table(ApiKeyRepos::Table)
                    .col(ApiKeyRepos::RepoId)
                    .col(ApiKeyRepos::KeyId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ApiKeyRepos::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ApiKeys::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum ApiKeys {
    Table,
    Id,
    UserId,
    Name,
    KeyHash,
    KeyPrefix,
    Capabilities,
    AllRepos,
    CreatedAt,
    ExpiresAt,
    LastUsedAt,
}

#[derive(Iden)]
enum ApiKeyRepos {
    Table,
    Id,
    KeyId,
    RepoId,
    Permission,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
