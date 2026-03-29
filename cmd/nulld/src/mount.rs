//! Filesystem mounting for nulld.
//!
//! Mounts the virtual filesystem hierarchy required by Linux userspace.
//! The root is already SquashFS (mounted by initramfs before pivot_root).

use nix::mount::{mount, MsFlags};
use std::fs;
use std::path::Path;

#[derive(Debug)]
struct MountEntry {
    source: &'static str,
    target: &'static str,
    fstype: &'static str,
    flags: MsFlags,
    data: Option<&'static str>,
}

/// All mounts required for NullBox userspace, in order.
const MOUNTS: &[MountEntry] = &[
    // procfs — process information
    MountEntry {
        source: "proc",
        target: "/proc",
        fstype: "proc",
        flags: MsFlags::MS_NOSUID
            .union(MsFlags::MS_NODEV)
            .union(MsFlags::MS_NOEXEC),
        data: None,
    },
    // sysfs — kernel/device information
    MountEntry {
        source: "sysfs",
        target: "/sys",
        fstype: "sysfs",
        flags: MsFlags::MS_NOSUID
            .union(MsFlags::MS_NODEV)
            .union(MsFlags::MS_NOEXEC),
        data: None,
    },
    // devtmpfs — device nodes
    MountEntry {
        source: "devtmpfs",
        target: "/dev",
        fstype: "devtmpfs",
        flags: MsFlags::MS_NOSUID,
        data: Some("mode=0755"),
    },
    // devpts — pseudo-terminal devices
    MountEntry {
        source: "devpts",
        target: "/dev/pts",
        fstype: "devpts",
        flags: MsFlags::MS_NOSUID.union(MsFlags::MS_NOEXEC),
        data: Some("mode=0620,ptmxmode=0666"),
    },
    // tmpfs for /dev/shm
    MountEntry {
        source: "tmpfs",
        target: "/dev/shm",
        fstype: "tmpfs",
        flags: MsFlags::MS_NOSUID.union(MsFlags::MS_NODEV),
        data: Some("mode=1777"),
    },
    // tmpfs for /tmp (ephemeral, wiped on reboot)
    MountEntry {
        source: "tmpfs",
        target: "/tmp",
        fstype: "tmpfs",
        flags: MsFlags::MS_NOSUID
            .union(MsFlags::MS_NODEV)
            .union(MsFlags::MS_NOEXEC),
        data: Some("mode=1777,size=256m"),
    },
    // tmpfs for /run (runtime state: sockets, PID files)
    MountEntry {
        source: "tmpfs",
        target: "/run",
        fstype: "tmpfs",
        flags: MsFlags::MS_NOSUID
            .union(MsFlags::MS_NODEV)
            .union(MsFlags::MS_NOEXEC),
        data: Some("mode=0755,size=64m"),
    },
    // tmpfs for /var (writable overlay — agent data, logs, ctxgraph state)
    // In future: overlay on a persistent partition. For v0.1: tmpfs.
    MountEntry {
        source: "tmpfs",
        target: "/var",
        fstype: "tmpfs",
        flags: MsFlags::MS_NOSUID.union(MsFlags::MS_NODEV),
        data: Some("mode=0755,size=512m"),
    },
];

/// Mount all virtual filesystems in order.
pub fn mount_all() -> Result<(), MountError> {
    for entry in MOUNTS {
        ensure_mountpoint(entry.target)?;

        mount(
            Some(entry.source),
            entry.target,
            Some(entry.fstype),
            entry.flags,
            entry.data,
        )
        .map_err(|e| MountError::MountFailed {
            target: entry.target,
            fstype: entry.fstype,
            source: e,
        })?;
    }

    // Create required subdirectories under writable mounts
    create_var_dirs()?;

    Ok(())
}

/// Ensure the mount point directory exists.
fn ensure_mountpoint(path: &str) -> Result<(), MountError> {
    if !Path::new(path).exists() {
        fs::create_dir_all(path).map_err(|e| MountError::MkdirFailed {
            path: path.to_string(),
            source: e,
        })?;
    }
    Ok(())
}

/// Create standard directory structure under /var.
fn create_var_dirs() -> Result<(), MountError> {
    let dirs = [
        "/var/log",
        "/var/run",
        "/var/run/ctxgraph",
        "/var/lib",
        "/var/lib/ctxgraph",
        "/agent",
    ];

    for dir in &dirs {
        fs::create_dir_all(dir).map_err(|e| MountError::MkdirFailed {
            path: dir.to_string(),
            source: e,
        })?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum MountError {
    MountFailed {
        target: &'static str,
        fstype: &'static str,
        source: nix::errno::Errno,
    },
    MkdirFailed {
        path: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MountFailed {
                target,
                fstype,
                source,
            } => {
                write!(f, "failed to mount {fstype} on {target}: {source}")
            }
            Self::MkdirFailed { path, source } => {
                write!(f, "failed to create directory {path}: {source}")
            }
        }
    }
}

impl std::error::Error for MountError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_entries_have_valid_targets() {
        for entry in MOUNTS {
            assert!(
                entry.target.starts_with('/'),
                "mount target must be absolute: {}",
                entry.target
            );
        }
    }

    #[test]
    fn mount_order_proc_before_sys() {
        let proc_idx = MOUNTS
            .iter()
            .position(|m| m.target == "/proc")
            .expect("/proc mount missing");
        let sys_idx = MOUNTS
            .iter()
            .position(|m| m.target == "/sys")
            .expect("/sys mount missing");
        assert!(
            proc_idx < sys_idx,
            "/proc must be mounted before /sys"
        );
    }

    #[test]
    fn mount_order_dev_before_devpts() {
        let dev_idx = MOUNTS
            .iter()
            .position(|m| m.target == "/dev")
            .expect("/dev mount missing");
        let pts_idx = MOUNTS
            .iter()
            .position(|m| m.target == "/dev/pts")
            .expect("/dev/pts mount missing");
        assert!(
            dev_idx < pts_idx,
            "/dev must be mounted before /dev/pts"
        );
    }
}
