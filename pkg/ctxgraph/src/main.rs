//! ctxgraph daemon — content-addressed shared agent memory for NullBox.
//!
//! On startup:
//! 1. Opens (or creates) the SQLite database at /var/lib/ctxgraph/db.sqlite
//! 2. Listens on a TCP port (for agents inside microVMs via TSI)
//! 3. Listens on a Unix socket (for host-side services)
//! 4. Handles JSON protocol: write, read, query, history

use ctxgraph::entry::Entry;
use ctxgraph::store::Store;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

const SOCKET_PATH: &str = "/run/ctxgraph.sock";
const TCP_PORT: u16 = 9100;
const DB_DIR: &str = "/var/lib/ctxgraph";
const DB_PATH: &str = "/var/lib/ctxgraph/db.sqlite";

fn main() {
    let result = run();
    if let Err(e) = result {
        log(&format!("ctxgraph: fatal: {e}"));
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log("ctxgraph: starting shared agent memory");

    // Ensure database directory exists
    std::fs::create_dir_all(DB_DIR)?;

    // Open database
    let store = Arc::new(Mutex::new(Store::open(Path::new(DB_PATH))?));
    log("ctxgraph: database initialized");

    // Clean up stale Unix socket
    let sock_path = Path::new(SOCKET_PATH);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }

    // Start TCP listener for agents (TSI-accessible)
    let tcp_store = Arc::clone(&store);
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", TCP_PORT)) {
            Ok(l) => l,
            Err(e) => {
                log(&format!("ctxgraph: TCP bind failed: {e}"));
                return;
            }
        };
        log(&format!("ctxgraph: TCP listening on port {TCP_PORT}"));

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let store = Arc::clone(&tcp_store);
                    // Handle each connection in a thread to avoid blocking
                    std::thread::spawn(move || {
                        handle_tcp_connection(&stream, &store);
                    });
                }
                Err(e) => {
                    log(&format!("ctxgraph: TCP accept error: {e}"));
                }
            }
        }
    });

    // Unix socket listener for host services
    let listener = UnixListener::bind(SOCKET_PATH)?;
    log(&format!("ctxgraph: listening on {SOCKET_PATH}"));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    handle_unix_connection(&stream, &store);
                });
            }
            Err(e) => {
                log(&format!("ctxgraph: accept error: {e}"));
            }
        }
    }

    Ok(())
}

/// Handle a single TCP connection.
fn handle_tcp_connection(stream: &std::net::TcpStream, store: &Arc<Mutex<Store>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(5000)));
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.is_empty() => l,
            _ => return,
        };
        let response = dispatch_request(&line, store);
        let mut writer = stream;
        let _ = writeln!(writer, "{}", serde_json::to_string(&response).unwrap_or_default());
        let _ = writer.flush();
    }
}

/// Handle a single Unix socket connection.
fn handle_unix_connection(stream: &std::os::unix::net::UnixStream, store: &Arc<Mutex<Store>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(5000)));
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.is_empty() => l,
            _ => return,
        };
        let response = dispatch_request(&line, store);
        let mut writer = stream;
        let _ = writeln!(writer, "{}", serde_json::to_string(&response).unwrap_or_default());
        let _ = writer.flush();
    }
}

/// Parse a JSON request and dispatch to the appropriate handler.
fn dispatch_request(line: &str, store: &Arc<Mutex<Store>>) -> serde_json::Value {
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return serde_json::json!({"error": format!("invalid JSON: {e}")}),
    };

    let method = request
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");

    match method {
        "write" => handle_write(store, &request),
        "read" => handle_read(store, &request),
        "query" => handle_query(store, &request),
        "history" => handle_history(store, &request),
        _ => serde_json::json!({"error": format!("unknown method: {method}")}),
    }
}

fn handle_write(
    store: &Arc<Mutex<Store>>,
    request: &serde_json::Value,
) -> serde_json::Value {
    let agent_id = request.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
    let key = request.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = request.get("value").unwrap_or(&serde_json::Value::Null);

    if agent_id.is_empty() || key.is_empty() {
        return serde_json::json!({"error": "agent_id and key are required"});
    }

    let store = store.lock().unwrap();
    match store.write(agent_id, key, value) {
        Ok(hash) => serde_json::json!({"ok": true, "hash": hash}),
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn handle_read(
    store: &Arc<Mutex<Store>>,
    request: &serde_json::Value,
) -> serde_json::Value {
    let hash = request.get("hash").and_then(|v| v.as_str()).unwrap_or("");
    if hash.is_empty() {
        return serde_json::json!({"error": "hash is required"});
    }

    let store = store.lock().unwrap();
    match store.read(hash) {
        Ok(Some(entry)) => entry_to_json(&entry),
        Ok(None) => serde_json::json!({"error": "not found"}),
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn handle_query(
    store: &Arc<Mutex<Store>>,
    request: &serde_json::Value,
) -> serde_json::Value {
    let prefix = request.get("prefix").and_then(|v| v.as_str()).unwrap_or("");

    let store = store.lock().unwrap();
    match store.query_by_prefix(prefix) {
        Ok(entries) => {
            let items: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
            serde_json::json!({"entries": items})
        }
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn handle_history(
    store: &Arc<Mutex<Store>>,
    request: &serde_json::Value,
) -> serde_json::Value {
    let key = request.get("key").and_then(|v| v.as_str()).unwrap_or("");
    if key.is_empty() {
        return serde_json::json!({"error": "key is required"});
    }

    let store = store.lock().unwrap();
    match store.history(key) {
        Ok(entries) => {
            let items: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
            serde_json::json!({"entries": items})
        }
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

fn entry_to_json(entry: &Entry) -> serde_json::Value {
    serde_json::json!({
        "hash": entry.hash,
        "agent_id": entry.agent_id,
        "key": entry.key,
        "value": entry.value,
        "timestamp": entry.timestamp,
    })
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
