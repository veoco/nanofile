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

    /// Verify a code, returning the matched TOTP time step.
    ///
    /// The step is what makes a code single-use: `totp-rs` accepts the previous,
    /// current and next step (±30s of clock skew), so a code observed by an
    /// attacker would otherwise stay valid for up to ~90 seconds. Callers should
    /// prefer [`Self::verify_and_consume`], which also records the step.
    pub fn verify_code(totp: &Totp, code: &str) -> Option<u64> {
        totp.check_current(code)
    }

    /// Verify a code and atomically claim its time step.
    ///
    /// The skew tolerance (`totp-rs` accepts the previous, current and next
    /// step, so a code stays valid for ~90 s) is what makes a claim necessary:
    /// a code observed by someone else must not be replayable. Claiming is a
    /// single conditional UPDATE (`last_used_step < step`), so two concurrent
    /// submissions of the same code cannot both win — a read-then-write guard
    /// left exactly that window open.
    ///
    /// A storage error fails **closed**: this gates authentication, so an
    /// unwritable database must not silently disable the replay guard.
    pub async fn verify_and_consume(
        repos: &crate::repository::Repositories,
        user_id: i32,
        totp: &Totp,
        code: &str,
    ) -> bool {
        let Some(step) = Self::verify_code(totp, code) else {
            return false;
        };
        let step = step as i64;
        match repos.user_2fa.consume_step(user_id, step).await {
            Ok(claimed) => claimed,
            Err(e) => {
                tracing::error!(user_id, "could not record the used TOTP step: {e}");
                false
            }
        }
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
