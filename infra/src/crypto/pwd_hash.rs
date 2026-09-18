//! `pwd_hash`-based library passwords (Seafile 11 / seahub
//! `ENCRYPTED_LIBRARY_PWD_HASH_ALGO`).
//!
//! Up to Seafile 10 an encrypted library stored a `magic`
//! (see [`crate::crypto::key_derivation::generate_magic`]) and both sides
//! re-derived it with PBKDF2-SHA256 at a hard-coded 1000 iterations. Seafile 11
//! added a second, configurable verifier: `pwd_hash`, computed from
//! `repo_id + password` with a named algorithm and explicit parameters
//! (`common/password-hash.c`, `seafile_generate_pwd_hash()`).
//!
//! * `pbkdf2_sha256` — `PBKDF2-HMAC-SHA256`, `params_str` is the iteration count
//! * `argon2id` — raw Argon2id (v1.3, no secret/associated data),
//!   `params_str` is `time_cost,memory_cost,parallelism`
//!
//! The algorithm is advertised to clients through `/api2/server-info/`; the
//! desktop client compares the value verbatim, so the names below must stay
//! lowercase and exact. A library created through the `pwd_hash` flow carries
//! **no** `magic` at all (the commit omits the field), which is why every
//! password check has to go through this module instead.
//!
//! # The v1/v2 salt is not what it looks like
//!
//! `seafile_generate_pwd_hash()` uses the fixed 8-byte salt for
//! `enc_version <= 2`, but hands it to `pwd_hash_derive_key()` as a hex string
//! produced by `rawdata_to_hex(salt, fixed_salt, 8)` into a 64-byte zeroed
//! buffer — i.e. `"da9045c306c7cc26"` followed by a NUL. `pwd_hash_derive_key()`
//! then calls `hex_to_rawdata(salt, salt_bin, 32)`, whose `hexval()` returns
//! `~0` for `'\0'`, so the loop bails out at the NUL **without writing** the
//! offending pair. The 32-byte buffer therefore ends up as the 8 magic bytes
//! followed by **24 zero bytes**.
//!
//! Note that this differs from `seafile_derive_key()`'s v2 branch (used for
//! `magic`), which hashes with the 8-byte salt only. The two v2 salts are
//! different lengths on purpose; do not "unify" them.

use argon2::{Algorithm, Argon2, Params, Version};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::crypto::key_derivation::{CryptoError, MAGIC_SALT};

/// seafile-server's `PWD_HASH_PDKDF2` (`common/password-hash.h`).
pub const ALGO_PBKDF2_SHA256: &str = "pbkdf2_sha256";
/// seafile-server's `PWD_HASH_ARGON2ID` (`common/password-hash.h`).
pub const ALGO_ARGON2ID: &str = "argon2id";

/// `parse_pbkdf2_sha256_params()` fallback.
pub const PBKDF2_DEFAULT_ITERATIONS: u32 = 1000;
/// `parse_argon2id_params()` fallback, as `(time_cost, memory_cost, parallelism)`.
pub const ARGON2ID_DEFAULT_PARAMS: (u32, u32, u32) = (2, 102_400, 8);

// ── Resource bounds (deliberate deviation from upstream) ────────────────────
//
// Upstream validates only the algorithm name and `strlen(pwd_hash)`. The
// parameters, however, are chosen by the *client* at creation time and are
// replayed on every later password check, so an unbounded value is a
// server-side resource-exhaustion primitive: `2,4294967295,8` makes every
// `set-password` attempt to allocate terabytes. We bound the *parsed* values
// (so the desktop client's habit of sending the algorithm name as
// `pwd_hash_params` still lands on the defaults) and reject at creation, at
// config load, and at verification.

/// Upper bound for a `pbkdf2_sha256` iteration count.
pub const MAX_PBKDF2_ITERATIONS: u32 = 50_000_000;
/// Upper bound for Argon2id `time_cost`.
pub const MAX_ARGON2_T_COST: u32 = 64;
/// Upper bound for Argon2id `memory_cost` (KiB), i.e. 1 GiB.
pub const MAX_ARGON2_M_COST_KIB: u32 = 1_048_576;
/// Upper bound for Argon2id `parallelism`.
pub const MAX_ARGON2_P_COST: u32 = 64;
/// Upper bound for `memory_cost × time_cost`, in KiB-passes.
///
/// Bounding the two independently is not enough: `1 GiB × 64 passes` satisfies
/// both and still costs minutes of CPU per password check. 2 GiB-passes keeps
/// the worst case in the same order as the defaults (`100 MiB × 2`), while
/// still allowing e.g. 1 GiB with the default 2 passes.
pub const MAX_ARGON2_MEMORY_TIME_KIB_PASSES: u64 = 2 * 1024 * 1024;

/// Whether `algo` is one of the two algorithms seafile implements.
pub fn is_supported_algo(algo: &str) -> bool {
    algo == ALGO_PBKDF2_SHA256 || algo == ALGO_ARGON2ID
}

/// C `atoi()`/`atoll()` semantics: skip leading whitespace, accept an optional
/// sign, then consume digits and stop at the first non-digit. Returns 0 when
/// there is no number at all.
///
/// A faithful port matters: the desktop client sends the *algorithm name* in
/// the `pwd_hash_params` field (`create-repo-dialog.cpp` assigns
/// `getEncryptedLibraryPwdHashAlgo()` to `pwd_hash_params`), and upstream turns
/// `"pbkdf2_sha256"` into 0 and then into the default iteration count.
fn c_atoi(s: &str) -> i64 {
    let bytes = s.as_bytes();
    let mut i = 0;
    // C isspace(): space, \t, \n, \v, \f, \r
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        i += 1;
    }
    let mut negative = false;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        negative = bytes[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut value: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if i == start {
        return 0;
    }
    if negative { -value } else { value }
}

/// Convert a `c_atoi()` result the way upstream would end up storing it.
/// Values that do not fit are saturated, which [`validate_params`] then rejects.
fn positive_or(default: u32, value: i64) -> u32 {
    if value <= 0 {
        default
    } else {
        u32::try_from(value).unwrap_or(u32::MAX)
    }
}

/// `parse_pbkdf2_sha256_params()`: `None` and any non-positive parse mean the
/// upstream default of 1000 iterations.
pub fn parse_pbkdf2_params(params: Option<&str>) -> u32 {
    match params {
        None => PBKDF2_DEFAULT_ITERATIONS,
        Some(s) => positive_or(PBKDF2_DEFAULT_ITERATIONS, c_atoi(s)),
    }
}

/// `parse_argon2id_params()`: `"t,m,p"`. A string that does not split into
/// exactly three comma-separated fields falls back to the whole default triple;
/// each individual field that parses to a non-positive number falls back to its
/// own default.
///
/// `g_strsplit(s, ",", 3)` stops after three fields, so `"1,2,3,4"` yields
/// `("1", "2", "3,4")` and `atoll("3,4")` is 3 — reproduced here with
/// `splitn(3, ',')` plus a `c_atoi()` that stops at the comma.
pub fn parse_argon2id_params(params: Option<&str>) -> (u32, u32, u32) {
    let (default_t, default_m, default_p) = ARGON2ID_DEFAULT_PARAMS;
    let Some(s) = params else {
        return (default_t, default_m, default_p);
    };
    let fields: Vec<&str> = s.splitn(3, ',').collect();
    if fields.len() != 3 {
        return (default_t, default_m, default_p);
    }
    (
        positive_or(default_t, c_atoi(fields[0].trim())),
        positive_or(default_m, c_atoi(fields[1].trim())),
        positive_or(default_p, c_atoi(fields[2].trim())),
    )
}

/// Reject an unsupported algorithm and out-of-range parameters.
///
/// Applied when the config is loaded, when a client proposes `pwd_hash`
/// parameters at creation, and again before verifying a stored hash.
pub fn validate_params(algo: &str, params: Option<&str>) -> Result<(), String> {
    match algo {
        ALGO_PBKDF2_SHA256 => {
            let iterations = parse_pbkdf2_params(params);
            if iterations > MAX_PBKDF2_ITERATIONS {
                return Err(format!(
                    "pbkdf2_sha256 iterations = {iterations} exceeds the supported maximum of \
                     {MAX_PBKDF2_ITERATIONS}"
                ));
            }
            Ok(())
        }
        ALGO_ARGON2ID => {
            let (t, m, p) = parse_argon2id_params(params);
            if t > MAX_ARGON2_T_COST {
                return Err(format!(
                    "argon2id time_cost = {t} exceeds the supported maximum of {MAX_ARGON2_T_COST}"
                ));
            }
            if m > MAX_ARGON2_M_COST_KIB {
                return Err(format!(
                    "argon2id memory_cost = {m} KiB exceeds the supported maximum of \
                     {MAX_ARGON2_M_COST_KIB} KiB"
                ));
            }
            if p > MAX_ARGON2_P_COST {
                return Err(format!(
                    "argon2id parallelism = {p} exceeds the supported maximum of \
                     {MAX_ARGON2_P_COST}"
                ));
            }
            // `Params::new` additionally requires `m_cost >= 8 * p_cost`; let it
            // report that rather than duplicating the rule here.
            if m < 8u32.saturating_mul(p) {
                return Err(format!(
                    "argon2id memory_cost = {m} KiB is too small for parallelism = {p}"
                ));
            }
            // Both maxima can be satisfied while the *product* is still minutes
            // of CPU (1 GiB × 64), so bound the work as well.
            let kib_passes = u64::from(m) * u64::from(t);
            if kib_passes > MAX_ARGON2_MEMORY_TIME_KIB_PASSES {
                return Err(format!(
                    "argon2id memory_cost × time_cost = {kib_passes} KiB-passes exceeds the \
                     supported maximum of {MAX_ARGON2_MEMORY_TIME_KIB_PASSES} \
                     (m_cost = {m}, t_cost = {t})"
                ));
            }
            Ok(())
        }
        other => Err(format!("unsupported pwd_hash algorithm: {other}")),
    }
}

/// The 32-byte salt `pwd_hash_derive_key()` actually sees (see the module docs).
fn pwd_hash_salt(enc_version: i32, repo_salt: &str) -> Result<[u8; 32], CryptoError> {
    let mut salt = [0u8; 32];
    if enc_version <= 2 {
        salt[..8].copy_from_slice(&MAGIC_SALT);
        return Ok(salt);
    }
    let decoded = hex::decode(repo_salt).map_err(|e| CryptoError::InvalidSalt(e.to_string()))?;
    if decoded.len() != 32 {
        return Err(CryptoError::InvalidSalt(format!(
            "pwd_hash needs the 64-hex-char library salt (32 bytes), got {} bytes",
            decoded.len()
        )));
    }
    salt.copy_from_slice(&decoded);
    Ok(salt)
}

/// Compute `pwd_hash` exactly like `seafile_generate_pwd_hash()`.
///
/// Unlike the commit id, the hash input is the bare `repo_id + password` byte
/// string — no NUL terminator is hashed.
pub fn derive_pwd_hash(
    repo_id: &str,
    password: &str,
    enc_version: i32,
    repo_salt: &str,
    algo: &str,
    params: Option<&str>,
) -> Result<String, CryptoError> {
    let salt = pwd_hash_salt(enc_version, repo_salt)?;
    let input = format!("{repo_id}{password}");

    let key: [u8; 32] = match algo {
        ALGO_PBKDF2_SHA256 => {
            let iterations = parse_pbkdf2_params(params);
            let mut key = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<Sha256>(input.as_bytes(), &salt, iterations, &mut key);
            key
        }
        ALGO_ARGON2ID => {
            // C's `argon2id_hash_raw(t_cost, memory_cost, parallelism, ...)` —
            // note that `Params::new` takes memory first.
            let (t_cost, m_cost, p_cost) = parse_argon2id_params(params);
            let argon_params = Params::new(m_cost, t_cost, p_cost, Some(32))
                .map_err(|e| CryptoError::HashFailed(format!("invalid argon2id params: {e}")))?;
            let mut key = [0u8; 32];
            Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params)
                .hash_password_into(input.as_bytes(), &salt, &mut key)
                .map_err(|e| CryptoError::HashFailed(format!("argon2id failed: {e}")))?;
            key
        }
        other => return Err(CryptoError::UnsupportedAlgo(other.to_string())),
    };

    Ok(hex::encode(key))
}

/// Check a password against a stored `pwd_hash`
/// (`seafile_pwd_hash_verify_repo_passwd()`).
///
/// Fails closed: an unsupported algorithm or out-of-range stored parameters
/// return `false` rather than attempting an unbounded derivation.
pub fn verify_pwd_hash(
    repo_id: &str,
    password: &str,
    enc_version: i32,
    repo_salt: &str,
    stored_pwd_hash: &str,
    algo: &str,
    params: Option<&str>,
) -> bool {
    if validate_params(algo, params).is_err() {
        return false;
    }
    let computed = match derive_pwd_hash(repo_id, password, enc_version, repo_salt, algo, params) {
        Ok(h) => h,
        Err(_) => return false,
    };
    // Constant-time comparison; the hash itself is password-equivalent.
    Sha256::digest(computed.as_bytes())
        .as_slice()
        .ct_eq(Sha256::digest(stored_pwd_hash.as_bytes()).as_slice())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_ID: &str = "11111111-1111-1111-1111-111111111111";
    const PASSWORD: &str = "hunter2";
    const SALT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SALT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Golden vectors produced by `/tmp/pwdhash_ref/pwdhash_ref`, a driver built
    /// from `common/password-hash.c` + `common/seafile-crypt.c` +
    /// `lib/utils.c` verbatim (see the module docs for the salt subtlety).
    /// The v2 default was independently reproduced with
    /// `openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt pass:... \
    ///  -kdfopt hexsalt:da9045c306c7cc26000...0 -kdfopt iter:1000 PBKDF2`.
    #[test]
    fn test_derive_pwd_hash_pbkdf2_matches_seafile_c() {
        let cases: &[(i32, &str, Option<&str>, &str)] = &[
            (
                2,
                "",
                None,
                "5358432032aca60b6e828950082a1afa3dff0cce09fd11ac1d36d0b889fac294",
            ),
            (
                2,
                "",
                Some("5000"),
                "018927c478bd10359994dc351f0376f3e3fae55032d536fcb0e4684f7ddad5d6",
            ),
            (
                4,
                SALT_A,
                None,
                "30a70fcc53e723b332068ec4037f3f5b853b89771a4ebdf8e088fec462d1ab3b",
            ),
            (
                4,
                SALT_A,
                Some("2500"),
                "a9858e6e5059827780da92c5c0c97a27e58f347653d60f18cbaad0d6721b7965",
            ),
            (
                4,
                SALT_B,
                Some("12000"),
                "408f7c4d02a3e7e3957e6849b2fe129ca83e3257fc9be8ea25194ac48760da5a",
            ),
        ];
        for (enc_version, salt, params, expected) in cases {
            let got = derive_pwd_hash(
                REPO_ID,
                PASSWORD,
                *enc_version,
                salt,
                ALGO_PBKDF2_SHA256,
                *params,
            )
            .unwrap();
            assert_eq!(got, *expected, "enc={enc_version} params={params:?}");
        }
    }

    /// The desktop client sends the algorithm name where the parameters belong
    /// (`create-repo-dialog.cpp`), so this must parse to the 1000-iteration
    /// default rather than failing.
    #[test]
    fn test_pbkdf2_params_that_are_an_algorithm_name_fall_back_to_default() {
        assert_eq!(parse_pbkdf2_params(Some("pbkdf2_sha256")), 1000);
        assert_eq!(parse_pbkdf2_params(None), 1000);
        assert_eq!(parse_pbkdf2_params(Some("")), 1000);
        assert_eq!(parse_pbkdf2_params(Some("abc")), 1000);
        assert_eq!(parse_pbkdf2_params(Some("0")), 1000);
        assert_eq!(parse_pbkdf2_params(Some("-5")), 1000);
        assert_eq!(parse_pbkdf2_params(Some("5000")), 5000);
        assert_eq!(parse_pbkdf2_params(Some(" 7000")), 7000);
        assert_eq!(parse_pbkdf2_params(Some("8000x")), 8000);
    }

    #[test]
    fn test_parse_argon2id_params_matches_g_strsplit() {
        let defaults = ARGON2ID_DEFAULT_PARAMS;
        assert_eq!(parse_argon2id_params(None), defaults);
        assert_eq!(parse_argon2id_params(Some("argon2id")), defaults);
        assert_eq!(parse_argon2id_params(Some("1,2")), defaults);
        assert_eq!(parse_argon2id_params(Some("1,2,3,4")), (1, 2, 3));
        assert_eq!(parse_argon2id_params(Some(" 2 , 4096 , 8 ")), (2, 4096, 8));
        // Per-field fallbacks are independent.
        assert_eq!(parse_argon2id_params(Some("0,0,0")), defaults);
        assert_eq!(parse_argon2id_params(Some("3,,4")), (3, 102_400, 4));
    }

    #[test]
    fn test_validate_params_rejects_out_of_range() {
        assert!(validate_params(ALGO_PBKDF2_SHA256, None).is_ok());
        assert!(validate_params(ALGO_PBKDF2_SHA256, Some("5000")).is_ok());
        // The desktop's buggy value still validates.
        assert!(validate_params(ALGO_PBKDF2_SHA256, Some("pbkdf2_sha256")).is_ok());
        assert!(validate_params(ALGO_PBKDF2_SHA256, Some("999999999999")).is_err());
        assert!(validate_params(ALGO_ARGON2ID, None).is_ok());
        assert!(validate_params(ALGO_ARGON2ID, Some("2,4096,1")).is_ok());
        assert!(validate_params(ALGO_ARGON2ID, Some("2,4294967295,8")).is_err());
        assert!(validate_params(ALGO_ARGON2ID, Some("9999999,4096,1")).is_err());
        assert!(validate_params(ALGO_ARGON2ID, Some("2,4096,9999999")).is_err());
        // m_cost must cover 8 blocks per lane.
        assert!(validate_params(ALGO_ARGON2ID, Some("2,8,8")).is_err());
        // Neither maximum alone is enough: 1 GiB × 64 passes satisfies both and
        // still costs minutes per password check.
        assert!(validate_params(ALGO_ARGON2ID, Some("2,1048576,1")).is_ok());
        assert!(validate_params(ALGO_ARGON2ID, Some("64,1048576,1")).is_err());
        assert!(validate_params(ALGO_ARGON2ID, Some("16,65536,1")).is_ok());
        assert!(validate_params(ALGO_ARGON2ID, Some("64,65536,1")).is_err());
        assert!(validate_params("PBKDF2", None).is_err());
        assert!(validate_params("scrypt", None).is_err());
    }

    /// The salt the KDF sees for `enc_version <= 2` is the 8 fixed bytes
    /// followed by 24 zeros — not the 8 bytes on their own, and not the
    /// (unrelated) 8-byte salt `generate_magic` uses.
    #[test]
    fn test_pwd_hash_salt_is_the_8_magic_bytes_plus_24_zeros() {
        let salt = pwd_hash_salt(2, "").unwrap();
        assert_eq!(&salt[..8], &MAGIC_SALT);
        assert!(
            salt[8..].iter().all(|b| *b == 0),
            "the remainder must stay zeroed"
        );
        // Same for enc_version 1 and for a v4-looking salt on a v2 library.
        assert_eq!(pwd_hash_salt(1, "").unwrap(), salt);
        assert_eq!(pwd_hash_salt(2, SALT_A).unwrap(), salt);

        let v4 = pwd_hash_salt(4, SALT_A).unwrap();
        assert_eq!(v4, [0xaa; 32]);
        assert!(pwd_hash_salt(4, "").is_err());
        assert!(pwd_hash_salt(4, "zz").is_err());
    }

    /// Guards the mistake this module's docs warn about: hashing with only the
    /// 8 magic bytes yields a different (wrong) value.
    #[test]
    fn test_v2_salt_is_not_the_bare_eight_bytes() {
        let input = format!("{REPO_ID}{PASSWORD}");
        let mut key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(input.as_bytes(), &MAGIC_SALT, 1000, &mut key);
        assert_ne!(
            hex::encode(key),
            "5358432032aca60b6e828950082a1afa3dff0cce09fd11ac1d36d0b889fac294"
        );
    }

    /// Golden vectors from OpenSSL 3.5's `ARGON2ID` KDF (Argon2 v1.3, no
    /// secret/ad — the same shape as `argon2id_hash_raw`), e.g.
    /// `openssl kdf -keylen 32 -kdfopt pass:... -kdfopt hexsalt:... \
    ///  -kdfopt iter:2 -kdfopt memcost:102400 -kdfopt lanes:8 -kdfopt threads:1 ARGON2ID`.
    /// OpenSSL derives the tag from `lanes`; `threads` only sizes its pool.
    #[test]
    fn test_derive_pwd_hash_argon2id_matches_openssl() {
        let cases: &[(i32, &str, &str, &str)] = &[
            (
                2,
                "",
                "2,102400,8",
                "bf88876cd14d76401050aa33c7e92af0eccfc77115d65125776cb70ba4d45999",
            ),
            (
                2,
                "",
                "2,4096,1",
                "0a0bdf9253a6c0d63929f571a1cfaaf83d7a29068aea1fd4ee10f1726ee9a3cd",
            ),
            (
                2,
                "",
                "2,4096,2",
                "a19057dbe9ef64fc259fbb88f2c74b11ac70cdeb56c8ac9108e321f44e39a1f2",
            ),
            (
                4,
                SALT_A,
                "2,4096,1",
                "7efa1a3b0c0713da4cacfc5aacc4cd450659023f55156a26920dae232313bfef",
            ),
            (
                4,
                SALT_B,
                "3,65536,4",
                "3779532fade8bf749d30d749f278ca61c800758704ea40b367bc5012212e9d3f",
            ),
        ];
        for (enc_version, salt, params, expected) in cases {
            let got = derive_pwd_hash(
                REPO_ID,
                PASSWORD,
                *enc_version,
                salt,
                ALGO_ARGON2ID,
                Some(params),
            )
            .unwrap();
            assert_eq!(got, *expected, "enc={enc_version} params={params}");
        }
    }

    /// `params_str == NULL` must mean the documented defaults, not the crate's.
    #[test]
    fn test_argon2id_default_params_are_seafiles_not_the_crates() {
        let explicit =
            derive_pwd_hash(REPO_ID, PASSWORD, 2, "", ALGO_ARGON2ID, Some("2,102400,8")).unwrap();
        let implicit = derive_pwd_hash(REPO_ID, PASSWORD, 2, "", ALGO_ARGON2ID, None).unwrap();
        assert_eq!(explicit, implicit);
    }

    #[test]
    fn test_verify_pwd_hash() {
        for algo in [ALGO_PBKDF2_SHA256, ALGO_ARGON2ID] {
            let params = if algo == ALGO_PBKDF2_SHA256 {
                Some("2000")
            } else {
                Some("2,4096,1")
            };
            let hash = derive_pwd_hash(REPO_ID, PASSWORD, 2, "", algo, params).unwrap();
            assert!(verify_pwd_hash(
                REPO_ID, PASSWORD, 2, "", &hash, algo, params
            ));
            assert!(!verify_pwd_hash(
                REPO_ID, "wrong", 2, "", &hash, algo, params
            ));
            assert!(!verify_pwd_hash(
                "other-repo",
                PASSWORD,
                2,
                "",
                &hash,
                algo,
                params
            ));
            assert!(!verify_pwd_hash(
                REPO_ID, PASSWORD, 4, SALT_A, &hash, algo, params
            ));
        }
    }

    #[test]
    fn test_verify_pwd_hash_is_case_and_parameter_sensitive() {
        let hash = derive_pwd_hash(REPO_ID, PASSWORD, 2, "", ALGO_PBKDF2_SHA256, None).unwrap();
        assert!(!verify_pwd_hash(
            REPO_ID,
            PASSWORD,
            2,
            "",
            &hash.to_uppercase(),
            ALGO_PBKDF2_SHA256,
            None
        ));
        assert!(!verify_pwd_hash(
            REPO_ID,
            PASSWORD,
            2,
            "",
            &hash,
            ALGO_PBKDF2_SHA256,
            Some("2000")
        ));
    }

    /// Unsupported algorithms and out-of-range parameters fail closed instead of
    /// running an unbounded derivation.
    #[test]
    fn test_verify_pwd_hash_fails_closed() {
        let hash = derive_pwd_hash(REPO_ID, PASSWORD, 2, "", ALGO_PBKDF2_SHA256, None).unwrap();
        assert!(!verify_pwd_hash(
            REPO_ID, PASSWORD, 2, "", &hash, "scrypt", None
        ));
        assert!(!verify_pwd_hash(
            REPO_ID,
            PASSWORD,
            2,
            "",
            &hash,
            ALGO_ARGON2ID,
            Some("2,4294967295,8")
        ));
    }

    #[test]
    fn test_derive_pwd_hash_rejects_unknown_algo_and_bad_salt() {
        assert!(matches!(
            derive_pwd_hash(REPO_ID, PASSWORD, 2, "", "PBKDF2", None),
            Err(CryptoError::UnsupportedAlgo(_))
        ));
        assert!(matches!(
            derive_pwd_hash(REPO_ID, PASSWORD, 4, "not-hex", ALGO_PBKDF2_SHA256, None),
            Err(CryptoError::InvalidSalt(_))
        ));
    }
}
