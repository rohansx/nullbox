//! Vault — encrypted secret storage.
//!
//! Secrets are stored as an encrypted JSON file. Each write generates
//! a fresh nonce. The file format is:
//!
//! ```json
//! {"salt": "<hex>", "nonce": "<hex>", "ciphertext": "<hex>"}
//! ```
//!
//! The plaintext is a JSON object: `{"KEY_NAME": "secret_value", ...}`

use crate::crypto;
use crate::master_key;
use std::collections::HashMap;
use std::path::Path;

pub const VAULT_PATH: &str = "/vault/secrets.enc";

/// In-memory vault holding decrypted secrets and the master key.
pub struct Vault {
    secrets: HashMap<String, String>,
    master_key: [u8; 32],
    salt: [u8; 16],
}

/// On-disk vault file format.
#[derive(serde::Serialize, serde::Deserialize)]
struct VaultFile {
    salt: String,
    nonce: String,
    ciphertext: String,
}

impl Vault {
    /// Load the vault from disk, or create a new empty vault.
    ///
    /// Resolves the master key from environment/file sources.
    pub fn load(path: &Path) -> Result<Self, VaultError> {
        if !path.exists() {
            // New vault — generate salt, resolve key (may use passphrase)
            let salt = crypto::generate_salt().map_err(VaultError::Crypto)?;
            let master_key =
                master_key::resolve(Some(&salt)).map_err(VaultError::MasterKey)?;

            return Ok(Self {
                secrets: HashMap::new(),
                master_key,
                salt,
            });
        }

        let content = std::fs::read_to_string(path).map_err(VaultError::Io)?;
        let file: VaultFile =
            serde_json::from_str(&content).map_err(VaultError::Parse)?;

        let salt_bytes = hex::decode(&file.salt).map_err(|_| VaultError::CorruptedFile("invalid salt hex"))?;
        let nonce_bytes = hex::decode(&file.nonce).map_err(|_| VaultError::CorruptedFile("invalid nonce hex"))?;
        let ciphertext = hex::decode(&file.ciphertext).map_err(|_| VaultError::CorruptedFile("invalid ciphertext hex"))?;

        if salt_bytes.len() != 16 {
            return Err(VaultError::CorruptedFile("salt must be 16 bytes"));
        }
        if nonce_bytes.len() != 12 {
            return Err(VaultError::CorruptedFile("nonce must be 12 bytes"));
        }

        let mut salt = [0u8; 16];
        salt.copy_from_slice(&salt_bytes);
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&nonce_bytes);

        let master_key =
            master_key::resolve(Some(&salt)).map_err(VaultError::MasterKey)?;

        let plaintext = crypto::decrypt(&ciphertext, &master_key, &nonce)
            .map_err(VaultError::Crypto)?;

        let secrets: HashMap<String, String> =
            serde_json::from_slice(&plaintext).map_err(VaultError::Parse)?;

        Ok(Self {
            secrets,
            master_key,
            salt,
        })
    }

    /// Save the vault to disk. Generates a fresh nonce and writes atomically.
    pub fn save(&self, path: &Path) -> Result<(), VaultError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(VaultError::Io)?;
        }

        let plaintext = serde_json::to_vec(&self.secrets).map_err(VaultError::Parse)?;
        let (ciphertext, nonce) =
            crypto::encrypt(&plaintext, &self.master_key).map_err(VaultError::Crypto)?;

        let file = VaultFile {
            salt: hex::encode(self.salt),
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
        };

        let json = serde_json::to_string_pretty(&file).map_err(VaultError::Parse)?;

        // Atomic write: tmp + rename
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, json).map_err(VaultError::Io)?;
        std::fs::rename(&tmp_path, path).map_err(VaultError::Io)?;

        Ok(())
    }

    /// Get secrets filtered to only the requested keys.
    /// Missing keys are silently omitted.
    pub fn get(&self, keys: &[String]) -> HashMap<String, String> {
        keys.iter()
            .filter_map(|k| self.secrets.get(k).map(|v| (k.clone(), v.clone())))
            .collect()
    }

    /// Set a secret. Call `save()` after to persist.
    pub fn set(&mut self, key: &str, value: &str) {
        self.secrets.insert(key.to_string(), value.to_string());
    }

    /// Delete a secret. Returns true if the key existed.
    pub fn delete(&mut self, key: &str) -> bool {
        self.secrets.remove(key).is_some()
    }

    /// List all secret key names (never values).
    pub fn list_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.secrets.keys().cloned().collect();
        keys.sort();
        keys
    }
}

#[derive(Debug)]
pub enum VaultError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    Crypto(crypto::CryptoError),
    MasterKey(master_key::MasterKeyError),
    CorruptedFile(&'static str),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "vault I/O error: {e}"),
            Self::Parse(e) => write!(f, "vault parse error: {e}"),
            Self::Crypto(e) => write!(f, "vault crypto error: {e}"),
            Self::MasterKey(e) => write!(f, "master key error: {e}"),
            Self::CorruptedFile(msg) => write!(f, "corrupted vault file: {msg}"),
        }
    }
}

impl std::error::Error for VaultError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_key() {
        unsafe { std::env::set_var("WARDEN_MASTER_KEY", "ab".repeat(32)) };
    }

    fn cleanup_key() {
        unsafe { std::env::remove_var("WARDEN_MASTER_KEY") };
    }

    #[test]
    fn new_vault_is_empty() {
        setup_key();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().with_extension("enc");
        // Don't create the file — Vault::load should return empty
        let vault = Vault::load(&path).unwrap();
        assert!(vault.list_keys().is_empty());
        cleanup_key();
    }

    #[test]
    fn set_save_load_roundtrip() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        // Create and populate
        let mut vault = Vault::load(&path).unwrap();
        vault.set("OPENAI_KEY", "sk-test123");
        vault.set("EXA_KEY", "exa-456");
        vault.save(&path).unwrap();

        // Reload from disk
        let vault2 = Vault::load(&path).unwrap();
        let all = vault2.get(&["OPENAI_KEY".into(), "EXA_KEY".into()]);
        assert_eq!(all["OPENAI_KEY"], "sk-test123");
        assert_eq!(all["EXA_KEY"], "exa-456");
        cleanup_key();
    }

    #[test]
    fn get_filters_to_requested_keys() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        let mut vault = Vault::load(&path).unwrap();
        vault.set("KEY_A", "val_a");
        vault.set("KEY_B", "val_b");
        vault.set("KEY_C", "val_c");

        let filtered = vault.get(&["KEY_A".into(), "KEY_C".into()]);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains_key("KEY_A"));
        assert!(filtered.contains_key("KEY_C"));
        assert!(!filtered.contains_key("KEY_B"));
        cleanup_key();
    }

    #[test]
    fn get_missing_keys_omitted() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        let mut vault = Vault::load(&path).unwrap();
        vault.set("EXISTS", "yes");

        let filtered = vault.get(&["EXISTS".into(), "MISSING".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered["EXISTS"], "yes");
        cleanup_key();
    }

    #[test]
    fn delete_removes_key() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        let mut vault = Vault::load(&path).unwrap();
        vault.set("TO_DELETE", "val");
        assert!(vault.delete("TO_DELETE"));
        assert!(!vault.delete("TO_DELETE")); // already gone
        assert!(vault.list_keys().is_empty());
        cleanup_key();
    }

    #[test]
    fn list_keys_sorted() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        let mut vault = Vault::load(&path).unwrap();
        vault.set("ZEBRA", "z");
        vault.set("ALPHA", "a");
        vault.set("MIDDLE", "m");

        let keys = vault.list_keys();
        assert_eq!(keys, vec!["ALPHA", "MIDDLE", "ZEBRA"]);
        cleanup_key();
    }

    #[test]
    fn vault_file_is_encrypted() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        let mut vault = Vault::load(&path).unwrap();
        vault.set("SECRET", "super-secret-value");
        vault.save(&path).unwrap();

        // Read raw file — should NOT contain plaintext
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("super-secret-value"));
        assert!(raw.contains("ciphertext")); // JSON structure present
        cleanup_key();
    }

    #[test]
    fn re_encrypt_produces_different_ciphertext() {
        setup_key();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");

        let mut vault = Vault::load(&path).unwrap();
        vault.set("KEY", "value");
        vault.save(&path).unwrap();
        let raw1 = std::fs::read_to_string(&path).unwrap();

        vault.save(&path).unwrap();
        let raw2 = std::fs::read_to_string(&path).unwrap();

        // Different nonces → different ciphertext
        assert_ne!(raw1, raw2);
        cleanup_key();
    }
}
