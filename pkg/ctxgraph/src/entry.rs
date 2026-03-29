//! Content-addressed entry type for ctxgraph.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A single entry in the ctxgraph store.
/// Content-addressed: the hash is derived from (agent_id, key, value).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub hash: String,
    pub agent_id: String,
    pub key: String,
    pub value: serde_json::Value,
    pub timestamp: i64,
}

/// Compute the content-address hash for an entry.
/// Hash = SHA-256(agent_id || "\0" || key || "\0" || canonical_json(value))
pub fn compute_hash(
    agent_id: &str,
    key: &str,
    value: &serde_json::Value,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(agent_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    // Canonical JSON: sorted keys, no whitespace
    let json_bytes = serde_json::to_vec(value).unwrap_or_default();
    hasher.update(&json_bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_is_deterministic() {
        let h1 = compute_hash("agent-1", "key", &json!("value"));
        let h2 = compute_hash("agent-1", "key", &json!("value"));
        assert_eq!(h1, h2);
    }

    #[test]
    fn different_agents_produce_different_hashes() {
        let h1 = compute_hash("agent-1", "key", &json!("value"));
        let h2 = compute_hash("agent-2", "key", &json!("value"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn different_keys_produce_different_hashes() {
        let h1 = compute_hash("agent-1", "key-a", &json!("value"));
        let h2 = compute_hash("agent-1", "key-b", &json!("value"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let hash = compute_hash("a", "b", &json!(null));
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
