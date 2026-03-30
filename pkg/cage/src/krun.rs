//! Minimal FFI bindings to libkrun.
//!
//! Only the functions needed for NullBox agent VM lifecycle.
//! Full API: https://github.com/containers/libkrun/blob/main/include/libkrun.h

use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

#[link(name = "krun")]
unsafe extern "C" {
    fn krun_set_log_level(level: u32) -> i32;
    fn krun_create_ctx() -> i32;
    fn krun_free_ctx(ctx_id: u32) -> i32;
    fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
    fn krun_set_root(ctx_id: u32, root_path: *const c_char) -> i32;
    fn krun_set_workdir(ctx_id: u32, workdir_path: *const c_char) -> i32;
    fn krun_set_exec(
        ctx_id: u32,
        exec_path: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> i32;
    fn krun_set_port_map(ctx_id: u32, port_map: *const *const c_char) -> i32;
    fn krun_add_virtiofs(ctx_id: u32, tag: *const c_char, path: *const c_char) -> i32;
    fn krun_set_rlimits(ctx_id: u32, rlimits: *const *const c_char) -> i32;
    fn krun_set_console_output(ctx_id: u32, filepath: *const c_char) -> i32;
    fn krun_start_enter(ctx_id: u32) -> i32;
}

/// A host directory shared into the guest via virtio-fs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VirtiofsMount {
    pub tag: String,
    pub host_path: String,
}

/// Configuration for a single agent microVM.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VmConfig {
    pub name: String,
    pub vcpus: u8,
    pub ram_mib: u32,
    pub root_path: String,
    pub exec_path: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub port_map: Vec<String>,
    pub workdir: String,
    #[serde(default)]
    pub virtiofs_mounts: Vec<VirtiofsMount>,
    #[serde(default)]
    pub rlimits: Vec<String>,
    #[serde(default)]
    pub console_output: Option<String>,
}

/// Errors from libkrun operations.
#[derive(Debug, thiserror::Error)]
pub enum KrunError {
    #[error("krun_create_ctx failed: {0}")]
    CreateCtx(i32),
    #[error("krun_set_vm_config failed: {0}")]
    SetVmConfig(i32),
    #[error("krun_set_root failed: {0}")]
    SetRoot(i32),
    #[error("krun_set_exec failed: {0}")]
    SetExec(i32),
    #[error("krun_start_enter failed: {0}")]
    StartEnter(i32),
    #[error("nul byte in string: {0}")]
    NulError(#[from] std::ffi::NulError),
    #[error("krun_add_virtiofs failed: {0}")]
    AddVirtiofs(i32),
    #[error("krun_set_rlimits failed: {0}")]
    SetRlimits(i32),
    #[error("krun_set_console_output failed: {0}")]
    SetConsoleOutput(i32),
    #[error("libkrun call failed: {0}")]
    Other(String),
}

/// Set libkrun log level. Call once before creating any context.
/// 0=Off, 1=Error, 2=Warn, 3=Info, 4=Debug, 5=Trace
pub fn set_log_level(level: u32) {
    unsafe {
        krun_set_log_level(level);
    }
}

/// Create a VM context, configure it, and start it.
///
/// **WARNING**: `krun_start_enter` never returns on success.
/// This function should only be called from a forked child process.
pub fn run_vm(config: &VmConfig) -> Result<(), KrunError> {
    unsafe {
        // Create context
        let ctx = krun_create_ctx();
        if ctx < 0 {
            return Err(KrunError::CreateCtx(ctx));
        }
        let ctx = ctx as u32;

        // Set CPU and memory
        let ret = krun_set_vm_config(ctx, config.vcpus, config.ram_mib);
        if ret < 0 {
            krun_free_ctx(ctx);
            return Err(KrunError::SetVmConfig(ret));
        }

        // Set root filesystem
        let root = CString::new(config.root_path.as_str())?;
        let ret = krun_set_root(ctx, root.as_ptr());
        if ret < 0 {
            krun_free_ctx(ctx);
            return Err(KrunError::SetRoot(ret));
        }

        // Set working directory
        let workdir = CString::new(config.workdir.as_str())?;
        let ret = krun_set_workdir(ctx, workdir.as_ptr());
        if ret < 0 {
            krun_free_ctx(ctx);
            return Err(KrunError::Other("krun_set_workdir failed".into()));
        }

        // Set port map (TSI networking)
        if !config.port_map.is_empty() {
            let c_ports: Vec<CString> = config
                .port_map
                .iter()
                .map(|p| CString::new(p.as_str()))
                .collect::<Result<_, _>>()?;
            let mut port_ptrs: Vec<*const c_char> =
                c_ports.iter().map(|p| p.as_ptr()).collect();
            port_ptrs.push(ptr::null());
            let ret = krun_set_port_map(ctx, port_ptrs.as_ptr());
            if ret < 0 {
                krun_free_ctx(ctx);
                return Err(KrunError::Other("krun_set_port_map failed".into()));
            }
        }

        // Share host directories into guest via virtio-fs
        for mount in &config.virtiofs_mounts {
            let tag = CString::new(mount.tag.as_str())?;
            let path = CString::new(mount.host_path.as_str())?;
            let ret = krun_add_virtiofs(ctx, tag.as_ptr(), path.as_ptr());
            if ret < 0 {
                krun_free_ctx(ctx);
                return Err(KrunError::AddVirtiofs(ret));
            }
        }

        // Set resource limits (RLIMIT_AS, RLIMIT_NPROC, etc.)
        if !config.rlimits.is_empty() {
            let c_rlimits: Vec<CString> = config
                .rlimits
                .iter()
                .map(|r| CString::new(r.as_str()))
                .collect::<Result<_, _>>()?;
            let mut rlimit_ptrs: Vec<*const c_char> =
                c_rlimits.iter().map(|r| r.as_ptr()).collect();
            rlimit_ptrs.push(ptr::null());
            let ret = krun_set_rlimits(ctx, rlimit_ptrs.as_ptr());
            if ret < 0 {
                krun_free_ctx(ctx);
                return Err(KrunError::SetRlimits(ret));
            }
        }

        // Route console output to per-agent log file
        if let Some(ref console_path) = config.console_output {
            let path = CString::new(console_path.as_str())?;
            let ret = krun_set_console_output(ctx, path.as_ptr());
            if ret < 0 {
                krun_free_ctx(ctx);
                return Err(KrunError::SetConsoleOutput(ret));
            }
        }

        // Set executable, args, and environment
        let exec = CString::new(config.exec_path.as_str())?;

        let c_args: Vec<CString> = config
            .args
            .iter()
            .map(|a| CString::new(a.as_str()))
            .collect::<Result<_, _>>()?;
        let mut argv_ptrs: Vec<*const c_char> =
            c_args.iter().map(|a| a.as_ptr()).collect();
        argv_ptrs.push(ptr::null());

        let c_env: Vec<CString> = config
            .env
            .iter()
            .map(|e| CString::new(e.as_str()))
            .collect::<Result<_, _>>()?;
        let mut envp_ptrs: Vec<*const c_char> =
            c_env.iter().map(|e| e.as_ptr()).collect();
        envp_ptrs.push(ptr::null());

        let ret = krun_set_exec(
            ctx,
            exec.as_ptr(),
            argv_ptrs.as_ptr(),
            envp_ptrs.as_ptr(),
        );
        if ret < 0 {
            krun_free_ctx(ctx);
            return Err(KrunError::SetExec(ret));
        }

        // Start VM — this call never returns on success
        let ret = krun_start_enter(ctx);
        Err(KrunError::StartEnter(ret))
    }
}

/// Sanitize a filesystem path into a valid virtiofs tag.
///
/// `/data/research` → `data_research`
/// `/var/lib/my-data` → `var_lib_my_data`
pub fn path_to_virtiofs_tag(path: &str) -> String {
    path.trim_start_matches('/')
        .replace(['/', '-', '.'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vmconfig_roundtrip_with_new_fields() {
        let config = VmConfig {
            name: "test".to_string(),
            vcpus: 2,
            ram_mib: 512,
            root_path: "/rootfs".to_string(),
            exec_path: "/agent/bin/test".to_string(),
            args: vec![],
            env: vec!["FOO=bar".to_string()],
            port_map: vec![],
            workdir: "/".to_string(),
            virtiofs_mounts: vec![
                VirtiofsMount {
                    tag: "data_research".to_string(),
                    host_path: "/var/lib/cage/test/data/research".to_string(),
                },
            ],
            rlimits: vec![
                "RLIMIT_AS=536870912:536870912".to_string(),
                "RLIMIT_NPROC=64:64".to_string(),
            ],
            console_output: Some("/var/log/cage/test.log".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: VmConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.virtiofs_mounts.len(), 1);
        assert_eq!(deserialized.virtiofs_mounts[0].tag, "data_research");
        assert_eq!(deserialized.rlimits.len(), 2);
        assert_eq!(
            deserialized.console_output,
            Some("/var/log/cage/test.log".to_string())
        );
    }

    #[test]
    fn vmconfig_backwards_compatible() {
        // Old-style JSON without new fields should deserialize with defaults
        let json = r#"{
            "name": "old",
            "vcpus": 1,
            "ram_mib": 256,
            "root_path": "/rootfs",
            "exec_path": "/bin/agent",
            "args": [],
            "env": [],
            "port_map": [],
            "workdir": "/"
        }"#;

        let config: VmConfig = serde_json::from_str(json).unwrap();
        assert!(config.virtiofs_mounts.is_empty());
        assert!(config.rlimits.is_empty());
        assert!(config.console_output.is_none());
    }

    #[test]
    fn path_to_tag_sanitization() {
        assert_eq!(path_to_virtiofs_tag("/data/research"), "data_research");
        assert_eq!(
            path_to_virtiofs_tag("/var/lib/my-data"),
            "var_lib_my_data"
        );
        assert_eq!(path_to_virtiofs_tag("/data"), "data");
        assert_eq!(
            path_to_virtiofs_tag("/data/.hidden/output"),
            "data__hidden_output"
        );
    }
}
