use totp_rs::{Builder, Secret, Totp};

use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::user_2fa;

pub struct TotpManager;

impl TotpManager {
    pub fn generate_secret() -> String {
        Secret::generate().to_base32()
    }

    pub fn create_totp(secret: &str, account_name: &str, issuer: &str) -> Result<Totp, AppError> {
        let secret =
            Secret::try_from_base32(secret).map_err(|e| AppError::internal(e.to_string()))?;
        let totp = Builder::new()
            .with_secret(secret)
            .with_issuer(Some(issuer.to_string()))
            .with_account_name(account_name.to_string())
            .build()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(totp)
    }

    pub fn verify_code(totp: &Totp, code: &str) -> bool {
        totp.check_current(code).is_some()
    }

    pub fn get_otpauth_url(totp: &Totp) -> String {
        totp.to_url()
            .expect("otpauth url generation cannot fail for a valid account name")
    }

    pub async fn get_or_create_2fa(
        repos: &Repositories,
        user_id: i32,
    ) -> Result<user_2fa::Model, AppError> {
        repos
            .user_2fa
            .get_or_create(user_id, Self::generate_secret())
            .await
    }
}
