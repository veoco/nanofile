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

    /// Verify a code and record its time step, rejecting any step already used.
    ///
    /// The skew tolerance stays in place for honest clients; only a *replayed*
    /// code (same or earlier step) is refused. Recording is best effort: a
    /// failed write must not lock a user out, so a storage error is logged and
    /// the code is accepted (the database being unwritable is a bigger problem
    /// than this check).
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
        let already_used = repos
            .user_2fa
            .find_by_user_id(user_id)
            .await
            .ok()
            .flatten()
            .and_then(|m| m.last_used_step)
            .is_some_and(|last| step <= last);
        if already_used {
            return false;
        }
        if let Err(e) = repos.user_2fa.set_last_used_step(user_id, step).await {
            tracing::warn!(user_id, "could not record the used TOTP step: {e}");
        }
        true
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
