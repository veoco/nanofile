//! Self-hosted proof-of-work challenges for the anonymous web forms.
//!
//! The server issues a random seed and a difficulty; the browser finds a nonce
//! such that `SHA-256(seed || nonce)` has at least that many leading zero bits.
//! The server re-computes the hash and consumes the challenge, so the work cannot
//! be replayed. This is a cost multiplier, not a human/AI discriminator: it stops
//! naive automation and raises the price of volumetric abuse, while the login
//! lockout, per-IP limits and email verification remain the primary controls.
//!
//! Challenges live in this process only, like the rate limiters: a restart drops
//! them, and a user simply reloads the form. The store is bounded and expires
//! lazily, because every entry is anonymous and client-triggered.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use infra::config::{CAPTCHA_MAX_BITS, CAPTCHA_MIN_BITS};
use rand::Rng;
use sha2::{Digest, Sha256};

/// Lifetime of one challenge. Long enough for a slow phone and a typo.
const TTL_SECS: i64 = 600;
/// What the process may hold; an entry is roughly an id, a seed and a timestamp.
const MAX_ENTRIES: usize = 20_000;
/// How many extra bits one failed sign-in may add.
const ADAPTIVE_STEPS: u32 = 3;
const ID_BYTES: usize = 16;
const SEED_BYTES: usize = 16;
const ID_HEX_LEN: usize = ID_BYTES * 2;
const NONCE_MAX_DIGITS: usize = 20;
const HASH_INPUT_LEN: usize = SEED_BYTES + 8;

/// Which form a challenge was issued for. A challenge cannot be moved between
/// purposes even if its id leaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowPurpose {
    Login,
    Register,
    PasswordReset,
}

/// The browser-visible part of a challenge. The answer (the seed's hash target)
/// stays on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowView {
    /// Random 128-bit identifier, hex.
    pub id: String,
    /// Random 128-bit seed, hex; the browser hashes `seed || nonce`.
    pub seed: String,
    /// Required leading zero bits.
    pub bits: u8,
}

struct Challenge {
    seed: [u8; SEED_BYTES],
    purpose: PowPurpose,
    bits: u8,
    expires_at: i64,
}

/// In-memory challenge table shared through `AppState`.
pub struct PowStore {
    challenges: Mutex<HashMap<String, Challenge>>,
    max_entries: usize,
}

impl Default for PowStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PowStore {
    pub fn new() -> Self {
        Self::with_capacity(MAX_ENTRIES)
    }

    fn with_capacity(max_entries: usize) -> Self {
        Self {
            challenges: Mutex::new(HashMap::new()),
            max_entries: max_entries.max(1),
        }
    }

    /// Issue a challenge for `purpose`, clamping the difficulty to the accepted
    /// range. The caller has already clamped the configured base.
    pub fn issue(&self, purpose: PowPurpose, bits: u8) -> PowView {
        self.issue_at(purpose, bits, now())
    }

    fn issue_at(&self, purpose: PowPurpose, bits: u8, now: i64) -> PowView {
        let mut id_bytes = [0u8; ID_BYTES];
        rand::rng().fill_bytes(&mut id_bytes);
        let mut seed = [0u8; SEED_BYTES];
        rand::rng().fill_bytes(&mut seed);

        let id = hex::encode(id_bytes);
        let bits = bits.clamp(CAPTCHA_MIN_BITS, CAPTCHA_MAX_BITS);

        let mut map = self.lock();
        // The cap is a memory bound, not a correctness one: when a flood fills
        // the table, an evicted owner simply reloads the form. Doing this only
        // when full keeps the common path O(1) instead of scanning every entry
        // on every anonymous page load.
        if map.len() >= self.max_entries
            && let Some(oldest) = map.keys().next().cloned()
        {
            map.remove(&oldest);
        }

        map.insert(
            id.clone(),
            Challenge {
                seed,
                purpose,
                bits,
                expires_at: now + TTL_SECS,
            },
        );

        PowView {
            id,
            seed: hex::encode(seed),
            bits,
        }
    }

    /// Verify and consume a challenge. Every call removes the entry, so a wrong
    /// answer buys exactly one guess; the caller re-renders the form with a fresh
    /// challenge on failure.
    pub fn verify(&self, id: &str, nonce: &str, purpose: PowPurpose) -> bool {
        self.verify_at(id, nonce, purpose, now())
    }

    fn verify_at(&self, id: &str, nonce: &str, purpose: PowPurpose, now: i64) -> bool {
        if id.len() != ID_HEX_LEN
            || nonce.is_empty()
            || nonce.len() > NONCE_MAX_DIGITS
            || !nonce.bytes().all(|byte| byte.is_ascii_digit())
        {
            return false;
        }
        let Ok(nonce) = nonce.parse::<u64>() else {
            return false;
        };

        let Some(challenge) = self.lock().remove(id) else {
            return false;
        };
        if challenge.purpose != purpose || challenge.expires_at <= now {
            return false;
        }

        let input = hash_input(&challenge.seed, nonce);
        let digest: [u8; 32] = Sha256::digest(input).into();
        leading_zero_bits(&digest) >= u32::from(challenge.bits)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Challenge>> {
        self.challenges
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Extra bits earned by recent failures, capped at the configured maximum.
///
/// Called with the same failure count the login lockout sees, so a shared address
/// gets harder challenges instead of only being locked out later.
pub fn bits_for(base: u8, failures: u32) -> u8 {
    base.saturating_add(failures.min(ADAPTIVE_STEPS) as u8)
        .min(CAPTCHA_MAX_BITS)
}

/// The exact bytes both sides hash: `seed || nonce` as a 64-bit big-endian value.
fn hash_input(seed: &[u8; SEED_BYTES], nonce: u64) -> [u8; HASH_INPUT_LEN] {
    let mut input = [0u8; HASH_INPUT_LEN];
    input[..SEED_BYTES].copy_from_slice(seed);
    input[SEED_BYTES..].copy_from_slice(&nonce.to_be_bytes());
    input
}

/// Count leading zero bits, so `bits = 12` means `0x00 0x0f` or better.
fn leading_zero_bits(hash: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force a solution the same way the browser does. Tests use 8–12 bits.
    fn solve(seed_hex: &str, bits: u8) -> u64 {
        let seed: [u8; SEED_BYTES] = hex::decode(seed_hex).unwrap().try_into().unwrap();
        for nonce in 0..=u32::MAX as u64 {
            let digest: [u8; 32] = Sha256::digest(hash_input(&seed, nonce)).into();
            if leading_zero_bits(&digest) >= u32::from(bits) {
                return nonce;
            }
        }
        unreachable!("a 32-bit search space always contains a 12-bit solution")
    }

    #[test]
    fn leading_zero_bits_counts_bits_not_bytes() {
        assert_eq!(leading_zero_bits(&[0u8; 32]), 256);
        let mut hash = [0u8; 32];
        hash[1] = 0x0f;
        assert_eq!(leading_zero_bits(&hash), 12);
        hash[0] = 0x7f;
        assert_eq!(leading_zero_bits(&hash), 1);
        hash[0] = 0x80;
        assert_eq!(leading_zero_bits(&hash), 0);
    }

    #[test]
    fn a_solution_verifies_once() {
        let store = PowStore::with_capacity(8);
        let at = now();
        let view = store.issue_at(PowPurpose::Login, 8, at);
        let nonce = solve(&view.seed, view.bits);

        assert!(store.verify_at(&view.id, &nonce.to_string(), PowPurpose::Login, at));
        assert!(
            !store.verify_at(&view.id, &nonce.to_string(), PowPurpose::Login, at),
            "a challenge is consumed by its first answer"
        );
    }

    #[test]
    fn a_wrong_answer_is_consumed_too() {
        let store = PowStore::with_capacity(8);
        let at = now();
        let view = store.issue_at(PowPurpose::Login, 8, at);
        let nonce = solve(&view.seed, view.bits);

        assert!(!store.verify_at(&view.id, &(nonce + 1).to_string(), PowPurpose::Login, at));
        assert!(
            !store.verify_at(&view.id, &nonce.to_string(), PowPurpose::Login, at),
            "the single allowed guess is gone"
        );
    }

    #[test]
    fn expired_and_cross_purpose_challenges_are_rejected() {
        let store = PowStore::with_capacity(8);
        let at = now();
        let view = store.issue_at(PowPurpose::Login, 8, at);
        let nonce = solve(&view.seed, view.bits);

        assert!(!store.verify_at(&view.id, &nonce.to_string(), PowPurpose::Register, at));

        let view = store.issue_at(PowPurpose::Login, 8, at);
        let nonce = solve(&view.seed, view.bits);
        assert!(!store.verify_at(
            &view.id,
            &nonce.to_string(),
            PowPurpose::Login,
            at + TTL_SECS + 1
        ));
    }

    #[test]
    fn malformed_ids_and_nonces_are_rejected() {
        let store = PowStore::with_capacity(8);
        let at = now();
        let view = store.issue_at(PowPurpose::Login, 8, at);
        let nonce = solve(&view.seed, view.bits).to_string();

        assert!(!store.verify_at("short", &nonce, PowPurpose::Login, at));
        assert!(!store.verify_at(&view.id, "", PowPurpose::Login, at));
        assert!(!store.verify_at(&view.id, "12x", PowPurpose::Login, at));
        assert!(!store.verify_at(&view.id, &"9".repeat(21), PowPurpose::Login, at));
        // The last malformed attempt did not consume the id, so the real answer works.
        assert!(store.verify_at(&view.id, &nonce, PowPurpose::Login, at));
    }

    #[test]
    fn the_store_is_bounded() {
        let store = PowStore::with_capacity(2);
        let at = now();
        store.issue_at(PowPurpose::Login, 8, at);
        store.issue_at(PowPurpose::Login, 8, at);
        store.issue_at(PowPurpose::Login, 8, at);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn bits_grow_with_failures_and_stop_at_the_cap() {
        assert_eq!(bits_for(17, 0), 17);
        assert_eq!(bits_for(17, 1), 18);
        assert_eq!(bits_for(17, 3), 20);
        assert_eq!(bits_for(17, 99), 20);
        assert_eq!(bits_for(CAPTCHA_MAX_BITS, 99), CAPTCHA_MAX_BITS);
    }
}
