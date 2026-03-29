//! cage daemon — per-agent microVM manager for NullBox.
//!
//! Two modes:
//! 1. Daemon mode (default): scans manifests, listens for commands, manages VM lifecycle
//! 2. VM runner mode (--run-vm): child process that calls krun_start_enter()

use cage::krun::{self, VmConfig};
use cage::manifest;
use cage::vm::VmManager;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

const EGRESS_SOCKET: &str = "/run/egress.sock";

const SOCKET_PATH: &str = "/run/cage.sock";
const AGENT_DIR: &str = "/agent";
const AGENT_ROOTFS_BASE: &str = "/system/rootfs";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Child process mode: run a VM and never return
    if args.len() >= 3 && args[1] == "--run-vm" {
        run_vm_child(&args[2]);
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

/// Child process: parse VM config, call krun_start_enter (never returns).
fn run_vm_child(config_json: &str) {
    let config: VmConfig = match serde_json::from_str(config_json) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cage: invalid VM config: {e}");
            std::process::exit(1);
        }
    };

    krun::set_log_level(2); // Warn

    if let Err(e) = krun::run_vm(&config) {
        eprintln!("cage: VM '{}' failed: {e}", config.name);
        std::process::exit(1);
    }
}

/// Daemon mode: scan manifests, listen for commands.
fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    log("cage: starting microVM manager");

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

    // Listen for lifecycle commands
    let listener = UnixListener::bind(SOCKET_PATH)?;
    log(&format!("cage: listening on {SOCKET_PATH}"));

    // Auto-start all agents that have a valid rootfs
    for (name, manifest) in &manifests {
        let exec_path = format!("/agent/bin/{name}");
        let rootfs_path = format!("{AGENT_ROOTFS_BASE}/{name}");
        let bin_path = format!("{rootfs_path}/agent/bin/{name}");
        if Path::new(&bin_path).exists() {
            match vm_mgr.start_agent(manifest, &exec_path) {
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

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                handle_command(&stream, &mut vm_mgr, &manifests);
            }
            Err(e) => {
                log(&format!("cage: accept error: {e}"));
            }
        }
    }

    Ok(())
}

/// Handle a command from the Unix socket.
fn handle_command(
    stream: &std::os::unix::net::UnixStream,
    vm_mgr: &mut VmManager,
    manifests: &HashMap<String, manifest::AgentManifest>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    let reader = BufReader::new(stream);
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

    match vm_mgr.start_agent(manifest, &exec_path) {
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
            if path.extension().is_some_and(|e| e == "toml") {
                if let Ok(content) = std::fs::read_to_string(&path) {
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
    }

    manifests
}

/// Send add-agent request to egress daemon with the agent's allowed domains.
fn notify_egress_add(agent_name: &str, domains: &[String]) {
    let request = serde_json::json!({
        "method": "add-agent",
        "agent": agent_name,
        "domains": domains,
    });

    match send_to_egress(&request) {
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

    match send_to_egress(&request) {
        Ok(resp) => log(&format!("cage: egress remove-agent '{agent_name}': {resp}")),
        Err(e) => log(&format!("cage: egress remove-agent '{agent_name}' failed: {e}")),
    }
}

/// Send a JSON request to the egress Unix socket and read one line of response.
fn send_to_egress(request: &serde_json::Value) -> Result<String, String> {
    let mut stream = UnixStream::connect(EGRESS_SOCKET)
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
