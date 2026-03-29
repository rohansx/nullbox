# ctxgraph -- Content-Addressed Shared Agent Memory

## Overview

ctxgraph provides a shared, immutable memory store for NullBox agents. All entries are content-addressed using SHA-256, meaning identical data always produces the same hash and writing the same content twice is a no-op. Agents inside microVMs access ctxgraph over TCP (port 9100, routed via TSI), while host-side services use a Unix socket. The backing store is SQLite.

## Architecture

### Data Model

Each entry consists of:

| Field | Type | Description |
|-------|------|-------------|
| `hash` | string (64 hex chars) | Content address: SHA-256 of `agent_id \0 key \0 canonical_json(value)` |
| `agent_id` | string | The agent that wrote the entry |
| `key` | string | Hierarchical key (e.g., `research.result`) |
| `value` | JSON | Arbitrary JSON payload |
| `timestamp` | i64 | Unix timestamp at write time |

### Content Addressing

```
hash = SHA-256(agent_id || \0 || key || \0 || canonical_json(value))
```

This scheme means:
- Same agent writing the same key with the same value produces the same hash (idempotent).
- Different agents writing the same key/value produce different hashes (agent-scoped).
- The hash is deterministic and verifiable by any party.

### Cross-Agent Visibility

All entries are globally visible. Any agent can read any other agent's entries by hash, query by key prefix, or list entries by agent. This enables agents to share findings, coordinate work, and build on each other's output.

### Key Components

| File | Purpose |
|------|---------|
| `pkg/ctxgraph/src/main.rs` | Daemon: TCP + Unix socket listeners, request dispatch |
| `pkg/ctxgraph/src/entry.rs` | Entry struct, SHA-256 hash computation |
| `pkg/ctxgraph/src/store.rs` | SQLite backend: write, read, query, history, by_agent |
| `pkg/ctxgraph/src/query.rs` | High-level Graph API wrapping the store |
| `pkg/ctxgraph/src/lib.rs` | Library re-exports |

### Database Schema

```sql
CREATE TABLE entries (
    hash      TEXT PRIMARY KEY,
    agent_id  TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     TEXT NOT NULL,  -- JSON string
    timestamp INTEGER NOT NULL
);

CREATE INDEX idx_entries_key ON entries(key);
CREATE INDEX idx_entries_agent ON entries(agent_id);
CREATE INDEX idx_entries_timestamp ON entries(timestamp);
```

The `hash` primary key with `INSERT OR IGNORE` ensures idempotent writes.

### Dual Listener Architecture

- **TCP (port 9100)** -- For agents inside microVMs. Accessible via TSI at `127.0.0.1:9100` from within the guest. Each connection is handled in a dedicated thread.
- **Unix socket (`/run/ctxgraph.sock`)** -- For host-side services (nullctl, future orchestrators). Also threaded.

## Configuration

### Paths

| Path | Purpose |
|------|---------|
| `/var/lib/ctxgraph/db.sqlite` | SQLite database file |
| `/run/ctxgraph.sock` | Unix socket for host services |
| `0.0.0.0:9100` | TCP listener for agents |

### Agent Access

Agents receive `CTXGRAPH_PORT=9100` in their environment. They connect to `127.0.0.1:9100` which TSI routes to the host.

## API / Protocol

Newline-delimited JSON over TCP or Unix socket.

### write

Store a new entry. Returns the content-address hash. Idempotent.

```json
-> {"method": "write", "agent_id": "researcher", "key": "research.result", "value": {"found": true}}
<- {"ok": true, "hash": "a1b2c3d4..."}
```

### read

Retrieve an entry by its content-address hash.

```json
-> {"method": "read", "hash": "a1b2c3d4..."}
<- {"hash": "a1b2c3d4...", "agent_id": "researcher", "key": "research.result", "value": {"found": true}, "timestamp": 1711700000}
```

### query

Find entries by key prefix. Returns newest first.

```json
-> {"method": "query", "prefix": "research."}
<- {"entries": [
     {"hash": "...", "agent_id": "researcher", "key": "research.result", "value": {...}, "timestamp": 1711700000},
     {"hash": "...", "agent_id": "researcher", "key": "research.topic", "value": "AI", "timestamp": 1711699000}
   ]}
```

### history

Get all entries for a specific key across all agents. Returns newest first.

```json
-> {"method": "history", "key": "status"}
<- {"entries": [
     {"hash": "...", "agent_id": "agent-2", "key": "status", "value": "running", "timestamp": 1711700001},
     {"hash": "...", "agent_id": "agent-1", "key": "status", "value": "starting", "timestamp": 1711700000}
   ]}
```

### Error responses

```json
<- {"error": "agent_id and key are required"}
<- {"error": "not found"}
<- {"error": "invalid JSON: ..."}
```

## Status

**Implemented:**
- Content-addressed storage with SHA-256 hashing
- SQLite backend with indexed queries
- Idempotent writes (INSERT OR IGNORE)
- TCP server on port 9100 (agent access via TSI)
- Unix socket at /run/ctxgraph.sock (host access)
- Full CRUD: write, read, query (by prefix), history
- Graph module for high-level operations
- by_agent query for listing all entries from a specific agent
- Test agent successfully writes from inside a microVM

**Planned:**
- Entry expiration / TTL
- Access control (restrict which agents can read which keys)
- Pagination for large query results
- Streaming notifications when new entries are written
- Persistent storage (currently on tmpfs /var, lost on reboot)
