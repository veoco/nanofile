use sea_orm_migration::prelude::*;

/// Drop the group and contact tables.
///
/// nanofile targets independent per-user use: it never grew a group
/// create/join/leave endpoint, so `groups` and `group_members` were only ever
/// populated out of band by whatever inserted rows for the read-only group
/// listings. `user_contacts` was read by exactly one endpoint,
/// `/api2/groupandcontacts/`, which is removed in the same change. All three
/// tables are gone, and with them the endpoints that serialized them.
///
/// `repo_members` deliberately stays: it is the membership index the permission
/// checks read (`server/src/domain/permission.rs`), and library creation writes
/// the owner's row into it. The migration that follows this one purges every
/// row that was not an owner, which is what actually withdraws the access a
/// previous share granted.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `group_members` carries a foreign key to `groups`, so drop it first.
        manager
            .drop_table(
                Table::drop()
                    .table(GroupMembers::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Groups::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(UserContacts::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Recreate the tables exactly as `m20260603_000001_extend_schema` made
        // them. Informational only: nothing in this build reads or writes them.
        manager
            .create_table(
                Table::create()
                    .table(Groups::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Groups::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Groups::Name).string_len(255).not_null())
                    .col(ColumnDef::new(Groups::CreatorId).integer().not_null())
                    .col(ColumnDef::new(Groups::CreatedAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_groups_creator_id")
                            .from(Groups::Table, Groups::CreatorId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(GroupMembers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GroupMembers::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(GroupMembers::GroupId).integer().not_null())
                    .col(ColumnDef::new(GroupMembers::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(GroupMembers::Role)
                            .char_len(10)
                            .not_null()
                            .default("member"),
                    )
                    .col(
                        ColumnDef::new(GroupMembers::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_group_members_group_id")
                            .from(GroupMembers::Table, GroupMembers::GroupId)
                            .to(Groups::Table, Groups::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_group_members_user_id")
                            .from(GroupMembers::Table, GroupMembers::UserId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_group_members_unique")
                    .table(GroupMembers::Table)
                    .col(GroupMembers::GroupId)
                    .col(GroupMembers::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserContacts::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserContacts::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserContacts::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(UserContacts::ContactEmail)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(ColumnDef::new(UserContacts::ContactName).string_len(255))
                    .col(
                        ColumnDef::new(UserContacts::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_contacts_user_id")
                            .from(UserContacts::Table, UserContacts::UserId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
#[iden = "users"]
struct UsersRef;

#[derive(Iden)]
#[iden = "users"]
#[allow(dead_code)]
enum UsersRefCol {
    Table,
    Id,
}

#[derive(Iden)]
enum Groups {
    Table,
    Id,
    Name,
    CreatorId,
    CreatedAt,
}

#[derive(Iden)]
enum GroupMembers {
    Table,
    Id,
    GroupId,
    UserId,
    Role,
    CreatedAt,
}

#[derive(Iden)]
enum UserContacts {
    Table,
    Id,
    UserId,
    ContactEmail,
    ContactName,
    CreatedAt,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration, so the
    /// three tables still exist and can be seeded.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20260920_000001_drop_groups_and_contacts")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn exec(db: &DatabaseConnection, sql: &str) {
        db.execute_unprepared(sql).await.expect("seed/exec");
    }

    async fn table_exists(db: &DatabaseConnection, name: &str) -> bool {
        let row = db
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'table' AND name = ?",
                [name.into()],
            ))
            .await
            .expect("query sqlite_master")
            .expect("a row");
        row.try_get::<i64>("", "n").expect("count") > 0
    }

    async fn count_rows(db: &DatabaseConnection, table: &str) -> i64 {
        db.query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            format!("SELECT COUNT(*) AS n FROM {table}"),
        ))
        .await
        .expect("count")
        .expect("a row")
        .try_get::<i64>("", "n")
        .expect("n")
    }

    async fn seed(db: &DatabaseConnection) {
        exec(
            db,
            "INSERT INTO users (email, password_hash, is_active, created_at)
             VALUES ('owner@example.com', 'h', 1, 0)",
        )
        .await;
        exec(
            db,
            "INSERT INTO groups (name, creator_id, created_at) VALUES ('Team', 1, 0)",
        )
        .await;
        exec(
            db,
            "INSERT INTO group_members (group_id, user_id, role, created_at)
             VALUES (1, 1, 'owner', 0)",
        )
        .await;
        exec(
            db,
            "INSERT INTO user_contacts (user_id, contact_email, created_at)
             VALUES (1, 'friend@example.com', 0)",
        )
        .await;
    }

    #[tokio::test]
    async fn drops_the_group_and_contact_tables() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        assert!(!table_exists(&db, "groups").await);
        assert!(!table_exists(&db, "group_members").await);
        assert!(!table_exists(&db, "user_contacts").await);
        // The ownership index the permission checks read must survive.
        assert!(table_exists(&db, "repo_members").await);
    }

    #[tokio::test]
    async fn down_recreates_them_empty() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        // Apply exactly this migration, so the rollback below targets it and
        // not the purge migration that follows.
        crate::Migrator::up(&db, Some(1))
            .await
            .expect("run migration");
        crate::Migrator::down(&db, Some(1)).await.expect("roll back");

        assert!(table_exists(&db, "groups").await);
        assert!(table_exists(&db, "group_members").await);
        assert!(table_exists(&db, "user_contacts").await);
        // The recreated tables are empty: the dropped rows are not restored.
        assert_eq!(count_rows(&db, "groups").await, 0);
        assert_eq!(count_rows(&db, "group_members").await, 0);
        assert_eq!(count_rows(&db, "user_contacts").await, 0);
    }
}
