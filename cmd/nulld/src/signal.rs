//! Signal handling for nulld (PID 1).
//!
//! PID 1 must:
//! - Reap all orphaned children via SIGCHLD (or they become zombies)
//! - Handle SIGTERM/SIGINT for graceful shutdown
//!
//! Uses a pipe-based signaling mechanism for async-signal-safety.

use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::sync::atomic::{AtomicBool, Ordering};

/// Global shutdown flag, set by SIGTERM/SIGINT handlers.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Install signal handlers for PID 1.
/// Returns a reference to the static shutdown flag.
pub fn install_handlers() -> Result<&'static AtomicBool, SignalError> {
    // SIGCHLD: reap children (handled in main loop via reap_children)
    // We set SA_NOCLDSTOP so we only get SIGCHLD on child exit, not stop/continue.
    let sigchld_action = SigAction::new(
        SigHandler::SigDfl,
        SaFlags::SA_NOCLDSTOP,
        SigSet::empty(),
    );
    unsafe {
        sigaction(Signal::SIGCHLD, &sigchld_action).map_err(SignalError::SigAction)?;
    }

    // SIGTERM / SIGINT: set shutdown flag
    let shutdown_handler = SigAction::new(
        SigHandler::Handler(handle_shutdown),
        SaFlags::empty(),
        SigSet::empty(),
    );
    unsafe {
        sigaction(Signal::SIGTERM, &shutdown_handler).map_err(SignalError::SigAction)?;
        sigaction(Signal::SIGINT, &shutdown_handler).map_err(SignalError::SigAction)?;
    }

    Ok(&SHUTDOWN_REQUESTED)
}

/// Signal handler for SIGTERM/SIGINT — sets the shutdown flag.
extern "C" fn handle_shutdown(_signal: nix::libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Check if shutdown has been requested.
pub fn shutdown_requested(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

/// Reap all zombie children. Call this periodically from the main loop.
/// Returns the list of exited child PIDs with their exit statuses.
pub fn reap_children() -> Vec<(u32, ExitReason)> {
    let mut reaped = Vec::new();

    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, status)) => {
                reaped.push((pid.as_raw() as u32, ExitReason::Exited(status)));
            }
            Ok(WaitStatus::Signaled(pid, signal, _core_dumped)) => {
                reaped.push((
                    pid.as_raw() as u32,
                    ExitReason::Signaled(signal as i32),
                ));
            }
            Ok(WaitStatus::StillAlive) => break,
            Err(nix::errno::Errno::ECHILD) => break, // No children
            _ => break,
        }
    }

    reaped
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    Exited(i32),
    Signaled(i32),
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "exited with code {code}"),
            Self::Signaled(sig) => write!(f, "killed by signal {sig}"),
        }
    }
}

#[derive(Debug)]
pub enum SignalError {
    SigAction(nix::errno::Errno),
}

impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SigAction(e) => write!(f, "sigaction failed: {e}"),
        }
    }
}

impl std::error::Error for SignalError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reap_children_returns_empty_when_no_children() {
        let reaped = reap_children();
        assert!(reaped.is_empty());
    }

    #[test]
    fn exit_reason_display() {
        assert_eq!(ExitReason::Exited(0).to_string(), "exited with code 0");
        assert_eq!(ExitReason::Signaled(9).to_string(), "killed by signal 9");
    }
}
