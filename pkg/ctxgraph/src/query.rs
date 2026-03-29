//! Query interface for ctxgraph.
//!
//! Provides a high-level API over the store for agent consumption.
//! In v0.1, this is used directly. In future versions, this will be
//! exposed over a Unix socket / virtio-vsock daemon.

use crate::entry::Entry;
use crate::store::{Store, StoreError};

/// High-level query operations on ctxgraph.
pub struct Graph {
    store: Store,
}

impl Graph {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Write an entry. Returns the content-address hash.
    pub fn write(
        &self,
        agent_id: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<String, StoreError> {
        self.store.write(agent_id, key, value)
    }

    /// Read an entry by hash.
    pub fn read(&self, hash: &str) -> Result<Option<Entry>, StoreError> {
        self.store.read(hash)
    }

    /// Find entries matching a key prefix.
    pub fn query(&self, key_prefix: &str) -> Result<Vec<Entry>, StoreError> {
        self.store.query_by_prefix(key_prefix)
    }

    /// Get full history of a key.
    pub fn history(&self, key: &str) -> Result<Vec<Entry>, StoreError> {
        self.store.history(key)
    }

    /// Get all entries from a specific agent.
    pub fn by_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Entry>, StoreError> {
        self.store.by_agent(agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_graph() -> Graph {
        let store = Store::in_memory().unwrap();
        Graph::new(store)
    }

    #[test]
    fn graph_write_and_read() {
        let g = test_graph();
        let hash = g.write("test-agent", "key", &json!("value")).unwrap();
        let entry = g.read(&hash).unwrap().unwrap();
        assert_eq!(entry.agent_id, "test-agent");
    }

    #[test]
    fn graph_cross_agent_visibility() {
        let g = test_graph();

        // Agent A writes
        let hash = g
            .write("agent-a", "shared.data", &json!({"x": 1}))
            .unwrap();

        // Agent B reads
        let entry = g.read(&hash).unwrap().unwrap();
        assert_eq!(entry.agent_id, "agent-a");
        assert_eq!(entry.value, json!({"x": 1}));
    }
}
