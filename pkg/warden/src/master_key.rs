//! Master key resolution for the Warden vault.
//!
//! Resolution order:
//! 1. WARDEN_MASTER_KEY env var (hex-encoded 32 bytes)
//! 2. /vault/master.key file (raw 32 bytes)
//! 3. WARDEN_PASSPHRASE env var (derived via PBKDF2 with vault salt)

use crate::crypto;
use std::path::Path;

const MASTER_KEY_PATH: &str = "/vault/master.key";

/// Resolve the master key from available sources.
///
/// The `salt` is required only for passphrase-based derivation. Pass `None`
/// if no vault file exists yet (a new salt will be generated).
pub fn resolve(salt: Option<&[u8; 16]>) -> Result<[u8; 32], MasterKeyError> {
    // 1. Hex-encoded env var
    if let Ok(hex_key) = std::env::var("WARDEN_MASTER_KEY") {
        let bytes = hex::decode(hex_key.trim()).map_err(|_| MasterKeyError::InvalidHex)?;
        if bytes.len() != 32 {
            return Err(MasterKeyError::WrongLength(bytes.len()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }

    // 2. Key file on disk
    let key_path = Path::new(MASTER_KEY_PATH);
    if key_path.exists() {
        let bytes = std::fs::read(key_path).map_err(MasterKeyError::ReadFailed)?;
        if bytes.len() != 32 {
            return Err(MasterKeyError::WrongLength(bytes.len()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }

    // 3. Passphrase (requires salt for derivation)
    if let Ok(passphrase) = std::env::var("WARDEN_PASSPHRASE") {
        if passphrase.is_empty() {
            return Err(MasterKeyError::EmptyPassphrase);
        }
        let salt = salt.ok_or(MasterKeyError::NoSaltForPassphrase)?;
        return Ok(crypto::derive_key(&passphrase, salt));
    }

    Err(MasterKeyError::NoKeySource)
}

#[derive(Debug)]
pub enum MasterKeyError {
    InvalidHex,
    WrongLength(usize),
    ReadFailed(std::io::Error),
    EmptyPassphrase,
    NoSaltForPassphrase,
    NoKeySource,
}

impl std::fmt::Display for MasterKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHex => write!(f, "WARDEN_MASTER_KEY is not valid hex"),
            Self::WrongLength(n) => write!(f, "master key must be 32 bytes, got {n}"),
            Self::ReadFailed(e) => write!(f, "failed to read {MASTER_KEY_PATH}: {e}"),
            Self::EmptyPassphrase => write!(f, "WARDEN_PASSPHRASE is empty"),
            Self::NoSaltForPassphrase => {
                write!(f, "passphrase derivation requires a salt (no existing vault)")
            }
            Self::NoKeySource => write!(
                f,
                "no master key found. Set WARDEN_MASTER_KEY, WARDEN_PASSPHRASE, or create {MASTER_KEY_PATH}"
            ),
        }
    }
}

impl std::error::Error for MasterKeyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_env_var() {
        let key_hex = "ab".repeat(32);
        unsafe { std::env::set_var("WARDEN_MASTER_KEY", &key_hex) };
        let key = resolve(None).unwrap();
        assert_eq!(key[0], 0xAB);
        assert_eq!(key[31], 0xAB);
        unsafe { std::env::remove_var("WARDEN_MASTER_KEY") };
    }

    #[test]
    fn invalid_hex_rejected() {
        unsafe { std::env::set_var("WARDEN_MASTER_KEY", "not-hex!") };
        let result = resolve(None);
        assert!(result.is_err());
        unsafe { std::env::remove_var("WARDEN_MASTER_KEY") };
    }

    #[test]
    fn wrong_length_rejected() {
        unsafe { std::env::set_var("WARDEN_MASTER_KEY", "aabb") };
        let result = resolve(None);
        assert!(matches!(result, Err(MasterKeyError::WrongLength(2))));
        unsafe { std::env::remove_var("WARDEN_MASTER_KEY") };
    }

    #[test]
    fn passphrase_derivation() {
        unsafe { std::env::remove_var("WARDEN_MASTER_KEY") };
        unsafe { std::env::set_var("WARDEN_PASSPHRASE", "test-passphrase") };
        let salt = [42u8; 16];
        let key = resolve(Some(&salt)).unwrap();
        assert_eq!(key.len(), 32);
        // Deterministic
        let key2 = resolve(Some(&salt)).unwrap();
        assert_eq!(key, key2);
        unsafe { std::env::remove_var("WARDEN_PASSPHRASE") };
    }

    #[test]
    fn no_key_source_fails() {
        unsafe { std::env::remove_var("WARDEN_MASTER_KEY") };
        unsafe { std::env::remove_var("WARDEN_PASSPHRASE") };
        let result = resolve(None);
        assert!(matches!(result, Err(MasterKeyError::NoKeySource)));
    }
}
