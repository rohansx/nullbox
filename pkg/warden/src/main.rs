//! warden daemon — encrypted secret vault for NullBox agents.
//!
//! On startup:
//! 1. Loads (or creates) the encrypted vault
//! 2. Listens on a Unix socket for secret management commands
//!
//! Protocol (JSON-over-Unix-socket, one object per line):
//!   {"method":"get-secrets","agent":"researcher","credential_refs":["OPENAI_KEY"]}
//!   {"method":"set","key":"OPENAI_KEY","value":"sk-..."}
//!   {"method":"delete","key":"OPENAI_KEY"}
//!   {"method":"list"}

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use warden::vault::{self, Vault};

const SOCKET_PATH: &str = "/run/warden.sock";

fn main() {
    let result = run();
    if let Err(e) = result {
        log(&format!("warden: fatal: {e}"));
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log("warden: starting encrypted secret vault");

    // Ensure vault directory exists
    std::fs::create_dir_all("/vault").ok();

    let vault_path = Path::new(vault::VAULT_PATH);
    let mut vault = Vault::load(vault_path)?;

    if vault_path.exists() {
        log(&format!(
            "warden: loaded vault ({} secret(s))",
            vault.list_keys().len()
        ));
    } else {
        log("warden: created new empty vault");
    }

    // Clean up stale socket
    let sock_path = Path::new(SOCKET_PATH);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;
    log(&format!("warden: listening on {SOCKET_PATH}"));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                handle_connection(&stream, &mut vault);
            }
            Err(e) => {
                log(&format!("warden: accept error: {e}"));
            }
        }
    }

    Ok(())
}

fn handle_connection(stream: &std::os::unix::net::UnixStream, vault: &mut Vault) {
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
            "get-secrets" => handle_get_secrets(&request, vault),
            "set" => handle_set(&request, vault),
            "delete" => handle_delete(&request, vault),
            "list" => handle_list(vault),
            _ => serde_json::json!({"error": format!("unknown method: {method}")}),
        };

        let _ = write_response(stream, &response);
    }
}

fn handle_get_secrets(
    request: &serde_json::Value,
    vault: &Vault,
) -> serde_json::Value {
    let agent = request
        .get("agent")
        .and_then(|a| a.as_str())
        .unwrap_or("unknown");

    let refs: Vec<String> = request
        .get("credential_refs")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let secrets = vault.get(&refs);

    log(&format!(
        "warden: agent '{}' requested {} ref(s), got {} secret(s)",
        agent,
        refs.len(),
        secrets.len()
    ));

    serde_json::json!({"ok": true, "secrets": secrets})
}

fn handle_set(
    request: &serde_json::Value,
    vault: &mut Vault,
) -> serde_json::Value {
    let key = match request.get("key").and_then(|k| k.as_str()) {
        Some(k) if !k.is_empty() => k,
        _ => return serde_json::json!({"error": "missing or empty 'key' field"}),
    };

    let value = match request.get("value").and_then(|v| v.as_str()) {
        Some(v) => v,
        _ => return serde_json::json!({"error": "missing 'value' field"}),
    };

    vault.set(key, value);

    match vault.save(Path::new(vault::VAULT_PATH)) {
        Ok(()) => {
            log(&format!("warden: set secret '{key}'"));
            serde_json::json!({"ok": true})
        }
        Err(e) => {
            log(&format!("warden: failed to save vault: {e}"));
            serde_json::json!({"error": format!("save failed: {e}")})
        }
    }
}

fn handle_delete(
    request: &serde_json::Value,
    vault: &mut Vault,
) -> serde_json::Value {
    let key = match request.get("key").and_then(|k| k.as_str()) {
        Some(k) if !k.is_empty() => k,
        _ => return serde_json::json!({"error": "missing or empty 'key' field"}),
    };

    if !vault.delete(key) {
        return serde_json::json!({"error": format!("key '{key}' not found")});
    }

    match vault.save(Path::new(vault::VAULT_PATH)) {
        Ok(()) => {
            log(&format!("warden: deleted secret '{key}'"));
            serde_json::json!({"ok": true})
        }
        Err(e) => {
            log(&format!("warden: failed to save vault: {e}"));
            serde_json::json!({"error": format!("save failed: {e}")})
        }
    }
}

fn handle_list(vault: &Vault) -> serde_json::Value {
    serde_json::json!({"keys": vault.list_keys()})
}

fn write_response(
    mut stream: &std::os::unix::net::UnixStream,
    response: &serde_json::Value,
) -> std::io::Result<()> {
    writeln!(
        stream,
        "{}",
        serde_json::to_string(response).unwrap_or_default()
    )?;
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
