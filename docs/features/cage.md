# cage -- Per-Agent MicroVM Isolation

## Overview

cage is the microVM manager for NullBox. Each AI agent runs in its own KVM-backed virtual machine via libkrun, providing hardware-level isolation between agents. cage parses AGENT.toml manifests to determine resource limits and network permissions, spawns child processes that enter libkrun VMs, and manages the lifecycle of running agents through a Unix socket API.

## Architecture

### Dual-Mode Execution

cage operates in two modes within a single binary:

1. **Daemon mode** (default) -- Scans `/agent/*.toml` for manifests, starts a control socket, auto-starts agents with valid rootfs, and accepts lifecycle commands.
2. **VM runner mode** (`--run-vm <json>`) -- A child process that deserializes a VmConfig, calls the libkrun FFI to enter a VM, and never returns. This separation exists because `krun_start_enter()` blocks forever on success.

### VM Lifecycle

```
cage daemon
  |
  +-- fork child process (cage --run-vm '{"name":"agent-1",...}')
       |
       +-- krun_create_ctx()
       +-- krun_set_vm_config(vcpus, ram)
       +-- krun_set_root("/system/rootfs/agent-1")
       +-- krun_set_exec("/agent/bin/agent-1", env)
       +-- krun_start_enter()  // never returns
```

### Key Components

| File | Purpose |
|------|---------|
| `pkg/cage/src/main.rs` | Daemon mode, socket handler, auto-start logic |
| `pkg/cage/src/vm.rs` | VmManager: tracks running VMs, start/stop/list |
| `pkg/cage/src/krun.rs` | FFI bindings to libkrun (create, configure, enter) |
| `pkg/cage/src/manifest.rs` | AGENT.toml parser and validator |
| `pkg/cage/build.rs` | Build script for libkrun linking |

### libkrun FFI

cage uses minimal FFI bindings to libkrun:

| Function | Purpose |
|----------|---------|
| `krun_set_log_level` | Control libkrun verbosity (0=Off to 5=Trace) |
| `krun_create_ctx` | Create a new VM context |
| `krun_set_vm_config` | Set vCPU count and RAM |
| `krun_set_root` | Set the guest rootfs directory |
| `krun_set_workdir` | Set the guest working directory |
| `krun_set_exec` | Set the executable, args, and environment |
| `krun_set_port_map` | Configure TSI port mappings |
| `krun_start_enter` | Enter the VM (never returns on success) |
| `krun_free_ctx` | Clean up on failure |

### Resource Mapping

The AGENT.toml `max_cpu_percent` maps to vCPUs:

| CPU % | vCPUs |
|-------|-------|
| 1-25 | 1 |
| 26-50 | 2 |
| 51-75 | 3 |
| 76-100 | 4 |

Maximum 8 vCPUs. RAM is taken directly from `max_memory_mb` (minimum 64 MiB).

### Networking (TSI)

libkrun uses Transparent Socket Impersonation (TSI), which makes host TCP services accessible to the guest via `127.0.0.1`. Agents access ctxgraph at `127.0.0.1:9100` -- this transparently routes to the host's TCP listener.

### Environment Variables

Every agent VM receives:

| Variable | Value | Purpose |
|----------|-------|---------|
| `AGENT_NAME` | Agent name from manifest | Self-identification |
| `CTXGRAPH_PORT` | `9100` | Port for shared memory access |

## Configuration

### AGENT.toml Manifest

Agent manifests live at `/agent/<name>.toml` and declare the agent's requirements:

```toml
[agent]
name = "researcher"
version = "1.2.0"

[capabilities]
shell = false
credential_refs = ["OPENAI_KEY", "EXA_KEY"]
accelerator = "none"
max_cpu_percent = 40
max_memory_mb = 512
max_api_calls_per_hour = 200

[capabilities.network]
allow = ["api.perplexity.ai", "api.exa.ai", "api.openai.com"]

[capabilities.filesystem]
read = ["/data/research"]
write = ["/data/research/output"]

[tools]
send_email = { risk = "low" }
read_files = { risk = "low" }
write_files = { risk = "medium" }
delete_files = { risk = "critical" }
execute_payment = { risk = "critical" }
```

### Manifest Fields

| Section | Field | Type | Default | Description |
|---------|-------|------|---------|-------------|
| `agent` | `name` | string | required | Agent identifier (must be non-empty) |
| `agent` | `version` | string | `"0.0.0"` | Semantic version |
| `capabilities` | `shell` | bool | `false` | Whether the agent can spawn shells |
| `capabilities` | `credential_refs` | string[] | `[]` | Secret names the agent needs |
| `capabilities` | `accelerator` | string | `"none"` | Hardware acceleration |
| `capabilities` | `max_cpu_percent` | u32 | `25` | CPU limit (1-100) |
| `capabilities` | `max_memory_mb` | u32 | `256` | RAM limit in MiB |
| `capabilities` | `max_api_calls_per_hour` | u32 | `0` | API rate limit |
| `capabilities.network` | `allow` | string[] | `[]` | Allowed outbound domains |
| `capabilities.filesystem` | `read` | string[] | `[]` | Readable paths (absolute) |
| `capabilities.filesystem` | `write` | string[] | `[]` | Writable paths (absolute) |
| `tools` | `<name>` | `{risk}` | -- | Tool declarations with risk level |

Risk levels: `low`, `medium`, `high`, `critical`.

### Validation Rules

- `agent.name` must be non-empty
- `max_cpu_percent` must be 1-100
- `max_memory_mb` must be > 0
- Network allow entries must be valid hostnames (no spaces)
- Filesystem paths must be absolute (start with `/`)

### Rootfs Layout

Each agent's rootfs lives at `/system/rootfs/<agent-name>/` on the host (read-only SquashFS). Inside the VM, the agent binary is at `/agent/bin/<agent-name>`.

## API / Protocol

### Control Socket

**Path:** `/run/cage.sock` (Unix domain socket, newline-delimited JSON)

#### list

```json
-> {"method": "list"}
<- {"agents": [
     {"name": "researcher", "pid": 100, "vcpus": 2, "ram_mib": 512}
   ]}
```

#### start

```json
-> {"method": "start", "agent": "researcher"}
<- {"ok": true, "pid": 100}
```

#### stop

```json
-> {"method": "stop", "agent": "researcher"}
<- {"ok": true}
```

## Status

**Implemented:**
- libkrun FFI bindings (create, configure, enter)
- VmManager with start/stop/list operations
- AGENT.toml manifest parsing and validation
- Auto-start of agents with valid rootfs on daemon boot
- Child process forking (cage re-execs with --run-vm)
- TSI networking (agents reach host services via 127.0.0.1)
- Unix socket control API
- Test agent successfully boots in real KVM microVM

**Planned:**
- Per-agent egress integration (pass network.allow to egress daemon)
- Credential injection from Warden vault
- Resource monitoring and enforcement
- Agent health checks
- Hot-reload of agent manifests
