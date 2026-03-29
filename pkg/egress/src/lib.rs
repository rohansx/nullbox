//! Egress — Default-deny network controller.
//!
//! Blocks all outbound traffic by default. Only domains explicitly declared
//! in an agent's AGENT.toml manifest are allowed. Blocks cloud metadata
//! endpoints and RFC-1918 ranges from agent traffic.

pub mod blocklist;
pub mod firewall;
pub mod resolve;
