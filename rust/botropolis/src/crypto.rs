//! Encryption for provider keys stored at rest.
//!
//! Team provider keys (BYOK) are encrypted with AES-256-GCM under
//! a platform master key from `BOTROPOLIS_MASTER_KEY`. The master
//! key is either 64 hex characters (32 bytes) or any passphrase,
//! in which case its SHA-256 is the key. A missing master key is
//! an error, never a reason to store plaintext.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// The environment variable holding the platform master key.
pub const MASTER_KEY_ENV: &str = "BOTROPOLIS_MASTER_KEY";

/// Read the master key. Err when it is not set.
pub fn master_key() -> Result<[u8; 32], String> {
    let raw = std::env::var(MASTER_KEY_ENV).map_err(|_| format!("{MASTER_KEY_ENV} is not set"))?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(format!("{MASTER_KEY_ENV} is not set"));
    }
    if raw.len() == 64 {
        if let Ok(bytes) = hex_decode(raw) {
            if bytes.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return Ok(key);
            }
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    Ok(key)
}

fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    let text = text.as_bytes();
    if !text.len().is_multiple_of(2) {
        return Err("odd-length hex".to_string());
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.chunks(2) {
        let hi = (pair[0] as char).to_digit(16).ok_or("bad hex")?;
        let lo = (pair[1] as char).to_digit(16).ok_or("bad hex")?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

/// Encrypt plaintext. Returns (nonce, ciphertext).
pub fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok((nonce_bytes.to_vec(), ciphertext))
}

/// Decrypt. Err when the key is wrong or the data is corrupt.
pub fn decrypt(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8]) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let nonce = Nonce::from_slice(nonce);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "could not decrypt provider key".to_string())?;
    String::from_utf8(plaintext).map_err(|_| "provider key is not valid text".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_env;

    #[test]
    fn roundtrip_and_wrong_key_fails() {
        let key = [7u8; 32];
        let (nonce, ciphertext) = encrypt(&key, "sk-secret-value").unwrap();
        assert_eq!(
            decrypt(&key, &nonce, &ciphertext).unwrap(),
            "sk-secret-value"
        );
        assert!(decrypt(&[8u8; 32], &nonce, &ciphertext).is_err());
        let text = String::from_utf8_lossy(&ciphertext);
        assert!(!text.contains("sk-secret-value"));
    }

    #[test]
    fn master_key_from_hex_and_passphrase() {
        let _guard = lock_env();
        std::env::set_var(MASTER_KEY_ENV, "00".repeat(32));
        assert_eq!(master_key().unwrap(), [0u8; 32]);
        std::env::set_var(MASTER_KEY_ENV, "a passphrase for tests");
        let first = master_key().unwrap();
        assert_eq!(master_key().unwrap(), first);
        assert_ne!(first, [0u8; 32]);
        std::env::remove_var(MASTER_KEY_ENV);
        assert!(master_key().is_err());
    }
}
