# ctxgraph — Agent Memory & Context Graph

> OS-level shared memory. Every agent on the box shares one unified, queryable, tamper-evident knowledge graph.

**Layer:** NullBox Layer 8

---

## Why ctxgraph Exists

Every agent runtime reinvents memory poorly. RAG pipelines, vector stores, conversation logs — they're all isolated per-agent, mutable, and unverifiable. When a `researcher` agent finds data, a `lead-enricher` agent has to re-fetch it because there's no shared state bus.

ctxgraph is the operating system's answer to agent memory: a single, queryable, tamper-evident graph that all agents on the box read from and write to.

---

## Technical Design

- **Entity extraction:** ONNX/GLiNER2 for local NER
- **Storage:** SQLite (simple, no external dependency)
- **Vector index:** Semantic search for context retrieval, offloaded to NPU/GPU via Accelerator Manager where available
- **Temporal awareness:** Every entry knows when it was created, by which agent, from what source. Entries decay in weight over time unless reinforced.

---

## Cross-Agent Communication

```
researcher writes: { key: "lead_data", entity: "John Smith", source: "exa_api", timestamp: "..." }
    |
enricher queries: ctxgraph.query("new_lead") -> gets researcher's findings
    |
enricher writes: { key: "enriched_lead", entity: "John Smith", clearbit_data: {...} }
    |
reporter queries: ctxgraph.query("enrichment_done") -> gets enriched data
```

Agents communicate through structured graph entries, not raw text pipes. This is the Swarm layer's state bus.

---

## Memory Integrity

- Each entry is **content-addressed** and **signed** with the writing agent's Provenance Vault identity key (Ed25519)
- Retroactive modification changes the hash — Watcher detects inconsistency
- Memory poisoning attacks are caught: if an attacker modifies ctxgraph entries, the signature chain breaks

---

## Sentinel Integration

Sentinel uses ctxgraph history to detect behavioral anomalies. If an agent suddenly receives instructions dramatically inconsistent with its established operational history, Sentinel scores it higher risk.

---

## MCP Interface

ctxgraph exposes itself as MCP tools:

- `ctxgraph.write` — write an entry (signed with agent's Provenance key)
- `ctxgraph.query` — semantic search across the graph
- `ctxgraph.read` — read specific entries by key
- `ctxgraph.history` — get temporal history of an entity
