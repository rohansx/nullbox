//! Cage — Per-agent microVM isolation via libkrun.
//!
//! Each agent gets its own microVM with hardware-level isolation.
//! Capabilities are declared in AGENT.toml and enforced at the hypervisor level.

pub mod manifest;

// These modules will be implemented as libkrun integration progresses:
// pub mod vm;
// pub mod network;
// pub mod filesystem;
// pub mod resources;
