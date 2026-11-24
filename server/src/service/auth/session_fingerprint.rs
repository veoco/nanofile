//! Start-up guard that ties the stored sessions to the master secret.
//!
//! Session and account tokens are random values stored only as SHA-256 digests,
//! so they are not encrypted with `server.secret_key` (see
//! [`crate::service::auth::token::hash_token`]). The key still decides whether
//! they are trusted: this module stores a fingerprint of the secret in
//! `instance_meta` and, when the fingerprint at start-up differs from the stored
//! one, deletes every session before the listener opens. Rotating the secret
//! therefore signs every account out, and a debug build's ephemeral key does the
//! same on each restart.
//!
//! A missing fingerprint (first start after this feature, or a fresh install) is
//! recorded without revoking anything, so upgrading does not log everybody out.

use infra::entity::{api_token, instance_meta};
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set};
use sha2::{Digest, Sha256};

/// The `instance_meta` key holding the fingerprint.
pub const FINGERPRINT_KEY: &str = "session_secret_fingerprint";

/// Fingerprint of the region of the master secret that sessions are bound to.
///
/// Domain-separated so it cannot collide with another use of the same secret
/// (the CSRF key, the token cipher and the at-rest key each have their own).
pub fn fingerprint(secret_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"session-fingerprint-v1:");
    hasher.update(secret_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Compare the stored fingerprint with `secret_key` and revoke on a change.
///
/// Returns the number of session rows deleted (0 on the first start and when the
/// secret is unchanged). The comparison runs once per server generation, before
/// the HTTP listener is bound, so a request never sees a half-revoked store.
pub async fn enforce(db: &DatabaseConnection, secret_key: &str) -> Result<u64, DbErr> {
    let current = fingerprint(secret_key);
    match instance_meta::Entity::find_by_id(FINGERPRINT_KEY)
        .one(db)
        .await?
    {
        Some(row) if row.value == current => Ok(0),
        Some(_) => {
            let revoked = api_token::Entity::delete_many()
                .exec(db)
                .await?
                .rows_affected;
            instance_meta::ActiveModel {
                key: Set(FINGERPRINT_KEY.to_string()),
                value: Set(current),
            }
            .update(db)
            .await?;
            Ok(revoked)
        }
        None => {
            instance_meta::ActiveModel {
                key: Set(FINGERPRINT_KEY.to_string()),
                value: Set(current),
            }
            .insert(db)
            .await?;
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fingerprint;

    #[test]
    fn the_fingerprint_changes_with_the_secret() {
        assert_eq!(fingerprint("secret-a"), fingerprint("secret-a"));
        assert_ne!(fingerprint("secret-a"), fingerprint("secret-b"));
        assert_eq!(fingerprint("secret-a").len(), 64);
        assert!(
            fingerprint("secret-a")
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
    }

    #[test]
    fn the_fingerprint_is_domain_separated() {
        // A different purpose over the same bytes must not produce the same value;
        // the literal here mirrors the domain tag in `fingerprint`.
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(b"csrf-v1:secret-a");
        let other = hex::encode(hasher.finalize());
        assert_ne!(fingerprint("secret-a"), other);
    }
}
