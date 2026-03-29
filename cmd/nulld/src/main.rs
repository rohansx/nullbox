//! nulld — PID 1 for NullBox
//!
//! Replaces systemd. Mounts filesystems, starts services in dependency order,
//! reaps orphaned children, handles shutdown.

mod config;
mod mount;
mod service;
mod signal;
mod supervisor;

use nix::libc;
use std::process;

fn main() {
    // PID 1 must never panic — a panic here causes kernel panic.
    // catch_unwind lets us log the error and enter a recovery hold.
    let result = std::panic::catch_unwind(run);

    match result {
        Ok(Ok(())) => {
            log_kmsg("nulld: clean shutdown");
        }
        Ok(Err(e)) => {
            log_kmsg(&format!("nulld: fatal error: {e}"));
            recovery_hold();
        }
        Err(_panic) => {
            log_kmsg("nulld: PANIC caught — entering recovery hold");
            recovery_hold();
        }
    }

    // PID 1 exiting means kernel panic. Reboot instead.
    unsafe {
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Verify we are PID 1
    let pid = process::id();
    if pid != 1 {
        return Err(format!("nulld must run as PID 1, got PID {pid}").into());
    }

    log_kmsg("nulld: starting (PID 1)");

    // Phase 1: Mount virtual filesystems
    log_kmsg("nulld: mounting filesystems");
    mount::mount_all()?;

    // Phase 2: Set up signal handling (SIGCHLD reaping, SIGTERM shutdown)
    log_kmsg("nulld: installing signal handlers");
    let shutdown_flag = signal::install_handlers()?;

    // Phase 3: Load service configuration
    log_kmsg("nulld: loading service configuration");
    let services = config::load_services()?;

    // Phase 4: Start services in dependency order
    log_kmsg("nulld: starting services");
    let mut sup = supervisor::Supervisor::new(services);
    sup.start_all()?;

    log_kmsg("nulld: all services started — entering main loop");

    // Phase 5: Main loop — reap children, check health, handle shutdown
    sup.run_until_shutdown(&shutdown_flag)?;

    // Phase 6: Stop services in reverse dependency order
    log_kmsg("nulld: shutting down services");
    sup.stop_all();

    log_kmsg("nulld: shutdown complete");
    Ok(())
}

/// Log to /dev/kmsg (kernel log buffer, visible on serial console).
/// Falls back to stderr if /dev/kmsg is not available.
fn log_kmsg(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/kmsg")
    {
        let _ = writeln!(f, "{msg}");
    } else {
        eprintln!("{msg}");
    }
}

/// Hold the system in a recovery state instead of panicking.
/// Sleeps forever so the kernel doesn't panic due to PID 1 exit.
fn recovery_hold() -> ! {
    log_kmsg("nulld: RECOVERY HOLD — system stopped. Connect via serial to debug.");
    loop {
        // Sleep for a long time. We cannot exit.
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
