use sha2::Sha256;

const MAGIC_SALT: [u8; 8] = [0xda, 0x90, 0x45, 0xc3, 0x06, 0xc7, 0xcc, 0x26];

pub fn compute_magic(repo_id: &str, password: &str) -> String {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), repo_id.as_bytes(), 1000, &mut key);

    let mut iv = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<Sha256>(&key, &MAGIC_SALT, 10, &mut iv);

    format!("{}{}", hex::encode(key), hex::encode(iv))
}

pub fn extract_key(magic: &str) -> Option<Vec<u8>> {
    if magic.len() != 128 {
        return None;
    }
    hex::decode(&magic[..64]).ok()
}

pub fn extract_iv(magic: &str) -> Option<Vec<u8>> {
    if magic.len() != 128 {
        return None;
    }
    hex::decode(&magic[64..]).ok()
}
