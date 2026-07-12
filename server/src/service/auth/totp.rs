use totp_rs::{Algorithm, Secret, TOTP};

use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::user_2fa;

pub struct TotpManager;

impl TotpManager {
    pub fn generate_secret() -> String {
        let secret = Secret::generate_secret();
        secret.to_encoded().to_string()
    }

    pub fn create_totp(secret: &str, account_name: &str, issuer: &str) -> Result<TOTP, AppError> {
        let secret_obj = Secret::Encoded(secret.to_string());
        let secret_bytes = secret_obj
            .to_bytes()
            .map_err(|e| AppError::internal(e.to_string()))?;
        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret_bytes,
            Some(issuer.to_string()),
            account_name.to_string(),
        )
        .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(totp)
    }

    pub fn verify_code(totp: &TOTP, code: &str) -> bool {
        totp.check_current(code).unwrap_or(false)
    }

    pub fn get_otpauth_url(totp: &TOTP) -> String {
        totp.get_url()
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
