use sea_orm_migration::prelude::*;

/// Add the `pwd_hash` password verifier of a Seafile 11+ encrypted library.
///
/// Upstream keeps `pwd_hash`/`pwd_hash_algo`/`pwd_hash_params` in the commit
/// JSON (and in the in-memory `SeafRepo`), never in its own database. nanofile
/// stores commits as rows and re-synthesises the encryption block from the
/// `repos` row on every read, so the three values belong here — next to the
/// existing `magic`/`random_key`/`salt`.
///
/// All three are nullable: a library created through the legacy `magic` flow
/// (still the default, and the only flow when
/// `server.encrypted_library_pwd_hash_algo` is unset) leaves them NULL.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite doesn't support adding multiple columns in one ALTER TABLE.
        for column in [
            Repos::PwdHash,
            Repos::PwdHashAlgo,
            Repos::PwdHashParams,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Repos::Table)
                        .add_column(ColumnDef::new(column).string().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            Repos::PwdHash,
            Repos::PwdHashAlgo,
            Repos::PwdHashParams,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Repos::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(Iden)]
enum Repos {
    Table,
    PwdHash,
    PwdHashAlgo,
    PwdHashParams,
}
