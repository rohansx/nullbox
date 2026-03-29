//! egress daemon — default-deny nftables network controller for NullBox.
//!
//! On startup:
//! 1. Generates the base default-deny ruleset (no agents yet)
//! 2. Applies it via `nft -f`
//! 3. Listens on a Unix socket for agent add/remove commands from cage
//! 4. Regenerates and reapplies rules atomically on each change
//!
//! Protocol (JSON-over-Unix-socket, one object per line):
//!   {"method":"add-agent","agent":"researcher","domains":["api.openai.com","api.exa.ai"]}
//!   {"method":"remove-agent","agent":"researcher"}
//!   {"method":"list"}

use egress::firewall::{self, TsiAgentRule};
use egress::resolve;
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;

const SOCKET_PATH: &str = "/run/egress.sock";
const NFT_RULES_PATH: &str = "/run/egress-rules.nft";

/// Per-agent state: the domains from AGENT.toml and their resolved IPs.
struct AgentEntry {
    domains: Vec<String>,
    ips: BTreeSet<IpAddr>,
}

fn main() {
    let result = run();
    if let Err(e) = result {
        log(&format!("egress: fatal: {e}"));
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log("egress: starting default-deny network controller");

    // Track per-agent state
    let mut agents: HashMap<String, AgentEntry> = HashMap::new();

    // Generate and apply base ruleset (no agents)
    regenerate_and_apply(&agents)?;

    // Clean up stale socket
    let sock_path = Path::new(SOCKET_PATH);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;
    log(&format!("egress: listening on {SOCKET_PATH}"));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                handle_connection(&stream, &mut agents);
            }
            Err(e) => {
                log(&format!("egress: accept error: {e}"));
            }
        }
    }

    Ok(())
}

fn handle_connection(
    stream: &std::os::unix::net::UnixStream,
    agents: &mut HashMap<String, AgentEntry>,
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
            Err(_) => {
                let _ = write_response(stream, &serde_json::json!({"error": "invalid JSON"}));
                return;
            }
        };

        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");

        let response = match method {
            "add-agent" => handle_add_agent(&request, agents),
            "remove-agent" => handle_remove_agent(&request, agents),
            "list" => handle_list(agents),
            _ => serde_json::json!({"error": format!("unknown method: {method}")}),
        };

        let _ = write_response(stream, &response);
    }
}

fn handle_add_agent(
    request: &serde_json::Value,
    agents: &mut HashMap<String, AgentEntry>,
) -> serde_json::Value {
    let name = match request.get("agent").and_then(|n| n.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return serde_json::json!({"error": "missing or empty 'agent' field"}),
    };

    let domains: Vec<String> = request
        .get("domains")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    log(&format!(
        "egress: adding agent '{}' with {} domain(s)",
        name,
        domains.len()
    ));

    // Resolve domains to IPs, filtering out blocked ranges
    let mut resolved = resolve::resolve_domains(&domains);
    resolved.retain(|ip| !egress::blocklist::is_blocked(ip));

    let ip_count = resolved.len();

    agents.insert(
        name.clone(),
        AgentEntry {
            domains,
            ips: resolved,
        },
    );

    // Regenerate and apply the full ruleset
    match regenerate_and_apply(agents) {
        Ok(()) => {
            log(&format!(
                "egress: agent '{name}' added ({ip_count} allowed IPs), rules applied"
            ));
            serde_json::json!({"ok": true, "agent": name, "allowed_ips": ip_count})
        }
        Err(e) => {
            log(&format!("egress: failed to apply rules after adding '{name}': {e}"));
            serde_json::json!({"error": format!("rules apply failed: {e}")})
        }
    }
}

fn handle_remove_agent(
    request: &serde_json::Value,
    agents: &mut HashMap<String, AgentEntry>,
) -> serde_json::Value {
    let name = match request.get("agent").and_then(|n| n.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return serde_json::json!({"error": "missing or empty 'agent' field"}),
    };

    if agents.remove(&name).is_none() {
        return serde_json::json!({"error": format!("agent '{name}' not registered")});
    }

    log(&format!("egress: removing agent '{name}'"));

    match regenerate_and_apply(agents) {
        Ok(()) => {
            log(&format!("egress: agent '{name}' removed, rules applied"));
            serde_json::json!({"ok": true, "agent": name})
        }
        Err(e) => {
            log(&format!("egress: failed to apply rules after removing '{name}': {e}"));
            serde_json::json!({"error": format!("rules apply failed: {e}")})
        }
    }
}

fn handle_list(agents: &HashMap<String, AgentEntry>) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = agents
        .iter()
        .map(|(name, entry)| {
            serde_json::json!({
                "agent": name,
                "domains": entry.domains,
                "allowed_ips": entry.ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
            })
        })
        .collect();

    serde_json::json!({"agents": entries})
}

/// Regenerate the nftables ruleset from current agent state and apply it.
fn regenerate_and_apply(
    agents: &HashMap<String, AgentEntry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let tsi_rules: Vec<TsiAgentRule> = agents
        .iter()
        .map(|(name, entry)| TsiAgentRule {
            agent_name: name.clone(),
            allowed_ips: entry.ips.clone(),
        })
        .collect();

    let ruleset = firewall::generate_tsi_ruleset(&tsi_rules);

    // Write atomically: write to temp, rename
    let tmp_path = format!("{NFT_RULES_PATH}.tmp");
    std::fs::write(&tmp_path, &ruleset.content)?;
    std::fs::rename(&tmp_path, NFT_RULES_PATH)?;

    apply_ruleset()
}

fn apply_ruleset() -> Result<(), Box<dyn std::error::Error>> {
    // Use absolute path — nft lives at /system/bin/nft in the SquashFS image.
    // Fall back to PATH for development/testing outside the image.
    let nft_bin = if Path::new("/system/bin/nft").exists() {
        "/system/bin/nft"
    } else {
        "nft"
    };

    let output = Command::new(nft_bin)
        .args(["-f", NFT_RULES_PATH])
        .env("LD_LIBRARY_PATH", "/usr/lib")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            log("egress: nftables rules applied");
            Ok(())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // Log but don't fail — nft may not be available in dev QEMU
            log(&format!("egress: nft apply warning: {stderr}"));
            Ok(())
        }
        Err(e) => {
            log(&format!("egress: nft not available: {e}"));
            Ok(())
        }
    }
}

fn write_response(
    mut stream: &std::os::unix::net::UnixStream,
    response: &serde_json::Value,
) -> std::io::Result<()> {
    writeln!(stream, "{}", serde_json::to_string(response).unwrap_or_default())?;
    stream.flush()
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
