use sea_orm_migration::prelude::*;

/// Durable history of background runs.
///
/// The task system keeps runs in memory so an in-place restart preserves them,
/// but memory does not survive a crash. This table records what happened, and
/// holds the input of a run whose job is safe to replay.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(JobRuns::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(JobRuns::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(JobRuns::Kind).string().not_null())
                    .col(ColumnDef::new(JobRuns::Owner).integer().null())
                    .col(ColumnDef::new(JobRuns::Phase).string().not_null())
                    .col(
                        ColumnDef::new(JobRuns::Summary)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(JobRuns::Params).text().null())
                    .col(ColumnDef::new(JobRuns::Error).text().null())
                    .col(ColumnDef::new(JobRuns::Processed).big_integer().null())
                    .col(
                        ColumnDef::new(JobRuns::Attempt)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(JobRuns::CreatedAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(JobRuns::StartedAt).big_integer().null())
                    .col(ColumnDef::new(JobRuns::FinishedAt).big_integer().null())
                    .col(ColumnDef::new(JobRuns::LeaseUntil).big_integer().null())
                    .to_owned(),
            )
            .await?;

        // The admin listing reads recent runs, newest first.
        manager
            .create_index(
                Index::create()
                    .name("idx_job_runs_finished_at")
                    .table(JobRuns::Table)
                    .col(JobRuns::FinishedAt)
                    .to_owned(),
            )
            .await?;
        // Recovery looks for unfinished runs whose lease has run out.
        manager
            .create_index(
                Index::create()
                    .name("idx_job_runs_phase_lease")
                    .table(JobRuns::Table)
                    .col(JobRuns::Phase)
                    .col(JobRuns::LeaseUntil)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(JobRuns::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum JobRuns {
    Table,
    Id,
    Kind,
    Owner,
    Phase,
    Summary,
    Params,
    Error,
    Processed,
    Attempt,
    CreatedAt,
    StartedAt,
    FinishedAt,
    LeaseUntil,
}
