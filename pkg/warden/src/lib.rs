//! Warden — Encrypted secret vault for NullBox agents.
//!
//! Encrypts agent credentials at rest using AES-256-GCM. Each agent
//! can only access the secrets declared in its AGENT.toml credential_refs.
//! Secrets flow: warden → cage → env vars inside microVM.

pub mod crypto;
pub mod master_key;
pub mod vault;
