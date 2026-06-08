use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
use rand::Rng;
use sha2::Sha256;

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

const MAGIC_SALT: [u8; 8] = [0xda, 0x90, 0x45, 0xc3, 0x06, 0xc7, 0xcc, 0x26];

pub fn generate_random_key(password: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut enc_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), password.as_bytes(), 1000, &mut enc_key);

    let mut enc_iv = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<Sha256>(&enc_key, &MAGIC_SALT, 10, &mut enc_iv);

    let mut secret_key = [0u8; 48];
    rand::thread_rng().fill(&mut secret_key[..]);

    let ciphertext = Aes256CbcEnc::new(&enc_key.into(), &enc_iv.into())
        .encrypt_padded_vec_mut::<NoPadding>(&secret_key);

    Ok(hex::encode(&ciphertext))
}

pub fn decrypt_random_key(
    random_key: &str,
    password: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let ciphertext = hex::decode(random_key)?;

    let mut enc_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), password.as_bytes(), 1000, &mut enc_key);

    let mut enc_iv = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<Sha256>(&enc_key, &MAGIC_SALT, 10, &mut enc_iv);

    let plaintext = Aes256CbcDec::new(&enc_key.into(), &enc_iv.into())
        .decrypt_padded_vec_mut::<NoPadding>(&ciphertext)
        .map_err(|e| format!("decrypt error: {}", e))?;

    Ok(plaintext)
}

pub fn encrypt_block(data: &[u8], file_key: &[u8], _file_iv: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; 16];
    rand::thread_rng().fill(&mut iv[..]);

    let ciphertext =
        Aes256CbcEnc::new(file_key.into(), &iv.into()).encrypt_padded_vec_mut::<NoPadding>(data);

    let mut result = Vec::with_capacity(16 + ciphertext.len());
    result.extend_from_slice(&iv);
    result.extend_from_slice(&ciphertext);
    result
}

pub fn decrypt_block(
    encrypted: &[u8],
    file_key: &[u8],
    _file_iv: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if encrypted.len() < 16 {
        return Err("encrypted block too short".into());
    }

    let iv = &encrypted[..16];
    let ciphertext = &encrypted[16..];

    let plaintext = Aes256CbcDec::new(file_key.into(), iv.into())
        .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
        .map_err(|e| format!("decrypt error: {}", e))?;

    Ok(plaintext)
}
