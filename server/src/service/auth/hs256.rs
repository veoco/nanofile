//! Minimal HS256 JWT encode/decode.
//!
//! The server only ever signs and verifies HS256 tokens, but `jsonwebtoken`
//! compiles every algorithm it supports into the binary (RSA, P-256, P-384,
//! Ed25519) even when they are unreachable. This module keeps the two operations
//! the notification protocol needs on top of crates that are already direct
//! dependencies (`hmac`, `sha2`, `base64`, `serde_json`).
//!
//! The validation is deliberately as strict as the configuration it replaces:
//!
//! * `exp` is required and must be an integer; a token is rejected once
//!   `exp < now - 60`, matching `jsonwebtoken`'s default 60-second leeway.
//! * An `aud` claim is rejected because no audience is configured, which is what
//!   `jsonwebtoken`'s `Validation::validate_aud = true` with `aud = None` does.
//! * `nbf`, `iss` and `sub` are ignored: the old validation never enabled `nbf`
//!   and never configured an issuer or subject.
//! * The header `alg` must be exactly `HS256`; `none` and any other algorithm are
//!   rejected before the signature check.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Longest token accepted before any decoding. Real tokens are a few hundred
/// bytes; the bound keeps a client-supplied string from reaching the base64 and
/// JSON parsers without limit.
const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// Clock skew allowance, matching `jsonwebtoken::Validation::leeway`'s default.
const LEEWAY_SECS: i64 = 60;

#[derive(Serialize)]
struct EncodedHeader<'a> {
    typ: &'a str,
    alg: &'a str,
}

#[derive(serde::Deserialize)]
struct DecodedHeader {
    alg: String,
}

/// Sign `claims` as a compact HS256 JWT.
///
/// The error is a short message for the caller to wrap in its own error type:
/// serialization of a small claims struct is not expected to fail, and the
/// caller reports it as an internal error.
pub fn encode<T: Serialize>(claims: &T, key: &[u8]) -> Result<String, String> {
    let header = serde_json::to_vec(&EncodedHeader {
        typ: "JWT",
        alg: "HS256",
    })
    .map_err(|e| format!("encode JWT header: {e}"))?;
    let payload = serde_json::to_vec(claims).map_err(|e| format!("encode JWT claims: {e}"))?;

    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(payload)
    );
    let signature = sign(signing_input.as_bytes(), key)?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// Verify a compact HS256 JWT and deserialize its claims.
///
/// Returns `None` for any failure: a bad token never needs a distinguishable
/// error at the call sites (both callers only ask "is this valid?").
pub fn decode<T: DeserializeOwned>(token: &str, key: &[u8]) -> Option<T> {
    if token.len() > MAX_TOKEN_BYTES {
        return None;
    }

    let mut parts = token.split('.');
    let header_b64 = parts.next()?;
    let payload_b64 = parts.next()?;
    let signature_b64 = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let header: DecodedHeader =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).ok()?).ok()?;
    if header.alg != "HS256" {
        return None;
    }

    // Decode the payload bytes only after the signature check: the JSON parser
    // must not run on an unauthenticated body.
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature = URL_SAFE_NO_PAD.decode(signature_b64).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature).ok()?;

    let payload: Value = serde_json::from_slice(&payload_bytes).ok()?;
    // `jsonwebtoken` rejects an `aud` claim when no audience is configured, and
    // both call sites leave the audience unset.
    if payload.get("aud").is_some() {
        return None;
    }
    let exp = payload.get("exp")?.as_i64()?;
    let now = chrono::Utc::now().timestamp();
    if exp < now.saturating_sub(LEEWAY_SECS) {
        return None;
    }

    serde_json::from_value(payload).ok()
}

fn sign(input: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| "HMAC key length is not supported".to_string())?;
    mac.update(input);
    Ok(mac.finalize().into_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[derive(Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Claims {
        repo_id: String,
        username: String,
        exp: i64,
    }

    fn claims(exp: i64) -> Claims {
        Claims {
            repo_id: "repo-1".to_string(),
            username: "user@example.com".to_string(),
            exp,
        }
    }

    #[test]
    fn roundtrips_a_token() {
        let original = claims(4_102_444_800);
        let token = encode(&original, KEY).expect("encode");
        let decoded: Claims = decode(&token, KEY).expect("decode");
        assert_eq!(decoded, original);
    }

    /// A token produced by the previous `jsonwebtoken` implementation (same
    /// header, same claims, same key) must keep working across the switch.
    #[test]
    fn accepts_a_token_from_the_previous_implementation() {
        const TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
            eyJyZXBvX2lkIjoicmVwby0xIiwidXNlcm5hbWUiOiJ1c2VyQGV4YW1wbGUuY29tIiwiZXhwIjo0MTAyNDQ0ODAwfQ.\
            89VCUUt42-cH6uURgmrmD99VXQ0CPmIen2VdMyT5jE4";
        let decoded: Claims = decode(TOKEN, KEY).expect("decode");
        assert_eq!(decoded.repo_id, "repo-1");
        assert_eq!(decoded.username, "user@example.com");
    }

    #[test]
    fn rejects_an_expired_token_past_the_leeway() {
        let token = encode(&claims(1_000), KEY).expect("encode");
        assert!(decode::<Claims>(&token, KEY).is_none());
    }

    #[test]
    fn accepts_a_token_expiring_inside_the_leeway() {
        let exp = chrono::Utc::now().timestamp() - 30;
        let token = encode(&claims(exp), KEY).expect("encode");
        assert!(decode::<Claims>(&token, KEY).is_some());
    }

    #[test]
    fn requires_exp() {
        const TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
            eyJyZXBvX2lkIjoicmVwby0xIiwidXNlcm5hbWUiOiJ1c2VyQGV4YW1wbGUuY29tIn0.\
            EzgOZqKRG8tKRVjF1ddwQyfqMIg7qNZLKTgDAtCnmXs";
        assert!(decode::<Claims>(TOKEN, KEY).is_none());
    }

    #[test]
    fn rejects_an_audience_claim() {
        const TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
            eyJyZXBvX2lkIjoicmVwby0xIiwidXNlcm5hbWUiOiJ1c2VyQGV4YW1wbGUuY29tIiwiZXhwIjo0MTAyNDQ0ODAwLCJhdWQiOiJ4In0.\
            7-j5jl79dGivV2VADF3iRdkj0xRQz1eLAgqZR-U7vdE";
        assert!(decode::<Claims>(TOKEN, KEY).is_none());
    }

    #[test]
    fn rejects_a_wrong_key_or_a_tampered_payload() {
        let token = encode(&claims(4_102_444_800), KEY).expect("encode");
        assert!(decode::<Claims>(&token, b"another-key-another-key-another!").is_none());

        let mut tampered = token.into_bytes();
        let payload_byte = tampered
            .iter()
            .position(|b| *b == b'.')
            .expect("header separator")
            + 2;
        tampered[payload_byte] = if tampered[payload_byte] == b'A' {
            b'B'
        } else {
            b'A'
        };
        let tampered = String::from_utf8(tampered).expect("utf8");
        assert!(decode::<Claims>(&tampered, KEY).is_none());
    }

    #[test]
    fn rejects_alg_none_and_malformed_tokens() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"repo_id":"r","username":"u","exp":4102444800}"#);
        let none_token = format!("{header}.{payload}.");
        assert!(decode::<Claims>(&none_token, KEY).is_none());

        for malformed in ["", "a.b", "a.b.c.d", "not-a-token", "...."] {
            assert!(decode::<Claims>(malformed, KEY).is_none(), "{malformed}");
        }

        let oversized = "a".repeat(MAX_TOKEN_BYTES + 1);
        assert!(decode::<Claims>(&oversized, KEY).is_none());
    }

    #[test]
    fn rejects_a_non_integer_exp() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload =
            URL_SAFE_NO_PAD.encode(br#"{"repo_id":"r","username":"u","exp":"4102444800"}"#);
        let token = format!("{header}.{payload}.x");
        assert!(decode::<Claims>(&token, KEY).is_none());
    }
}
