use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use totp_rs::{Algorithm, Secret, TOTP};

use crate::entity::user_2fa;

pub struct TotpManager;

impl TotpManager {
    pub fn generate_secret() -> String {
        let secret = Secret::generate_secret();
        secret.to_encoded().to_string()
    }

    pub fn create_totp(
        secret: &str,
        account_name: &str,
        issuer: &str,
    ) -> Result<TOTP, Box<dyn std::error::Error>> {
        let secret_obj = Secret::Encoded(secret.to_string());
        let secret_bytes = secret_obj.to_bytes()?;
        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret_bytes,
            Some(issuer.to_string()),
            account_name.to_string(),
        )?;
        Ok(totp)
    }

    pub fn verify_code(totp: &TOTP, code: &str) -> bool {
        totp.check_current(code).unwrap_or(false)
    }

    pub fn get_otpauth_url(totp: &TOTP) -> String {
        totp.get_url()
    }

    pub async fn get_or_create_2fa(
        db: &DatabaseConnection,
        user_id: i32,
    ) -> Result<user_2fa::Model, sea_orm::DbErr> {
        let existing = user_2fa::Entity::find()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .one(db)
            .await?;

        match existing {
            Some(model) => Ok(model),
            None => {
                let secret = Self::generate_secret();
                let model = user_2fa::ActiveModel {
                    user_id: sea_orm::Set(user_id),
                    totp_secret: sea_orm::Set(secret),
                    algorithm: sea_orm::Set("SHA1".to_string()),
                    digits: sea_orm::Set(6),
                    period: sea_orm::Set(30),
                    enabled: sea_orm::Set(false),
                    enabled_at: sea_orm::NotSet,
                };
                user_2fa::Entity::insert(model).exec(db).await?;
                user_2fa::Entity::find()
                    .filter(user_2fa::Column::UserId.eq(user_id))
                    .one(db)
                    .await?
                    .ok_or_else(|| {
                        sea_orm::DbErr::Custom("failed to find inserted 2fa".to_string())
                    })
            }
        }
    }
}
