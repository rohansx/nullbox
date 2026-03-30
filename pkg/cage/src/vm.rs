//! VM lifecycle manager for Cage.
//!
//! Each agent runs in its own microVM via libkrun.
//! Since krun_start_enter() blocks forever, we fork a child process per VM.
//! The parent (cage daemon) tracks child PIDs and handles lifecycle.

use crate::krun::{self, VmConfig, VirtiofsMount};
use crate::manifest::AgentManifest;
use std::collections::HashMap;

/// Returned when a VM child process exits.
pub struct VmExit {
    pub agent_name: String,
    pub exit_status: i32,
}

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
        secrets: &HashMap<String, String>,
    ) -> Result<u32, VmError> {
        let name = &manifest.agent.name;

        if self.vms.contains_key(name) {
            return Err(VmError::AlreadyRunning(name.clone()));
        }

        let vcpus = manifest.capabilities.max_cpu_percent.min(100) as u8;
        // Map cpu percent to vCPUs: 25% = 1, 50% = 2, 100% = 4
        let vcpus = (vcpus as u32).div_ceil(25).clamp(1, 8) as u8;
        let ram_mib = manifest.capabilities.max_memory_mb.max(64);

        let rootfs_path =
            format!("{}/{}", self.agent_rootfs_base, name);

        // Build virtiofs mounts from manifest filesystem declarations
        let virtiofs_mounts = build_virtiofs_mounts(name, &manifest.capabilities.filesystem);

        // Build rlimits from manifest resource fields
        let rlimits = build_rlimits(manifest);

        // Per-agent console log (when log directory exists)
        // Note: setting console_output redirects ALL output to the file,
        // which hides it from the serial console. Only enable when explicitly
        // configured, not by default.
        let console_output = None;

        let config = VmConfig {
            name: name.clone(),
            vcpus,
            ram_mib,
            root_path: rootfs_path,
            exec_path: exec_path.to_string(),
            args: vec![],
            env: {
                let mut env = vec![
                    format!("AGENT_NAME={name}"),
                    "CTXGRAPH_PORT=9100".to_string(),
                ];
                for (k, v) in secrets {
                    env.push(format!("{k}={v}"));
                }
                env
            },
            port_map: vec![],
            workdir: "/".to_string(),
            virtiofs_mounts,
            rlimits,
            console_output,
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
    pub fn handle_exit(&mut self, pid: u32, exit_status: i32) -> Option<VmExit> {
        let name = self
            .vms
            .iter()
            .find(|(_, vm)| vm.pid == pid)
            .map(|(name, _)| name.clone());

        if let Some(ref name) = name {
            self.vms.remove(name);
            return Some(VmExit {
                agent_name: name.clone(),
                exit_status,
            });
        }

        None
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

/// Build virtiofs mount list from manifest filesystem declarations.
///
/// Each declared path gets a host-side directory under `/var/lib/cage/<agent>/`
/// and a virtiofs tag derived from the path.
fn build_virtiofs_mounts(
    agent_name: &str,
    fs: &crate::manifest::FilesystemCaps,
) -> Vec<VirtiofsMount> {
    let mut mounts = Vec::new();

    for path in &fs.read {
        let tag = format!("{}_ro", krun::path_to_virtiofs_tag(path));
        let host_path = format!("/var/lib/cage/{agent_name}{path}");
        let _ = std::fs::create_dir_all(&host_path);
        mounts.push(VirtiofsMount { tag, host_path });
    }

    for path in &fs.write {
        let tag = format!("{}_rw", krun::path_to_virtiofs_tag(path));
        let host_path = format!("/var/lib/cage/{agent_name}{path}");
        let _ = std::fs::create_dir_all(&host_path);
        mounts.push(VirtiofsMount { tag, host_path });
    }

    mounts
}

/// Build rlimit strings from manifest resource fields.
///
/// Format: `"RLIMIT_<NAME>=<cur>:<max>"` (libkrun convention).
fn build_rlimits(manifest: &AgentManifest) -> Vec<String> {
    let mut rlimits = Vec::new();

    // Address space limit from max_memory_mb
    let mem_bytes = (manifest.capabilities.max_memory_mb as u64) * 1024 * 1024;
    rlimits.push(format!("RLIMIT_AS={mem_bytes}:{mem_bytes}"));

    // Cap processes per agent
    rlimits.push("RLIMIT_NPROC=64:64".to_string());

    rlimits
}

/// Whether a VM exit status indicates the agent should be restarted.
///
/// Non-zero = crash, restart. Zero = clean exit, don't restart.
pub fn should_restart(exit_status: i32) -> bool {
    exit_status != 0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlimits_from_manifest() {
        let manifest = crate::manifest::parse(
            r#"
            [agent]
            name = "test"

            [capabilities]
            max_memory_mb = 512
            "#,
        )
        .unwrap();

        let rlimits = build_rlimits(&manifest);
        assert_eq!(rlimits.len(), 2);
        // 512 MiB = 536870912 bytes
        assert_eq!(rlimits[0], "RLIMIT_AS=536870912:536870912");
        assert_eq!(rlimits[1], "RLIMIT_NPROC=64:64");
    }

    #[test]
    fn rlimits_minimum_memory() {
        let manifest = crate::manifest::parse(
            r#"
            [agent]
            name = "tiny"

            [capabilities]
            max_memory_mb = 64
            "#,
        )
        .unwrap();

        let rlimits = build_rlimits(&manifest);
        // 64 MiB = 67108864 bytes
        assert_eq!(rlimits[0], "RLIMIT_AS=67108864:67108864");
    }

    #[test]
    fn virtiofs_mounts_from_manifest() {
        let manifest = crate::manifest::parse(
            r#"
            [agent]
            name = "researcher"

            [capabilities.filesystem]
            read = ["/data/corpus"]
            write = ["/data/output"]
            "#,
        )
        .unwrap();

        let mounts = build_virtiofs_mounts("researcher", &manifest.capabilities.filesystem);
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].tag, "data_corpus_ro");
        assert_eq!(
            mounts[0].host_path,
            "/var/lib/cage/researcher/data/corpus"
        );
        assert_eq!(mounts[1].tag, "data_output_rw");
        assert_eq!(
            mounts[1].host_path,
            "/var/lib/cage/researcher/data/output"
        );
    }

    #[test]
    fn should_restart_logic() {
        assert!(should_restart(1));
        assert!(should_restart(137)); // SIGKILL
        assert!(!should_restart(0)); // clean exit
    }

    #[test]
    fn handle_exit_returns_status() {
        let mut mgr = VmManager::new("/tmp/rootfs");
        mgr.vms.insert(
            "test".to_string(),
            RunningVm {
                agent_name: "test".to_string(),
                pid: 1234,
                vcpus: 1,
                ram_mib: 256,
            },
        );

        let exit = mgr.handle_exit(1234, 137).unwrap();
        assert_eq!(exit.agent_name, "test");
        assert_eq!(exit.exit_status, 137);
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn handle_exit_unknown_pid() {
        let mut mgr = VmManager::new("/tmp/rootfs");
        assert!(mgr.handle_exit(9999, 0).is_none());
    }
}
