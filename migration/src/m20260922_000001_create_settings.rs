use sea_orm_migration::prelude::*;

/// Runtime-managed settings: one row per key saved from `/sysadmin/settings/`.
///
/// The table is deliberately schemaless about values: the catalog in
/// `infra::settings` owns what each key means, how it is parsed and whether a
/// change needs a restart. Adding a setting is therefore a code change with no
/// migration, and the database cannot disagree with the code about a key's type.
///
/// `email_settings` — a single typed row — is folded in here: its columns become
/// `email.*` keys, and its `password_enc` becomes `email.password` verbatim.
/// The value is already ciphertext under the same AEAD key (`enc1:` prefix), so
/// nothing is re-encrypted and no secret is ever written in the clear.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Settings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Settings::Key)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    // Canonical string form: the catalog parses it, so a row is
                    // readable in `sqlite3` without knowing the schema.
                    .col(ColumnDef::new(Settings::Value).text().not_null())
                    .col(
                        ColumnDef::new(Settings::UpdatedAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    // No foreign key: deleting an account must not fail on, or
                    // erase, the row of a setting it once changed.
                    .col(ColumnDef::new(Settings::UpdatedBy).integer())
                    .to_owned(),
            )
            .await?;

        if manager.has_table("email_settings").await? {
            let db = manager.get_connection();
            // Text columns are CAST so an existing INTEGER value cannot land in
            // the TEXT `value` column under a different storage class; booleans
            // become the catalog's canonical "true"/"false".
            let copies = [
                (
                    "email.paused",
                    "CASE WHEN paused THEN 'true' ELSE 'false' END",
                ),
                ("email.host", "CAST(host AS TEXT)"),
                ("email.port", "CAST(port AS TEXT)"),
                ("email.tls", "CAST(tls AS TEXT)"),
                ("email.username", "CAST(username AS TEXT)"),
                ("email.from_address", "CAST(from_address AS TEXT)"),
                ("email.from_name", "CAST(from_name AS TEXT)"),
                ("email.timeout_secs", "CAST(timeout_secs AS TEXT)"),
                ("email.max_attempts", "CAST(max_attempts AS TEXT)"),
                (
                    "email.notify_new_device",
                    "CASE WHEN notify_new_device THEN 'true' ELSE 'false' END",
                ),
                (
                    "email.notify_api_key_created",
                    "CASE WHEN notify_api_key_created THEN 'true' ELSE 'false' END",
                ),
                (
                    "email.notify_new_login",
                    "CASE WHEN notify_new_login THEN 'true' ELSE 'false' END",
                ),
            ];
            for (key, expression) in copies {
                db.execute_unprepared(&format!(
                    "INSERT OR REPLACE INTO settings (key, value, updated_at, updated_by) \
                     SELECT '{key}', {expression}, updated_at, updated_by FROM email_settings \
                     WHERE id = 1"
                ))
                .await?;
            }

            // The password may be NULL (no password configured). Copy only a
            // real value, so an "unset" row keeps falling back to the config
            // file — which is how an operator keeps the secret in a 0600 file.
            db.execute_unprepared(
                "INSERT OR REPLACE INTO settings (key, value, updated_at, updated_by) \
                 SELECT 'email.password', CAST(password_enc AS TEXT), updated_at, updated_by \
                 FROM email_settings WHERE id = 1 AND password_enc IS NOT NULL \
                   AND password_enc <> ''",
            )
            .await?;

            manager
                .drop_table(Table::drop().table(EmailSettings::Table).to_owned())
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Settings::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Settings {
    Table,
    Key,
    Value,
    UpdatedAt,
    UpdatedBy,
}

#[derive(Iden)]
enum EmailSettings {
    Table,
}
