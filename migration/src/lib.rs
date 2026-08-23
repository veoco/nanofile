pub use sea_orm_migration::prelude::*;

/// Declare each migration module once and register it in `Migrator`,
/// so adding a new migration touches a single list.
macro_rules! migrations {
    ($($name:ident),* $(,)?) => {
        $(mod $name;)*

        pub struct Migrator;

        impl MigratorTrait for Migrator {
            fn migrations() -> Vec<Box<dyn MigrationTrait>> {
                vec![$(Box::new($name::Migration)),*]
            }
        }
    };
}

migrations!(
    m20260601_000001_create_users,
    m20260601_000002_create_api_tokens,
    m20260601_000003_create_repos,
    m20260601_000004_create_repo_members,
    m20260601_000005_create_commits,
    m20260601_000006_create_fs_objects,
    m20260601_000007_create_sync_tokens,
    m20260601_000008_create_dir_entries,
    m20260601_000009_create_user_2fa,
    m20260601_000010_create_user_2fa_backup_codes,
    m20260601_000011_create_share_links,
    m20260601_000012_create_upload_links,
    m20260603_000001_extend_schema,
    m20260604_000001_create_activities,
    m20260604_000002_create_wikis,
    m20260604_000003_create_sdoc_comments,
    m20260604_000004_create_metadata,
    m20260605_000001_create_client_login_tokens,
    m20260606_000001_store_json_in_fs_objects,
    m20260607_000001_create_s2fa_tokens,
    m20260607_000002_add_sync_token_peer_info,
    m20260607_000003_create_invitation_codes,
    m20260607_000004_add_invited_by_to_users,
    m20260607_000005_create_password_reset_tokens,
    m20260607_000006_add_is_admin_to_users,
    m20260607_000007_add_email_to_invitation_codes,
    m20260609_000001_create_file_lock_timestamps,
    m20260614_000001_add_is_dir_to_starred_files,
    m20260614_000002_add_old_path_to_activities,
    m20260619_000001_add_share_link_fields,
    m20260619_000003_add_detail_to_activities,
    m20260619_000004_create_avatars,
    m20260620_000001_create_file_trash,
    m20260620_000002_create_deleted_repos,
    m20260705_000001_add_storage_quota_to_users,
    m20260802_000001_create_webdav_keys,
    m20260806_000001_add_history_settings_to_repos,
    m20260806_000002_add_language_to_users,
    m20260806_000003_add_perf_indexes,
    m20260808_000001_create_repo_tags,
    m20260814_000001_add_repo_type,
    m20260814_000002_drop_wikis,
    m20260814_000003_create_wiki2_publish,
    m20260814_000004_add_sso_accessed_at,
    m20260814_000005_add_query_indexes,
    m20260814_000006_drop_sdoc_comments,
    m20260814_000007_add_more_query_indexes,
    m20260814_000008_add_remaining_indexes,
    m20260814_000009_drop_dir_entries,
    m20260816_000001_hash_tokens,
    m20260816_000002_add_api_token_pending,
    m20260823_000001_hash_short_lived_tokens,
);

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{EntityTrait, PaginatorTrait};
    use sea_orm_migration::MigratorTrait;
    use sea_orm_migration::seaql_migrations::Entity as SeaqlMigrations;

    /// Verify the full migration chain applies cleanly to a fresh database.
    ///
    /// Catches ordering/`self.manager`/index-creation regressions that only
    /// surface when every migration runs end-to-end against an empty schema.
    #[tokio::test]
    async fn migrates_fresh_database_from_scratch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let db =
            sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", db_path.display()).as_str())
                .await
                .expect("connect sqlite");

        Migrator::up(&db, None).await.expect("run all migrations");

        // Every migration must be recorded in the migrations table.
        let applied = SeaqlMigrations::find()
            .count(&db)
            .await
            .expect("count migrations");
        assert_eq!(
            applied as usize,
            Migrator::migrations().len(),
            "migration chain did not apply fully"
        );

        let _ = db.close().await;
    }
}
