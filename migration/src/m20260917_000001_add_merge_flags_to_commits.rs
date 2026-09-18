use sea_orm_migration::prelude::*;

/// Add the two flags upstream writes into the commit JSON of a commit it
/// created by merging a client's upload with a newer HEAD
/// (`common/commit-mgr.c`: `if (commit->conflict) set "conflict", 1;
/// if (commit->new_merge) set "new_merge", 1`).
///
/// nanofile stores commits as rows and re-synthesises the JSON on every read, so
/// the flags have to be persisted or `GET /repo/{id}/commit/{id}` cannot report
/// a merge commit to the official clients. NULL means "absent", which is exactly
/// the upstream semantics for a normal (non-merge) commit.
///
/// `deleted_repo_commits` (the trash archive) mirrors `commits` column by column
/// — `repo_archive` copies rows with an explicit column list — so it gets the
/// same two columns; otherwise deleting and restoring a library would lose the
/// flags of its merge commits.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite doesn't support adding multiple columns in one ALTER TABLE, so
        // every column is its own `alter_table`.
        add(manager, Commits::Table, Commits::NewMerge).await?;
        add(manager, Commits::Table, Commits::Conflict).await?;
        add(manager, DeletedRepoCommits::Table, DeletedRepoCommits::NewMerge).await?;
        add(manager, DeletedRepoCommits::Table, DeletedRepoCommits::Conflict).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        drop_column(manager, Commits::Table, Commits::NewMerge).await?;
        drop_column(manager, Commits::Table, Commits::Conflict).await?;
        drop_column(manager, DeletedRepoCommits::Table, DeletedRepoCommits::NewMerge).await?;
        drop_column(manager, DeletedRepoCommits::Table, DeletedRepoCommits::Conflict).await?;
        Ok(())
    }
}

async fn add<A, B>(manager: &SchemaManager<'_>, table: A, column: B) -> Result<(), DbErr>
where
    A: IntoIden,
    B: IntoIden,
{
    manager
        .alter_table(
            Table::alter()
                .table(table)
                .add_column(ColumnDef::new(column).boolean().null())
                .to_owned(),
        )
        .await
}

async fn drop_column<A, B>(
    manager: &SchemaManager<'_>,
    table: A,
    column: B,
) -> Result<(), DbErr>
where
    A: IntoIden,
    B: IntoIden,
{
    manager
        .alter_table(
            Table::alter()
                .table(table)
                .drop_column(column)
                .to_owned(),
        )
        .await
}

#[derive(Iden)]
enum Commits {
    Table,
    NewMerge,
    Conflict,
}

#[derive(Iden)]
enum DeletedRepoCommits {
    Table,
    NewMerge,
    Conflict,
}
