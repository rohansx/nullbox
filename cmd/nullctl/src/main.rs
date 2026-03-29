//! nullctl — CLI for NullBox.
//!
//! Communicates with nulld via Unix socket to manage agents and services.
//! For v0.1: minimal command set (agent start/stop, status).

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process;

const NULLD_SOCKET: &str = "/run/nulld.sock";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        process::exit(1);
    }

    let result = match args[0].as_str() {
        "agent" => handle_agent(&args[1..]),
        "status" => handle_status(),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("nullctl: unknown command '{other}'");
            print_usage();
            process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("nullctl: error: {e}");
        process::exit(1);
    }
}

fn handle_agent(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("nullctl agent: missing subcommand");
        eprintln!("usage: nullctl agent <start|stop|pause|resume> <name>");
        return Err("missing subcommand".into());
    }

    let subcommand = &args[0];
    let name = args
        .get(1)
        .ok_or("missing agent name")?;

    let command = serde_json::json!({
        "type": "agent",
        "action": subcommand,
        "name": name,
    });

    send_command(&command)
}

fn handle_status() -> Result<(), Box<dyn std::error::Error>> {
    let command = serde_json::json!({
        "type": "status",
    });

    send_command(&command)
}

fn send_command(
    command: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(NULLD_SOCKET).map_err(|e| {
        format!("cannot connect to nulld at {NULLD_SOCKET}: {e}")
    })?;

    let json = serde_json::to_string(command)?;
    writeln!(stream, "{json}")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let reader = BufReader::new(&stream);
    for line in reader.lines() {
        println!("{}", line?);
    }

    Ok(())
}

fn print_usage() {
    eprintln!("nullctl — NullBox CLI");
    eprintln!();
    eprintln!("usage:");
    eprintln!("  nullctl agent start <name>    Start an agent");
    eprintln!("  nullctl agent stop <name>     Stop an agent");
    eprintln!("  nullctl agent pause <name>    Pause an agent");
    eprintln!("  nullctl agent resume <name>   Resume an agent");
    eprintln!("  nullctl status                Show system status");
}
