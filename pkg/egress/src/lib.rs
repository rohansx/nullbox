//! Egress — Default-deny network controller.
//!
//! Blocks all outbound traffic by default. Only domains explicitly declared
//! in an agent's AGENT.toml manifest are allowed. Blocks cloud metadata
//! endpoints and RFC-1918 ranges from agent traffic.

pub mod blocklist;
pub mod firewall;

// DNS resolver with IP binding will be implemented when we integrate
// with the actual nftables runtime:
// pub mod dns;
