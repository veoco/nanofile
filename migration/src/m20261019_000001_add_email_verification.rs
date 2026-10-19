use sea_orm_migration::prelude::*;

/// Email verification for self-registered accounts.
///
/// Open registration creates the account before the address is proven, so
/// `users.email_verified` is the gate the login paths read. It defaults to
/// `true`: every account that existed before this migration was created by an
/// administrator, an invitation code, the bootstrap flow or `adduser`, and none
/// of them had to prove its address.
///
/// `email_verification_tokens` stores only the SHA-256 of a one-time link, the
/// same rule the password-reset tokens follow. A row is deleted with its user.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::EmailVerified)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(EmailVerificationTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmailVerificationTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(EmailVerificationTokens::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailVerificationTokens::TokenHash)
                            .char_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(EmailVerificationTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailVerificationTokens::ExpiresAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailVerificationTokens::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_email_verification_tokens_user_id")
                            .from(EmailVerificationTokens::Table, EmailVerificationTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(EmailVerificationTokens::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::EmailVerified)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
    EmailVerified,
}

#[derive(Iden)]
enum EmailVerificationTokens {
    Table,
    Id,
    UserId,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    Used,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::tests_support::connect(&crate::tests_support::file_uri(
            dir.path(),
            "test.db",
        ))
        .await;
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20261019_000001_add_email_verification")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn insert_user(db: &DatabaseConnection, email: &str) -> i64 {
        db.execute_unprepared(&format!(
            "INSERT INTO users (email, password_hash, is_active, created_at) \
             VALUES ('{email}', 'hash', 1, 0)"
        ))
        .await
        .expect("insert a pre-migration user");
        db.query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!("SELECT id FROM users WHERE email = '{email}'"),
        ))
        .await
        .expect("select the user")
        .expect("the user exists")
        .try_get::<i64>("", "id")
        .expect("id")
    }

    /// Existing accounts become verified, and a token row follows its user out.
    #[tokio::test]
    async fn existing_users_are_verified_and_tokens_cascade() {
        let (_dir, db) = before_this_migration().await;
        let user_id = insert_user(&db, "old@example.com").await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        let row = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("SELECT email_verified FROM users WHERE id = {user_id}"),
            ))
            .await
            .expect("select email_verified")
            .expect("the user exists");
        assert!(
            row.try_get::<bool>("", "email_verified")
                .expect("email_verified"),
            "a pre-existing account must not be locked out by the migration"
        );

        db.execute_unprepared(&format!(
            "INSERT INTO email_verification_tokens \
             (user_id, token_hash, created_at, expires_at, used) \
             VALUES ({user_id}, '{}', 0, 1, 0)",
            "a".repeat(64)
        ))
        .await
        .expect("insert a token");

        db.execute_unprepared(&format!("DELETE FROM users WHERE id = {user_id}"))
            .await
            .expect("delete the user");

        let remaining = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT id FROM email_verification_tokens",
            ))
            .await
            .expect("select remaining tokens");
        assert!(
            remaining.is_empty(),
            "a deleted user's verification token must go with it"
        );

        let _ = db.close().await;
    }
}
