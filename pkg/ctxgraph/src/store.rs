//! SQLite storage backend for ctxgraph.

use crate::entry::{self, Entry};
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open or create a ctxgraph database at the given path.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::Sqlite)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Create an in-memory store (for testing).
    pub fn in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS entries (
                    hash      TEXT PRIMARY KEY,
                    agent_id  TEXT NOT NULL,
                    key       TEXT NOT NULL,
                    value     TEXT NOT NULL,
                    timestamp INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_entries_key ON entries(key);
                CREATE INDEX IF NOT EXISTS idx_entries_agent ON entries(agent_id);
                CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);",
            )
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    /// Write an entry. Returns the content-address hash.
    /// Idempotent: writing the same content twice is a no-op.
    pub fn write(
        &self,
        agent_id: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<String, StoreError> {
        let hash = entry::compute_hash(agent_id, key, value);
        let timestamp = current_timestamp();
        let value_json = serde_json::to_string(value)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;

        self.conn
            .execute(
                "INSERT OR IGNORE INTO entries (hash, agent_id, key, value, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![hash, agent_id, key, value_json, timestamp],
            )
            .map_err(StoreError::Sqlite)?;

        Ok(hash)
    }

    /// Read an entry by its content-address hash.
    pub fn read(&self, hash: &str) -> Result<Option<Entry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT hash, agent_id, key, value, timestamp FROM entries WHERE hash = ?1",
            )
            .map_err(StoreError::Sqlite)?;

        let entry = stmt
            .query_row(params![hash], |row| {
                let value_str: String = row.get(3)?;
                Ok(Entry {
                    hash: row.get(0)?,
                    agent_id: row.get(1)?,
                    key: row.get(2)?,
                    value: serde_json::from_str(&value_str).unwrap_or_default(),
                    timestamp: row.get(4)?,
                })
            })
            .optional()
            .map_err(StoreError::Sqlite)?;

        Ok(entry)
    }

    /// Query entries by key prefix.
    pub fn query_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<Entry>, StoreError> {
        let pattern = format!("{prefix}%");
        let mut stmt = self
            .conn
            .prepare(
                "SELECT hash, agent_id, key, value, timestamp
                 FROM entries WHERE key LIKE ?1
                 ORDER BY timestamp DESC",
            )
            .map_err(StoreError::Sqlite)?;

        let entries = stmt
            .query_map(params![pattern], |row| {
                let value_str: String = row.get(3)?;
                Ok(Entry {
                    hash: row.get(0)?,
                    agent_id: row.get(1)?,
                    key: row.get(2)?,
                    value: serde_json::from_str(&value_str).unwrap_or_default(),
                    timestamp: row.get(4)?,
                })
            })
            .map_err(StoreError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Sqlite)?;

        Ok(entries)
    }

    /// Get the history of a specific key, ordered by timestamp descending.
    pub fn history(&self, key: &str) -> Result<Vec<Entry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT hash, agent_id, key, value, timestamp
                 FROM entries WHERE key = ?1
                 ORDER BY timestamp DESC",
            )
            .map_err(StoreError::Sqlite)?;

        let entries = stmt
            .query_map(params![key], |row| {
                let value_str: String = row.get(3)?;
                Ok(Entry {
                    hash: row.get(0)?,
                    agent_id: row.get(1)?,
                    key: row.get(2)?,
                    value: serde_json::from_str(&value_str).unwrap_or_default(),
                    timestamp: row.get(4)?,
                })
            })
            .map_err(StoreError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Sqlite)?;

        Ok(entries)
    }

    /// Get all entries by a specific agent.
    pub fn by_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Entry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT hash, agent_id, key, value, timestamp
                 FROM entries WHERE agent_id = ?1
                 ORDER BY timestamp DESC",
            )
            .map_err(StoreError::Sqlite)?;

        let entries = stmt
            .query_map(params![agent_id], |row| {
                let value_str: String = row.get(3)?;
                Ok(Entry {
                    hash: row.get(0)?,
                    agent_id: row.get(1)?,
                    key: row.get(2)?,
                    value: serde_json::from_str(&value_str).unwrap_or_default(),
                    timestamp: row.get(4)?,
                })
            })
            .map_err(StoreError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Sqlite)?;

        Ok(entries)
    }
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// Needed for optional() on query_row
use rusqlite::OptionalExtension;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Serialization(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Serialization(e) => write!(f, "serialization error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_store() -> Store {
        Store::in_memory().unwrap()
    }

    #[test]
    fn write_and_read() {
        let store = test_store();
        let hash = store
            .write("agent-1", "research.result", &json!({"found": true}))
            .unwrap();

        let entry = store.read(&hash).unwrap().unwrap();
        assert_eq!(entry.agent_id, "agent-1");
        assert_eq!(entry.key, "research.result");
        assert_eq!(entry.value, json!({"found": true}));
    }

    #[test]
    fn write_is_idempotent() {
        let store = test_store();
        let h1 = store.write("a", "k", &json!("v")).unwrap();
        let h2 = store.write("a", "k", &json!("v")).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let store = test_store();
        let entry = store.read("nonexistent-hash").unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn query_by_prefix() {
        let store = test_store();
        store.write("a", "research.topic", &json!("AI")).unwrap();
        store
            .write("a", "research.result", &json!("found"))
            .unwrap();
        store.write("a", "other.key", &json!("nope")).unwrap();

        let results = store.query_by_prefix("research.").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.key.starts_with("research.")));
    }

    #[test]
    fn history_returns_entries_for_key() {
        let store = test_store();
        store.write("agent-1", "status", &json!("starting")).unwrap();
        store.write("agent-2", "status", &json!("running")).unwrap();

        let history = store.history("status").unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn by_agent_filters_correctly() {
        let store = test_store();
        store.write("agent-1", "key-a", &json!(1)).unwrap();
        store.write("agent-1", "key-b", &json!(2)).unwrap();
        store.write("agent-2", "key-c", &json!(3)).unwrap();

        let entries = store.by_agent("agent-1").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.agent_id == "agent-1"));
    }
}
