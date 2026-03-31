//! cage daemon — per-agent microVM manager for NullBox.
//!
//! Two modes:
//! 1. Daemon mode (default): scans manifests, listens for commands, manages VM lifecycle
//! 2. VM runner mode (--run-vm): child process that calls krun_start_enter()

use cage::krun::{self, VmConfig};
use cage::manifest;
use cage::vm::{self, VmManager};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

const EGRESS_SOCKET: &str = "/run/egress.sock";
const WARDEN_SOCKET: &str = "/run/warden.sock";

const SOCKET_PATH: &str = "/run/cage.sock";
const AGENT_DIR: &str = "/agent";
const AGENT_ROOTFS_BASE: &str = "/system/rootfs";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Child process mode: run a VM and never return
    if args.len() >= 2 && args[1] == "--run-vm" {
        run_vm_child();
        // Should never reach here
        std::process::exit(1);
    }

    // Daemon mode
    let result = run_daemon();
    if let Err(e) = result {
        log(&format!("cage: fatal: {e}"));
        std::process::exit(1);
    }
}

/// Child process: parse VM config from stdin, call krun_start_enter (never returns).
fn run_vm_child() {
    let mut config_json = String::new();
    std::io::stdin().read_to_string(&mut config_json).unwrap_or_else(|e| {
        eprintln!("cage: failed to read VM config from stdin: {e}");
        std::process::exit(1);
    });

    let config: VmConfig = match serde_json::from_str(&config_json) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cage: invalid VM config: {e}");
            std::process::exit(1);
        }
    };

    krun::set_log_level(2); // Warn

    // Seccomp + Landlock sandbox — default ON. Set CAGE_NO_SANDBOX=1 to skip.
    let pre_enter = || {
        if std::env::var("CAGE_NO_SANDBOX").is_ok() {
            eprintln!("cage: [{}] sandbox disabled via CAGE_NO_SANDBOX", config.name);
            return;
        }
        let name = &config.name;

        // Landlock filesystem sandboxing
        if let Some(toml_json) = config.manifest_toml.as_deref() {
            if let Ok(manifest) = serde_json::from_str::<cage::manifest::AgentManifest>(toml_json) {
                let sandbox = cage::fs_sandbox::build_sandbox(&config, &manifest);
                match cage::fs_sandbox::apply(&sandbox) {
                    Ok(()) => eprintln!("cage: [{name}] Landlock applied"),
                    Err(cage::fs_sandbox::SandboxError::NotSupported) => {
                        eprintln!("cage: [{name}] Landlock not supported — continuing without fs sandbox");
                    }
                    Err(e) => {
                        eprintln!("cage: [{name}] Landlock FAILED: {e} — aborting");
                        std::process::exit(1);
                    }
                }

                // Seccomp syscall filtering (audit mode for now, kill mode when stable)
                let profile = cage::seccomp::build_profile(&manifest);
                match cage::seccomp::apply(&profile, false) {
                    Ok(()) => eprintln!("cage: [{name}] seccomp applied (audit mode)"),
                    Err(e) => {
                        eprintln!("cage: [{name}] seccomp FAILED: {e} — aborting");
                        std::process::exit(1);
                    }
                }
            }
        }
    };

    if let Err(e) = krun::run_vm(&config, pre_enter) {
        eprintln!("cage: VM '{}' failed: {e}", config.name);
        std::process::exit(1);
    }
}

/// Daemon mode: scan manifests, listen for commands, reap children.
fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    log("cage: starting microVM manager");

    // Create log directory for per-agent console output
    let _ = std::fs::create_dir_all("/var/log/cage");

    // Check for KVM support
    if Path::new("/dev/kvm").exists() {
        log("cage: KVM available");
    } else {
        log("cage: WARNING — /dev/kvm not found, microVMs will not work");
    }

    // Scan for agent manifests
    let manifests = load_manifests();
    log(&format!("cage: {} agent manifest(s) loaded", manifests.len()));

    // Initialize VM manager (rootfs lives on read-only SquashFS at /system/rootfs/)
    let mut vm_mgr = VmManager::new(AGENT_ROOTFS_BASE);

    // Clean up stale socket
    let sock_path = Path::new(SOCKET_PATH);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }

    // Listen for lifecycle commands (non-blocking for SIGCHLD interleaving)
    let listener = UnixListener::bind(SOCKET_PATH)?;
    std::fs::set_permissions(SOCKET_PATH, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    log(&format!("cage: listening on {SOCKET_PATH}"));

    // Auto-start all agents that have a valid rootfs
    for (name, manifest) in &manifests {
        let exec_path = format!("/agent/bin/{name}");
        let rootfs_path = format!("{AGENT_ROOTFS_BASE}/{name}");
        let bin_path = format!("{rootfs_path}/agent/bin/{name}");
        if Path::new(&bin_path).exists() {
            let secrets = request_secrets(name, &manifest.capabilities.credential_refs);
            match vm_mgr.start_agent(manifest, &exec_path, &secrets) {
                Ok(pid) => {
                    log(&format!("cage: auto-started '{name}' (PID {pid})"));
                    notify_egress_add(name, &manifest.capabilities.network.allow);
                }
                Err(e) => log(&format!("cage: failed to auto-start '{name}': {e}")),
            }
        } else {
            log(&format!("cage: skipping '{name}' — no rootfs at {rootfs_path}"));
        }
    }

    // Main loop: accept socket commands + reap dead children
    loop {
        // Reap any exited child processes (non-blocking)
        reap_children(&mut vm_mgr, &manifests);

        // Try to accept a connection (non-blocking)
        match listener.accept() {
            Ok((stream, _)) => {
                handle_command(&stream, &mut vm_mgr, &manifests);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connections — sleep briefly to avoid busy-wait
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                log(&format!("cage: accept error: {e}"));
            }
        }
    }
}

/// Reap all exited child processes via waitpid(WNOHANG).
///
/// On crash (non-zero exit), restarts the agent after a brief delay.
fn reap_children(
    vm_mgr: &mut VmManager,
    manifests: &HashMap<String, manifest::AgentManifest>,
) {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, status)) => {
                let pid_u32 = pid.as_raw() as u32;
                if let Some(exit) = vm_mgr.handle_exit(pid_u32, status) {
                    log(&format!(
                        "cage: agent '{}' exited (status {})",
                        exit.agent_name, exit.exit_status
                    ));
                    notify_egress_remove(&exit.agent_name);

                    if vm::should_restart(exit.exit_status)
                        && let Some(manifest) = manifests.get(&exit.agent_name)
                    {
                        log(&format!(
                            "cage: restarting '{}' after crash",
                            exit.agent_name
                        ));
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        let exec_path = format!("/agent/bin/{}", exit.agent_name);
                        let secrets = request_secrets(
                            &exit.agent_name,
                            &manifest.capabilities.credential_refs,
                        );
                        match vm_mgr.start_agent(manifest, &exec_path, &secrets) {
                            Ok(pid) => {
                                log(&format!(
                                    "cage: restarted '{}' (PID {pid})",
                                    exit.agent_name
                                ));
                                notify_egress_add(
                                    &exit.agent_name,
                                    &manifest.capabilities.network.allow,
                                );
                            }
                            Err(e) => log(&format!(
                                "cage: failed to restart '{}': {e}",
                                exit.agent_name
                            )),
                        }
                    }
                }
            }
            Ok(WaitStatus::Signaled(pid, signal, _)) => {
                let pid_u32 = pid.as_raw() as u32;
                if let Some(exit) = vm_mgr.handle_exit(pid_u32, 128 + signal as i32) {
                    log(&format!(
                        "cage: agent '{}' killed by signal {}",
                        exit.agent_name, signal
                    ));
                    notify_egress_remove(&exit.agent_name);
                }
            }
            Ok(WaitStatus::StillAlive) | Err(_) => break,
            _ => continue,
        }
    }
}

/// Handle a command from the Unix socket.
fn handle_command(
    stream: &std::os::unix::net::UnixStream,
    vm_mgr: &mut VmManager,
    manifests: &HashMap<String, manifest::AgentManifest>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    let reader = BufReader::new(stream.take(65536));
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => return,
        };

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => return,
        };

        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");

        let response = match method {
            "start" => {
                let name = request
                    .get("agent")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                handle_start(vm_mgr, manifests, name)
            }
            "stop" => {
                let name = request
                    .get("agent")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                handle_stop(vm_mgr, name)
            }
            "list" => {
                let vms: Vec<serde_json::Value> = vm_mgr
                    .list()
                    .iter()
                    .map(|vm| {
                        serde_json::json!({
                            "name": vm.agent_name,
                            "pid": vm.pid,
                            "vcpus": vm.vcpus,
                            "ram_mib": vm.ram_mib,
                        })
                    })
                    .collect();
                serde_json::json!({"agents": vms})
            }
            _ => serde_json::json!({"error": format!("unknown method: {method}")}),
        };

        let mut writer = stream;
        let _ = writeln!(writer, "{}", serde_json::to_string(&response).unwrap_or_default());
        let _ = writer.flush();
    }
}

fn handle_start(
    vm_mgr: &mut VmManager,
    manifests: &HashMap<String, manifest::AgentManifest>,
    name: &str,
) -> serde_json::Value {
    let manifest = match manifests.get(name) {
        Some(m) => m,
        None => return serde_json::json!({"error": format!("unknown agent: {name}")}),
    };

    // The exec_path inside the VM — agent binaries live at /agent/bin/<name>
    let exec_path = format!("/agent/bin/{name}");

    // Fetch agent secrets from warden
    let secrets = request_secrets(name, &manifest.capabilities.credential_refs);

    match vm_mgr.start_agent(manifest, &exec_path, &secrets) {
        Ok(pid) => {
            log(&format!("cage: started agent '{name}' in VM (PID {pid})"));

            // Notify egress to allow this agent's declared domains
            notify_egress_add(name, &manifest.capabilities.network.allow);

            serde_json::json!({"ok": true, "pid": pid})
        }
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn handle_stop(
    vm_mgr: &mut VmManager,
    name: &str,
) -> serde_json::Value {
    match vm_mgr.stop_agent(name) {
        Ok(()) => {
            log(&format!("cage: stopped agent '{name}'"));

            // Notify egress to revoke this agent's network access
            notify_egress_remove(name);

            serde_json::json!({"ok": true})
        }
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn load_manifests() -> HashMap<String, manifest::AgentManifest> {
    let mut manifests = HashMap::new();
    let agent_dir = Path::new(AGENT_DIR);

    if !agent_dir.is_dir() {
        return manifests;
    }

    if let Ok(entries) = std::fs::read_dir(agent_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                match manifest::parse(&content) {
                    Ok(m) => {
                        log(&format!(
                            "cage: found agent '{}' v{}",
                            m.agent.name, m.agent.version
                        ));
                        manifests.insert(m.agent.name.clone(), m);
                    }
                    Err(e) => {
                        log(&format!(
                            "cage: invalid manifest {}: {e}",
                            path.display()
                        ));
                    }
                }
            }
        }
    }

    manifests
}

/// Request agent secrets from the warden vault.
///
/// Returns only the secrets matching the agent's credential_refs.
/// On failure, returns an empty map (warden may not be running during dev).
fn request_secrets(agent_name: &str, credential_refs: &[String]) -> HashMap<String, String> {
    if credential_refs.is_empty() {
        return HashMap::new();
    }

    let request = serde_json::json!({
        "method": "get-secrets",
        "agent": agent_name,
        "credential_refs": credential_refs,
    });

    match send_to_service(WARDEN_SOCKET, &request) {
        Ok(resp) => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&resp)
                && let Some(secrets) = parsed.get("secrets").and_then(|s| s.as_object())
            {
                let map: HashMap<String, String> = secrets
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect();
                log(&format!(
                    "cage: warden returned {} secret(s) for '{agent_name}'",
                    map.len()
                ));
                return map;
            }
            log(&format!("cage: warden unexpected response for '{agent_name}': {resp}"));
            HashMap::new()
        }
        Err(e) => {
            log(&format!("cage: warden request for '{agent_name}' failed: {e}"));
            HashMap::new()
        }
    }
}

/// Send add-agent request to egress daemon with the agent's allowed domains.
fn notify_egress_add(agent_name: &str, domains: &[String]) {
    let request = serde_json::json!({
        "method": "add-agent",
        "agent": agent_name,
        "domains": domains,
    });

    match send_to_service(EGRESS_SOCKET, &request) {
        Ok(resp) => log(&format!("cage: egress add-agent '{agent_name}': {resp}")),
        Err(e) => log(&format!("cage: egress add-agent '{agent_name}' failed: {e}")),
    }
}

/// Send remove-agent request to egress daemon.
fn notify_egress_remove(agent_name: &str) {
    let request = serde_json::json!({
        "method": "remove-agent",
        "agent": agent_name,
    });

    match send_to_service(EGRESS_SOCKET, &request) {
        Ok(resp) => log(&format!("cage: egress remove-agent '{agent_name}': {resp}")),
        Err(e) => log(&format!("cage: egress remove-agent '{agent_name}' failed: {e}")),
    }
}

/// Send a JSON request to a Unix socket and read one line of response.
fn send_to_service(socket_path: &str, request: &serde_json::Value) -> Result<String, String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| format!("connect: {e}"))?;

    stream
        .set_write_timeout(Some(std::time::Duration::from_millis(500)))
        .ok();
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(2000)))
        .ok();

    let msg = serde_json::to_string(request).map_err(|e| format!("serialize: {e}"))?;
    writeln!(stream, "{msg}").map_err(|e| format!("write: {e}"))?;
    stream.flush().map_err(|e| format!("flush: {e}"))?;

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|e| format!("read: {e}"))?;

    Ok(response.trim().to_string())
}

fn log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/kmsg")
    {
        let _ = writeln!(f, "{msg}");
    } else {
        eprintln!("{msg}");
    }
}
