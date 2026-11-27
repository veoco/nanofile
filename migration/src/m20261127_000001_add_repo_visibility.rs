use sea_orm_migration::prelude::*;

/// Add the administrator-managed sharing visibility of a library.
///
/// `private` (default): only the owner can see it.
/// `shared`: the owner has listed individual accounts in `repo_members`.
/// `public`: every active account can read the library, without a member row.
///
/// Existing libraries are all private, so the upgrade changes no behaviour.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .add_column(
                        ColumnDef::new(Repos::Visibility)
                            .string_len(10)
                            .not_null()
                            .default("private"),
                    )
                    .to_owned(),
            )
            .await?;

        // The `visibility = 'public'` lookup runs on every library listing, so
        // it gets an index even though the table is small.
        manager
            .create_index(
                Index::create()
                    .name("idx_repos_visibility")
                    .table(Repos::Table)
                    .col(Repos::Visibility)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Downgrading must not turn member rows into grants an older build
        // honours while the visibility column that scoped them is gone.
        manager
            .get_connection()
            .execute_unprepared(
                "DELETE FROM repo_members
                  WHERE NOT EXISTS (
                      SELECT 1 FROM repos r
                       WHERE r.id = repo_members.repo_id
                         AND r.owner_id = repo_members.user_id
                  )",
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_repos_visibility")
                    .table(Repos::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .drop_column(Repos::Visibility)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Repos {
    Table,
    Visibility,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::tests_support::connect(&crate::tests_support::file_uri(
            dir.path(),
            "test.db",
        ))
        .await;
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20261127_000001_add_repo_visibility")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn exec(db: &DatabaseConnection, sql: &str) {
        db.execute_unprepared(sql).await.expect("exec");
    }

    #[tokio::test]
    async fn existing_libraries_become_private() {
        let (_dir, db) = before_this_migration().await;
        exec(
            &db,
            "INSERT INTO users (id, email, password_hash, created_at)
             VALUES (1, 'owner@example.com', 'h', 0)",
        )
        .await;
        exec(
            &db,
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version,
                                salt, permission, created_at, updated_at)
             VALUES ('r1', 'old', '', 1, 0, 0, '', 'rw', 0, 0)",
        )
        .await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        let visibility: String = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT visibility FROM repos WHERE id = 'r1'".to_string(),
            ))
            .await
            .expect("query")
            .expect("a row")
            .try_get("", "visibility")
            .expect("visibility");
        assert_eq!(visibility, "private");
    }

    #[tokio::test]
    async fn down_revokes_non_owner_memberships() {
        let (_dir, db) = before_this_migration().await;
        exec(
            &db,
            "INSERT INTO users (id, email, password_hash, created_at)
             VALUES (1, 'owner@example.com', 'h', 0), (2, 'member@example.com', 'h', 0)",
        )
        .await;
        exec(
            &db,
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version,
                                salt, permission, created_at, updated_at)
             VALUES ('r1', 'shared', '', 1, 0, 0, '', 'rw', 0, 0)",
        )
        .await;
        exec(
            &db,
            "INSERT INTO repo_members (repo_id, user_id, permission, created_at)
             VALUES ('r1', 1, 'rw', 0), ('r1', 2, 'r', 0)",
        )
        .await;

        crate::Migrator::up(&db, None).await.expect("run migration");
        // Apply exactly this migration's down, not the next one's.
        crate::Migrator::down(&db, Some(1)).await.expect("roll back");

        let count: i64 = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM repo_members".to_string(),
            ))
            .await
            .expect("query")
            .expect("a row")
            .try_get("", "n")
            .expect("count");
        assert_eq!(count, 1, "only the owner membership survives a downgrade");
    }
}
