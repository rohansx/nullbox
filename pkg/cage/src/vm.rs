//! VM lifecycle manager for Cage.
//!
//! Each agent runs in its own microVM via libkrun.
//! Since krun_start_enter() blocks forever, we fork a child process per VM.
//! The parent (cage daemon) tracks child PIDs and handles lifecycle.

use crate::krun::VmConfig;
use crate::manifest::AgentManifest;
use std::collections::HashMap;

/// State of a running agent VM.
#[derive(Debug)]
pub struct RunningVm {
    pub agent_name: String,
    pub pid: u32,
    pub vcpus: u8,
    pub ram_mib: u32,
}

/// Manages all running agent VMs.
pub struct VmManager {
    vms: HashMap<String, RunningVm>,
    agent_rootfs_base: String,
}

impl VmManager {
    pub fn new(agent_rootfs_base: &str) -> Self {
        Self {
            vms: HashMap::new(),
            agent_rootfs_base: agent_rootfs_base.to_string(),
        }
    }

    /// Start an agent in a new microVM.
    ///
    /// Creates a VM config from the AGENT.toml manifest, forks a child,
    /// and the child calls krun_start_enter().
    pub fn start_agent(
        &mut self,
        manifest: &AgentManifest,
        exec_path: &str,
    ) -> Result<u32, VmError> {
        let name = &manifest.agent.name;

        if self.vms.contains_key(name) {
            return Err(VmError::AlreadyRunning(name.clone()));
        }

        let vcpus = manifest.capabilities.max_cpu_percent.min(100) as u8;
        // Map cpu percent to vCPUs: 25% = 1, 50% = 2, 100% = 4
        let vcpus = ((vcpus as u32 + 24) / 25).max(1).min(8) as u8;
        let ram_mib = manifest.capabilities.max_memory_mb.max(64);

        let rootfs_path =
            format!("{}/{}", self.agent_rootfs_base, name);

        let config = VmConfig {
            name: name.clone(),
            vcpus,
            ram_mib,
            root_path: rootfs_path,
            exec_path: exec_path.to_string(),
            args: vec![],
            env: vec![
                format!("AGENT_NAME={name}"),
                "CTXGRAPH_PORT=9100".to_string(),
            ],
            // TSI handles outbound transparently — no port map needed
            port_map: vec![],
            workdir: "/".to_string(),
        };

        validate_rootfs(&config.root_path, &config.exec_path)?;
        let pid = spawn_vm_process(&config)?;

        self.vms.insert(
            name.clone(),
            RunningVm {
                agent_name: name.clone(),
                pid,
                vcpus,
                ram_mib,
            },
        );

        Ok(pid)
    }

    /// Stop an agent's VM by sending SIGTERM to its process.
    pub fn stop_agent(&mut self, name: &str) -> Result<(), VmError> {
        let vm = self
            .vms
            .remove(name)
            .ok_or_else(|| VmError::NotRunning(name.to_string()))?;

        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(vm.pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );

        Ok(())
    }

    /// Handle a child process exit — remove the VM from tracking.
    pub fn handle_exit(&mut self, pid: u32) -> Option<String> {
        let name = self
            .vms
            .iter()
            .find(|(_, vm)| vm.pid == pid)
            .map(|(name, _)| name.clone());

        if let Some(ref name) = name {
            self.vms.remove(name);
        }

        name
    }

    /// List all running VMs.
    pub fn list(&self) -> Vec<&RunningVm> {
        self.vms.values().collect()
    }

    /// Check if an agent is running.
    pub fn is_running(&self, name: &str) -> bool {
        self.vms.contains_key(name)
    }
}

/// Validate that the rootfs directory and exec binary exist before spawning.
fn validate_rootfs(root_path: &str, exec_path: &str) -> Result<(), VmError> {
    let root = std::path::Path::new(root_path);
    if !root.is_dir() {
        return Err(VmError::RootfsInvalid(format!(
            "rootfs directory not found: {root_path}"
        )));
    }

    let bin_host_path = root.join(exec_path.trim_start_matches('/'));
    if !bin_host_path.exists() {
        return Err(VmError::RootfsInvalid(format!(
            "agent binary not found at {}",
            bin_host_path.display()
        )));
    }

    Ok(())
}

/// Fork a child process that calls krun_start_enter().
fn spawn_vm_process(config: &VmConfig) -> Result<u32, VmError> {
    let config_json =
        serde_json::to_string(config).map_err(|e| VmError::Internal(e.to_string()))?;

    // Use our own binary path so this works both inside SquashFS and during dev
    let cage_bin = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("/system/bin/cage"));

    let child = std::process::Command::new(cage_bin)
        .arg("--run-vm")
        .arg(&config_json)
        .spawn()
        .map_err(|e| VmError::SpawnFailed(e.to_string()))?;

    Ok(child.id())
}

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("agent '{0}' is already running")]
    AlreadyRunning(String),
    #[error("agent '{0}' is not running")]
    NotRunning(String),
    #[error("failed to spawn VM process: {0}")]
    SpawnFailed(String),
    #[error("rootfs invalid: {0}")]
    RootfsInvalid(String),
    #[error("internal error: {0}")]
    Internal(String),
}
