use sea_orm_migration::prelude::*;

/// Add a `type` column to `repos` to mark wiki repositories (Seafile wiki2:
/// a wiki is a library whose `type` is `'wiki'`).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite: one column per ALTER TABLE. `type` is a reserved-ish word so
        // the entity maps it with `column_name = "type"`.
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .add_column(
                        ColumnDef::new(Repos::Type)
                            .string_len(20)
                            .not_null()
                            .default("repo"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .drop_column(Repos::Type)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Repos {
    Table,
    Type,
}
