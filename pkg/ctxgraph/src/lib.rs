//! ctxgraph — Shared agent memory graph.
//!
//! Content-addressed key-value store backed by SQLite.
//! Agents write entries (key, value) and get back a SHA-256 hash.
//! Entries are immutable once written.

pub mod entry;
pub mod query;
pub mod store;
