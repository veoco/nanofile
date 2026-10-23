use sea_orm_migration::prelude::*;

/// Derive each library's modification time from its head commit.
///
/// Before this release, content changes only moved `repos.head_commit_id`; `updated_at` was
/// written by creation, rename, description and password changes, so a library's list time
/// stayed at its creation time after uploads. The runtime now stamps it with the new head
/// commit's `ctime` on every branch update, matching Seafile's `RepoInfo.update_time`.
///
/// Rows from earlier releases still carry the stale value, so backfill them once. The
/// update only moves the value forward: a rename or description change recorded after the
/// last commit is left alone. Libraries without a head (never uploaded) and heads whose
/// commit row is missing keep whatever they have.
///
/// `down()` is a no-op: the previous values were wrong, are not recoverable, and nothing
/// depends on them.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE repos
                    SET updated_at = (
                        SELECT c.ctime
                          FROM commits c
                         WHERE c.repo_id = repos.id
                           AND c.commit_id = repos.head_commit_id
                    )
                  WHERE repos.head_commit_id IS NOT NULL
                    AND EXISTS (
                        SELECT 1
                          FROM commits c
                         WHERE c.repo_id = repos.id
                           AND c.commit_id = repos.head_commit_id
                           AND c.ctime > repos.updated_at
                    )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Irreversible on purpose: see the type-level documentation.
        let _ = manager;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    const C1: &str = "1111111111111111111111111111111111111111";
    const C2: &str = "2222222222222222222222222222222222222222";
    const C3: &str = "3333333333333333333333333333333333333333";
    const C4: &str = "4444444444444444444444444444444444444444";
    const C9: &str = "9999999999999999999999999999999999999999";
    const ROOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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
            .position(|name| *name == "m20261023_000001_backfill_repo_updated_at")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn exec(db: &DatabaseConnection, sql: &str) {
        db.execute_unprepared(sql).await.expect("seed/exec");
    }

    /// One library per case: stale, renamed after its last commit, empty, missing head
    /// commit, commit time equal to the stored value, and a password rotation recorded
    /// before a later upload.
    async fn seed(db: &DatabaseConnection) {
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, created_at)
             VALUES (1, 'owner@example.com', 'h', 0)",
        )
        .await;
        exec(
            db,
            &format!(
                "INSERT INTO repos (id, name, owner_id, created_at, updated_at, head_commit_id)
                 VALUES ('repo-stale', 'stale', 1, 100, 100, '{C1}'),
                        ('repo-renamed', 'renamed', 1, 100, 300, '{C2}'),
                        ('repo-empty', 'empty', 1, 100, 100, NULL),
                        ('repo-missing-head', 'missing', 1, 100, 100, '{C9}'),
                        ('repo-equal', 'equal', 1, 100, 200, '{C3}'),
                        ('repo-rotated', 'rotated', 1, 100, 150, '{C4}')"
            ),
        )
        .await;
        exec(
            db,
            &format!(
                "INSERT INTO commits (repo_id, commit_id, root_id, creator_name, ctime)
                 VALUES ('repo-stale', '{C1}', '{ROOT}', 'owner@example.com', 200),
                        ('repo-renamed', '{C2}', '{ROOT}', 'owner@example.com', 200),
                        ('repo-equal', '{C3}', '{ROOT}', 'owner@example.com', 200),
                        ('repo-rotated', '{C4}', '{ROOT}', 'owner@example.com', 250)"
            ),
        )
        .await;
    }

    async fn updated_at(db: &DatabaseConnection, repo_id: &str) -> i64 {
        db.query_one_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            format!("SELECT updated_at FROM repos WHERE id = '{repo_id}'"),
        ))
        .await
        .expect("select updated_at")
        .expect("a repo row")
        .try_get::<i64>("", "updated_at")
        .expect("updated_at")
    }

    #[tokio::test]
    async fn backfills_only_when_the_head_commit_is_newer() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        assert_eq!(
            updated_at(&db, "repo-stale").await,
            200,
            "a never-renamed library takes its head commit's time"
        );
        assert_eq!(
            updated_at(&db, "repo-renamed").await,
            300,
            "a rename recorded after the last commit must win"
        );
        assert_eq!(
            updated_at(&db, "repo-empty").await,
            100,
            "a library without a head is left alone"
        );
        assert_eq!(
            updated_at(&db, "repo-missing-head").await,
            100,
            "a missing head commit row is left alone"
        );
        assert_eq!(
            updated_at(&db, "repo-equal").await,
            200,
            "an equal timestamp is not rewritten"
        );
        assert_eq!(
            updated_at(&db, "repo-rotated").await,
            250,
            "a password rotation recorded before a later upload is corrected"
        );
    }
}
