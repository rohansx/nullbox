//! Process supervisor for nulld.
//!
//! Starts services in dependency order, monitors them, restarts on failure
//! with exponential backoff.

use crate::service::{self, RestartPolicy, ServiceDef};
use crate::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const MAX_BACKOFF: Duration = Duration::from_secs(30);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAIN_LOOP_INTERVAL: Duration = Duration::from_millis(500);

/// Runtime state for a managed service.
struct ManagedService {
    def: ServiceDef,
    pid: Option<u32>,
    state: ServiceState,
    restart_count: u32,
    last_start: Option<Instant>,
    next_restart: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceState {
    Stopped,
    Running,
    Failed,
    WaitingRestart,
}

pub struct Supervisor {
    services: HashMap<String, ManagedService>,
    start_order: Vec<String>,
}

impl Supervisor {
    pub fn new(defs: Vec<ServiceDef>) -> Self {
        let services = defs
            .into_iter()
            .map(|def| {
                let name = def.name.clone();
                let managed = ManagedService {
                    def,
                    pid: None,
                    state: ServiceState::Stopped,
                    restart_count: 0,
                    last_start: None,
                    next_restart: None,
                };
                (name, managed)
            })
            .collect();

        Self {
            services,
            start_order: Vec::new(),
        }
    }

    /// Start all services in dependency order.
    pub fn start_all(&mut self) -> Result<(), SupervisorError> {
        let defs: Vec<ServiceDef> =
            self.services.values().map(|m| m.def.clone()).collect();

        self.start_order = service::resolve_start_order(&defs)
            .map_err(|e| SupervisorError::DependencyError(e.to_string()))?;

        for name in &self.start_order.clone() {
            self.start_service(name)?;
        }

        Ok(())
    }

    /// Start a single service by name.
    fn start_service(&mut self, name: &str) -> Result<(), SupervisorError> {
        let svc = self
            .services
            .get(name)
            .ok_or_else(|| SupervisorError::UnknownService(name.to_string()))?;

        let binary = svc.def.binary.clone();
        let args = svc.def.args.clone();

        crate::log_kmsg(&format!("nulld: starting service '{name}' ({binary})"));

        match spawn_process(&binary, &args) {
            Ok(pid) => {
                let svc = self.services.get_mut(name).unwrap();
                svc.pid = Some(pid);
                svc.state = ServiceState::Running;
                svc.last_start = Some(Instant::now());
                crate::log_kmsg(&format!(
                    "nulld: service '{name}' started (PID {pid})"
                ));
                Ok(())
            }
            Err(e) => {
                let svc = self.services.get_mut(name).unwrap();
                svc.state = ServiceState::Failed;
                crate::log_kmsg(&format!(
                    "nulld: service '{name}' failed to start: {e}"
                ));
                Err(SupervisorError::SpawnFailed {
                    service: name.to_string(),
                    source: e,
                })
            }
        }
    }

    /// Main loop: reap children, handle restarts, check for shutdown.
    pub fn run_until_shutdown(
        &mut self,
        shutdown_flag: &AtomicBool,
    ) -> Result<(), SupervisorError> {
        loop {
            if signal::shutdown_requested(shutdown_flag) {
                crate::log_kmsg("nulld: shutdown signal received");
                return Ok(());
            }

            // Reap any exited children
            let reaped = signal::reap_children();
            for (pid, reason) in &reaped {
                self.handle_child_exit(*pid, reason);
            }

            // Process pending restarts
            self.process_restarts();

            std::thread::sleep(MAIN_LOOP_INTERVAL);
        }
    }

    /// Handle a child process exit.
    fn handle_child_exit(&mut self, pid: u32, reason: &signal::ExitReason) {
        let service_name = self
            .services
            .iter()
            .find(|(_, svc)| svc.pid == Some(pid))
            .map(|(name, _)| name.clone());

        if let Some(name) = service_name {
            crate::log_kmsg(&format!(
                "nulld: service '{name}' (PID {pid}) {reason}"
            ));

            let svc = self.services.get_mut(&name).unwrap();
            svc.pid = None;

            let should_restart = match svc.def.restart {
                RestartPolicy::Always => true,
                RestartPolicy::OnFailure => {
                    matches!(reason, signal::ExitReason::Exited(code) if *code != 0)
                        || matches!(reason, signal::ExitReason::Signaled(_))
                }
                RestartPolicy::Never => false,
            };

            if should_restart {
                let backoff = calculate_backoff(svc.restart_count);
                svc.state = ServiceState::WaitingRestart;
                svc.next_restart = Some(Instant::now() + backoff);
                svc.restart_count += 1;
                crate::log_kmsg(&format!(
                    "nulld: will restart '{name}' in {backoff:?} (attempt {})",
                    svc.restart_count
                ));
            } else {
                svc.state = ServiceState::Failed;
                crate::log_kmsg(&format!(
                    "nulld: service '{name}' will not be restarted (policy: {:?})",
                    svc.def.restart
                ));
            }
        }
    }

    /// Check for services waiting to be restarted and restart them if due.
    fn process_restarts(&mut self) {
        let now = Instant::now();
        let ready: Vec<String> = self
            .services
            .iter()
            .filter(|(_, svc)| {
                svc.state == ServiceState::WaitingRestart
                    && svc.next_restart.is_some_and(|t| now >= t)
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in ready {
            if let Err(e) = self.start_service(&name) {
                crate::log_kmsg(&format!(
                    "nulld: restart of '{name}' failed: {e}"
                ));
            }
        }
    }

    /// Stop all services in reverse dependency order.
    pub fn stop_all(&mut self) {
        let mut stop_order = self.start_order.clone();
        stop_order.reverse();

        for name in &stop_order {
            if let Some(svc) = self.services.get_mut(name) {
                if let Some(pid) = svc.pid.take() {
                    crate::log_kmsg(&format!(
                        "nulld: stopping service '{name}' (PID {pid})"
                    ));

                    // Send SIGTERM, give 5s, then SIGKILL
                    let _ = nix::sys::signal::kill(
                        Pid::from_raw(pid as i32),
                        Signal::SIGTERM,
                    );

                    let deadline = Instant::now() + Duration::from_secs(5);
                    loop {
                        if Instant::now() > deadline {
                            crate::log_kmsg(&format!(
                                "nulld: force killing '{name}' (PID {pid})"
                            ));
                            let _ = nix::sys::signal::kill(
                                Pid::from_raw(pid as i32),
                                Signal::SIGKILL,
                            );
                            break;
                        }
                        match nix::sys::wait::waitpid(
                            Pid::from_raw(pid as i32),
                            Some(nix::sys::wait::WaitPidFlag::WNOHANG),
                        ) {
                            Ok(nix::sys::wait::WaitStatus::StillAlive) => {
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            _ => break,
                        }
                    }

                    svc.state = ServiceState::Stopped;
                }
            }
        }
    }
}

/// Spawn a child process. Returns the PID.
fn spawn_process(binary: &str, args: &[String]) -> Result<u32, std::io::Error> {
    use std::process::Command;

    let child = Command::new(binary).args(args).spawn()?;

    Ok(child.id())
}

/// Exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s, 30s, ...
fn calculate_backoff(restart_count: u32) -> Duration {
    let secs = INITIAL_BACKOFF.as_secs() * 2u64.pow(restart_count);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

#[derive(Debug)]
pub enum SupervisorError {
    DependencyError(String),
    UnknownService(String),
    SpawnFailed {
        service: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DependencyError(e) => {
                write!(f, "dependency resolution failed: {e}")
            }
            Self::UnknownService(name) => {
                write!(f, "unknown service: {name}")
            }
            Self::SpawnFailed { service, source } => {
                write!(f, "failed to spawn '{service}': {source}")
            }
        }
    }
}

impl std::error::Error for SupervisorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_increases_exponentially() {
        assert_eq!(calculate_backoff(0), Duration::from_secs(1));
        assert_eq!(calculate_backoff(1), Duration::from_secs(2));
        assert_eq!(calculate_backoff(2), Duration::from_secs(4));
        assert_eq!(calculate_backoff(3), Duration::from_secs(8));
        assert_eq!(calculate_backoff(4), Duration::from_secs(16));
    }

    #[test]
    fn backoff_caps_at_max() {
        assert_eq!(calculate_backoff(5), MAX_BACKOFF);
        assert_eq!(calculate_backoff(10), MAX_BACKOFF);
    }

    #[test]
    fn supervisor_creates_from_defs() {
        let defs = vec![ServiceDef {
            name: "test".to_string(),
            binary: "/bin/true".to_string(),
            args: vec![],
            depends_on: vec![],
            restart: RestartPolicy::Always,
        }];

        let sup = Supervisor::new(defs);
        assert!(sup.services.contains_key("test"));
    }
}
