//! test-agent — minimal binary for verifying microVM isolation.
//!
//! Runs as PID 1 inside a libkrun microVM. Demonstrates:
//! 1. VM boots and runs a static binary
//! 2. Agent can communicate with host services via TSI networking
//! 3. Agent writes to ctxgraph shared memory

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

fn main() {
    let name = std::env::var("AGENT_NAME").unwrap_or_else(|_| "unknown".into());
    let ctxgraph_port = std::env::var("CTXGRAPH_PORT").unwrap_or_else(|_| "9100".into());
    let ctxgraph_addr = format!("127.0.0.1:{ctxgraph_port}");

    println!("test-agent: booted inside microVM (AGENT_NAME={name})");

    // Try to write to ctxgraph via TSI with retries
    println!("test-agent: connecting to ctxgraph at {ctxgraph_addr}");
    let mut connected = false;
    for attempt in 1..=5 {
        // Increasing backoff: 1s, 2s, 3s, 4s, 5s
        std::thread::sleep(std::time::Duration::from_secs(attempt));

        match write_to_ctxgraph(&ctxgraph_addr, &name) {
            Ok(hash) => {
                println!("test-agent: wrote to ctxgraph (attempt {attempt}), hash={hash}");
                connected = true;
                break;
            }
            Err(e) => {
                println!("test-agent: ctxgraph attempt {attempt}/5 failed: {e}");
            }
        }
    }

    if !connected {
        println!("test-agent: could not reach ctxgraph (TSI may not work in nested KVM)");
    }

    // Heartbeat loop
    let mut tick: u64 = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        tick += 1;
        println!("test-agent: heartbeat {tick}");
    }
}

fn write_to_ctxgraph(addr: &str, agent_name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let stream = TcpStream::connect_timeout(
        &addr.parse()?,
        std::time::Duration::from_secs(3),
    )?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut stream = stream;

    // Write an entry to ctxgraph
    let request = format!(
        r#"{{"method":"write","agent_id":"{}","key":"agent.status","value":"booted"}}"#,
        agent_name
    );
    writeln!(stream, "{request}")?;
    stream.flush()?;

    // Read response
    let reader = BufReader::new(&stream);
    let line = reader
        .lines()
        .next()
        .ok_or("no response")??;

    // Parse hash from response
    let resp: serde_json::Value = serde_json::from_str(&line)?;
    if let Some(hash) = resp.get("hash").and_then(|h| h.as_str()) {
        Ok(hash.to_string())
    } else if let Some(err) = resp.get("error").and_then(|e| e.as_str()) {
        Err(err.into())
    } else {
        Err("unexpected response".into())
    }
}
