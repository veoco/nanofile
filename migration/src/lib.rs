pub use sea_orm_migration::prelude::*;

mod m20260601_000001_create_users;
mod m20260601_000002_create_api_tokens;
mod m20260601_000003_create_repos;
mod m20260601_000004_create_repo_members;
mod m20260601_000005_create_commits;
mod m20260601_000006_create_fs_objects;
mod m20260601_000007_create_sync_tokens;
mod m20260601_000008_create_dir_entries;
mod m20260601_000009_create_user_2fa;
mod m20260601_000010_create_user_2fa_backup_codes;
mod m20260601_000011_create_share_links;
mod m20260601_000012_create_upload_links;
mod m20260603_000001_extend_schema;
mod m20260604_000001_create_activities;
mod m20260604_000002_create_wikis;
mod m20260604_000003_create_sdoc_comments;
mod m20260604_000004_create_metadata;
mod m20260605_000001_create_client_login_tokens;
mod m20260606_000001_store_json_in_fs_objects;
mod m20260607_000001_create_s2fa_tokens;
mod m20260607_000002_add_sync_token_peer_info;
mod m20260607_000003_create_invitation_codes;
mod m20260607_000004_add_invited_by_to_users;
mod m20260607_000005_create_password_reset_tokens;
mod m20260607_000006_add_is_admin_to_users;
mod m20260607_000007_add_email_to_invitation_codes;
mod m20260609_000001_create_file_lock_timestamps;
mod m20260614_000001_add_is_dir_to_starred_files;
mod m20260614_000002_add_old_path_to_activities;
mod m20260619_000001_add_share_link_fields;
mod m20260619_000003_add_detail_to_activities;
mod m20260619_000004_create_avatars;

pub struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260601_000001_create_users::Migration),
            Box::new(m20260601_000002_create_api_tokens::Migration),
            Box::new(m20260601_000003_create_repos::Migration),
            Box::new(m20260601_000004_create_repo_members::Migration),
            Box::new(m20260601_000005_create_commits::Migration),
            Box::new(m20260601_000006_create_fs_objects::Migration),
            Box::new(m20260601_000007_create_sync_tokens::Migration),
            Box::new(m20260601_000008_create_dir_entries::Migration),
            Box::new(m20260601_000009_create_user_2fa::Migration),
            Box::new(m20260601_000010_create_user_2fa_backup_codes::Migration),
            Box::new(m20260601_000011_create_share_links::Migration),
            Box::new(m20260601_000012_create_upload_links::Migration),
            Box::new(m20260603_000001_extend_schema::Migration),
            Box::new(m20260604_000001_create_activities::Migration),
            Box::new(m20260604_000002_create_wikis::Migration),
            Box::new(m20260604_000003_create_sdoc_comments::Migration),
            Box::new(m20260604_000004_create_metadata::Migration),
            Box::new(m20260605_000001_create_client_login_tokens::Migration),
            Box::new(m20260606_000001_store_json_in_fs_objects::Migration),
            Box::new(m20260607_000001_create_s2fa_tokens::Migration),
            Box::new(m20260607_000002_add_sync_token_peer_info::Migration),
            Box::new(m20260607_000003_create_invitation_codes::Migration),
            Box::new(m20260607_000004_add_invited_by_to_users::Migration),
            Box::new(m20260607_000005_create_password_reset_tokens::Migration),
            Box::new(m20260607_000006_add_is_admin_to_users::Migration),
            Box::new(m20260607_000007_add_email_to_invitation_codes::Migration),
            Box::new(m20260609_000001_create_file_lock_timestamps::Migration),
            Box::new(m20260614_000001_add_is_dir_to_starred_files::Migration),
            Box::new(m20260614_000002_add_old_path_to_activities::Migration),
            Box::new(m20260619_000001_add_share_link_fields::Migration),
            Box::new(m20260619_000003_add_detail_to_activities::Migration),
            Box::new(m20260619_000004_create_avatars::Migration),
        ]
    }
}
