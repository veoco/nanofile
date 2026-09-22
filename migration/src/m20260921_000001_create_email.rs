use sea_orm_migration::prelude::*;

/// Email delivery: one settings row and the outbox/log of every message.
///
/// `email_settings` is a single row (`id = 1`). The master switch stays in
/// `config.toml` (`[email] enabled`) on purpose: enabling outbound mail is an
/// operational opt-in, while everything else about the SMTP connection is meant
/// to be editable at runtime from `/sysadmin/email/` — which requires a table,
/// because `AppState.config` is immutable.
///
/// `email_messages` is both the queue and the audit log. The rendered body is
/// stored **encrypted** (`body_enc`, the same domain-separated AEAD cipher as
/// repository sync tokens) and cleared once the message is delivered, so a
/// password-reset link is never readable at rest — the plaintext that used to
/// be dropped on the floor is now only recoverable by the recipient's mailbox.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EmailSettings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmailSettings::Id)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::Paused)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::Host)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::Port)
                            .integer()
                            .not_null()
                            .default(587),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::Tls)
                            .text()
                            .not_null()
                            .default("starttls"),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::Username)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(EmailSettings::PasswordEnc).text())
                    .col(
                        ColumnDef::new(EmailSettings::FromAddress)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::FromName)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::TimeoutSecs)
                            .integer()
                            .not_null()
                            .default(10),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::MaxAttempts)
                            .integer()
                            .not_null()
                            .default(5),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::NotifyNewDevice)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::NotifyApiKeyCreated)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::NotifyNewLogin)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(EmailSettings::UpdatedAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(EmailSettings::UpdatedBy).integer())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(EmailMessages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmailMessages::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EmailMessages::Kind).text().not_null())
                    .col(ColumnDef::new(EmailMessages::ToAddress).text().not_null())
                    .col(ColumnDef::new(EmailMessages::UserId).integer())
                    .col(ColumnDef::new(EmailMessages::Subject).text().not_null())
                    .col(ColumnDef::new(EmailMessages::BodyEnc).text())
                    .col(
                        ColumnDef::new(EmailMessages::Status)
                            .text()
                            .not_null()
                            .default("pending"),
                    )
                    .col(
                        ColumnDef::new(EmailMessages::Attempts)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(EmailMessages::NextAttemptAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(EmailMessages::LastError).text())
                    .col(
                        ColumnDef::new(EmailMessages::CreatedAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(EmailMessages::SentAt).big_integer())
                    .to_owned(),
            )
            .await?;

        // The drainer's only query is "due pending rows", so the index has to
        // lead with both columns in that order for it to be usable.
        manager
            .create_index(
                Index::create()
                    .name("idx_email_messages_status_next_attempt")
                    .table(EmailMessages::Table)
                    .col(EmailMessages::Status)
                    .col(EmailMessages::NextAttemptAt)
                    .to_owned(),
            )
            .await?;

        // Retention pruning and the admin page's "newest first" listing.
        manager
            .create_index(
                Index::create()
                    .name("idx_email_messages_created_at")
                    .table(EmailMessages::Table)
                    .col(EmailMessages::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EmailMessages::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(EmailSettings::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum EmailSettings {
    Table,
    Id,
    Paused,
    Host,
    Port,
    Tls,
    Username,
    PasswordEnc,
    FromAddress,
    FromName,
    TimeoutSecs,
    MaxAttempts,
    NotifyNewDevice,
    NotifyApiKeyCreated,
    NotifyNewLogin,
    UpdatedAt,
    UpdatedBy,
}

#[derive(Iden)]
enum EmailMessages {
    Table,
    Id,
    Kind,
    ToAddress,
    UserId,
    Subject,
    BodyEnc,
    Status,
    Attempts,
    NextAttemptAt,
    LastError,
    CreatedAt,
    SentAt,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// The number of migrations that run before this one.
    ///
    /// Looked up by name rather than assumed to be "the last one": a later
    /// migration that removes `email_settings` (which this migration's table
    /// has since been folded into) would otherwise silently become the subject
    /// of these tests.
    fn steps_before_this_migration() -> u32 {
        crate::migration_names()
            .iter()
            .position(|name| *name == "m20260921_000001_create_email")
            .expect("this migration is registered") as u32
    }

    /// A database migrated up to — but not including — this migration, so the
    /// two tables really are created by the code under test.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");
        crate::Migrator::up(&db, Some(steps_before_this_migration()))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    /// Stop right after this migration, before any later one touches the tables.
    async fn db_holding_this_migrations_tables() -> (tempfile::TempDir, DatabaseConnection) {
        let (dir, db) = before_this_migration().await;
        crate::Migrator::up(&db, Some(1)).await.expect("run migration");
        (dir, db)
    }

    async fn count(db: &DatabaseConnection, sql: &str) -> i64 {
        db.query_one_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            sql,
        ))
        .await
        .expect("count")
        .expect("a row")
        .try_get::<i64>("", "n")
        .expect("n")
    }

    #[tokio::test]
    async fn creates_both_tables_with_delivery_defaults() {
        let (_dir, db) = before_this_migration().await;
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM sqlite_master
                  WHERE type = 'table' AND name = 'email_settings'"
            )
            .await,
            0,
            "the table must not exist before this migration"
        );

        crate::Migrator::up(&db, Some(1)).await.expect("run migration");

        // A row inserted with nothing but the delivery defaults must be
        // *deliverable-shaped*: every notification on, not paused, sensible
        // SMTP defaults. A column added later without a default would fail here.
        db.execute_unprepared("INSERT INTO email_settings (id) VALUES (1)")
            .await
            .expect("insert settings");

        let row = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT paused, host, port, tls, from_address, timeout_secs, max_attempts,
                        notify_new_device, notify_api_key_created, notify_new_login
                   FROM email_settings WHERE id = 1",
            ))
            .await
            .expect("query")
            .expect("a row");

        assert!(!row.try_get::<bool>("", "paused").expect("paused"));
        assert_eq!(row.try_get::<String>("", "host").expect("host"), "");
        assert_eq!(row.try_get::<i32>("", "port").expect("port"), 587);
        assert_eq!(row.try_get::<String>("", "tls").expect("tls"), "starttls");
        assert_eq!(row.try_get::<i32>("", "timeout_secs").expect("timeout"), 10);
        assert_eq!(row.try_get::<i32>("", "max_attempts").expect("attempts"), 5);
        for column in [
            "notify_new_device",
            "notify_api_key_created",
            "notify_new_login",
        ] {
            assert!(
                row.try_get::<bool>("", column).expect(column),
                "{column} must default to on"
            );
        }
    }

    #[tokio::test]
    async fn queues_a_message_and_clears_its_body_in_place() {
        let (_dir, db) = db_holding_this_migrations_tables().await;

        db.execute_unprepared(
            "INSERT INTO email_messages (kind, to_address, subject, body_enc, status, created_at)
             VALUES ('new_login', 'user@example.com', 'New sign-in', 'ciphertext', 'pending', 7)",
        )
        .await
        .expect("insert message");

        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM email_messages WHERE status = 'pending'"
            )
            .await,
            1
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM email_messages WHERE attempts = 0 AND next_attempt_at = 0"
            )
            .await,
            1,
            "a queued row is immediately due and unattempted"
        );

        // Delivery clears the ciphertext: the delivered body must not survive
        // in the database, which is what keeps a reset link unreadable at rest.
        db.execute_unprepared(
            "UPDATE email_messages SET body_enc = NULL, status = 'sent', sent_at = 9 WHERE id = 1",
        )
        .await
        .expect("mark sent");
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM email_messages WHERE body_enc IS NULL AND sent_at = 9"
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn down_drops_both_tables() {
        // Stop right after this migration, then roll exactly it back: running
        // the whole chain and stepping back once would undo a *later* migration.
        let (_dir, db) = db_holding_this_migrations_tables().await;

        crate::Migrator::down(&db, Some(1))
            .await
            .expect("run this migration's down");

        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS n FROM sqlite_master
                  WHERE type = 'table' AND name IN ('email_settings', 'email_messages')"
            )
            .await,
            0
        );
    }
}
