//! AES-256-GCM encryption for the secret vault.
//!
//! Uses ring for all cryptographic operations. Each vault write
//! generates a fresh nonce to ensure ciphertext uniqueness.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};

/// Encrypt plaintext with AES-256-GCM.
///
/// Returns (ciphertext_with_tag, nonce). A fresh random nonce is generated
/// for each call — never reuse a nonce with the same key.
pub fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> Result<(Vec<u8>, [u8; 12]), CryptoError> {
    let rng = SystemRandom::new();

    let mut nonce_bytes = [0u8; 12];
    rng.fill(&mut nonce_bytes)
        .map_err(|_| CryptoError::RandomFailed)?;

    let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::KeyInvalid)?;
    let sealing_key = LessSafeKey::new(unbound);

    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let mut in_out = plaintext.to_vec();
    sealing_key
        .seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::EncryptFailed)?;

    Ok((in_out, nonce_bytes))
}

/// Decrypt ciphertext (with appended tag) using AES-256-GCM.
pub fn decrypt(
    ciphertext: &[u8],
    key: &[u8; 32],
    nonce: &[u8; 12],
) -> Result<Vec<u8>, CryptoError> {
    let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::KeyInvalid)?;
    let opening_key = LessSafeKey::new(unbound);

    let nonce = Nonce::assume_unique_for_key(*nonce);

    let mut in_out = ciphertext.to_vec();
    let plaintext = opening_key
        .open_in_place(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| CryptoError::DecryptFailed)?;

    Ok(plaintext.to_vec())
}

/// Generate a random 16-byte salt for key derivation.
pub fn generate_salt() -> Result<[u8; 16], CryptoError> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt)
        .map_err(|_| CryptoError::RandomFailed)?;
    Ok(salt)
}

/// Derive a 256-bit key from a passphrase and salt using PBKDF2-HMAC-SHA256.
///
/// Uses 600,000 iterations per OWASP recommendations.
pub fn derive_key(passphrase: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut key = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(600_000).unwrap(),
        salt,
        passphrase.as_bytes(),
        &mut key,
    );
    key
}

#[derive(Debug)]
pub enum CryptoError {
    KeyInvalid,
    EncryptFailed,
    DecryptFailed,
    RandomFailed,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyInvalid => write!(f, "invalid key"),
            Self::EncryptFailed => write!(f, "encryption failed"),
            Self::DecryptFailed => write!(f, "decryption failed (wrong key or corrupted data)"),
            Self::RandomFailed => write!(f, "random number generation failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        key[0] = 0xAB;
        key[31] = 0xCD;
        key
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = test_key();
        let plaintext = b"secret-api-key-12345";

        let (ciphertext, nonce) = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&ciphertext, &key, &nonce).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let key = test_key();
        let mut wrong_key = test_key();
        wrong_key[0] = 0xFF;

        let (ciphertext, nonce) = encrypt(b"secret", &key).unwrap();
        let result = decrypt(&ciphertext, &wrong_key, &nonce);

        assert!(result.is_err());
    }

    #[test]
    fn unique_nonces_per_encrypt() {
        let key = test_key();
        let plaintext = b"same-input";

        let (ct1, nonce1) = encrypt(plaintext, &key).unwrap();
        let (ct2, nonce2) = encrypt(plaintext, &key).unwrap();

        assert_ne!(nonce1, nonce2);
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn derive_key_deterministic() {
        let salt = [1u8; 16];
        let k1 = derive_key("passphrase", &salt);
        let k2 = derive_key("passphrase", &salt);
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_key_different_salt() {
        let k1 = derive_key("passphrase", &[1u8; 16]);
        let k2 = derive_key("passphrase", &[2u8; 16]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn generate_salt_unique() {
        let s1 = generate_salt().unwrap();
        let s2 = generate_salt().unwrap();
        assert_ne!(s1, s2);
    }
}
