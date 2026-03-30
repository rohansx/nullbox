# cage -- Per-Agent MicroVM Isolation

## Overview

cage is the microVM manager for NullBox. Each AI agent runs in its own KVM-backed virtual machine via libkrun, providing hardware-level isolation between agents. cage parses AGENT.toml manifests to determine resource limits and network permissions, spawns child processes that enter libkrun VMs, and manages the lifecycle of running agents through a Unix socket API.

## Architecture

### Dual-Mode Execution

cage operates in two modes within a single binary:

1. **Daemon mode** (default) -- Scans `/agent/*.toml` for manifests, starts a control socket, auto-starts agents with valid rootfs, reaps dead children, and accepts lifecycle commands.
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
       +-- krun_add_virtiofs("data_corpus_ro", "/var/lib/cage/agent-1/data/corpus")
       +-- krun_set_rlimits(["RLIMIT_AS=...", "RLIMIT_NPROC=64:64"])
       +-- krun_set_console_output("/var/log/cage/agent-1.log")
       +-- krun_set_exec("/agent/bin/agent-1", env)
       +-- krun_start_enter()  // never returns
```

### Main Loop

The daemon runs a non-blocking accept loop that interleaves socket command handling with child process reaping:

1. **Reap children** -- `waitpid(-1, WNOHANG)` collects exit statuses of dead VM processes
2. **Accept connections** -- Non-blocking accept on `/run/cage.sock`
3. **Auto-restart** -- Crashed agents (non-zero exit) are restarted after a 2-second delay

### Key Components

| File | Purpose |
|------|---------|
| `pkg/cage/src/main.rs` | Daemon mode, socket handler, SIGCHLD reaping, auto-restart |
| `pkg/cage/src/vm.rs` | VmManager: tracks running VMs, builds virtiofs/rlimits/console config |
| `pkg/cage/src/krun.rs` | FFI bindings to libkrun (create, configure, virtiofs, rlimits, enter) |
| `pkg/cage/src/manifest.rs` | AGENT.toml parser and validator |
| `pkg/cage/build.rs` | Build script for libkrun linking |

### libkrun FFI

| Function | Purpose |
|----------|---------|
| `krun_set_log_level` | Control libkrun verbosity (0=Off to 5=Trace) |
| `krun_create_ctx` | Create a new VM context |
| `krun_set_vm_config` | Set vCPU count and RAM |
| `krun_set_root` | Set the guest rootfs directory (virtiofs) |
| `krun_set_workdir` | Set the guest working directory |
| `krun_set_exec` | Set the executable, args, and environment |
| `krun_set_port_map` | Configure TSI port mappings |
| `krun_add_virtiofs` | Share a host directory into the guest |
| `krun_set_rlimits` | Set per-process resource limits inside the guest |
| `krun_set_console_output` | Route guest console to a host file |
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

### Resource Limits (rlimits)

Cage sets per-VM rlimits that libkrun applies inside the guest:

| Rlimit | Source | Purpose |
|--------|--------|---------|
| `RLIMIT_AS` | `max_memory_mb * 1024 * 1024` | Cap virtual address space |
| `RLIMIT_NPROC` | `64` (hardcoded) | Cap spawnable processes |

These are enforced inside the guest kernel by libkrun's init process.

### Virtiofs Filesystem Sharing

AGENT.toml `capabilities.filesystem.read` and `.write` paths are shared into the guest via virtio-fs:

1. For each declared path, cage creates a host directory at `/var/lib/cage/<agent>/<path>`
2. The path is converted to a virtiofs tag: `/data/research` → `data_research_ro` (read) or `data_research_rw` (write)
3. `krun_add_virtiofs(tag, host_path)` shares the directory into the guest
4. The guest rootfs must contain empty mount point directories matching the declared paths

Host directory layout:
```
/var/lib/cage/
  researcher/
    data/
      corpus/    ← virtiofs "data_corpus_ro" (read-only share)
      output/    ← virtiofs "data_output_rw" (read-write share)
```

Note: `/var/lib/cage/` is on tmpfs in v0.1, so agent data does not survive reboots unless a persistent partition is configured.

### Console Output

Each agent VM's console output is routed to `/var/log/cage/<agent-name>.log` via `krun_set_console_output`. This captures guest stdout/stderr and kernel messages.

To view agent logs:
```
cat /var/log/cage/researcher.log
```

### VM Exit Handling

When a VM child process exits, the daemon:

1. Calls `waitpid(-1, WNOHANG)` to collect the exit status
2. Removes the VM from tracking
3. Notifies egress to revoke network access
4. If the exit was non-zero (crash), restarts the agent after a 2-second delay
5. If the exit was zero (clean shutdown), does not restart

Exit status 0 = clean shutdown (no restart). Non-zero = crash (auto-restart).

### Networking (TSI)

libkrun uses Transparent Socket Impersonation (TSI), which makes host TCP services accessible to the guest via `127.0.0.1`. Agents access ctxgraph at `127.0.0.1:9100` -- this transparently routes to the host's TCP listener.

With TSI, agent network traffic appears to originate from the cage child process on the host. Egress nftables rules control which destinations are reachable.

### Environment Variables

Every agent VM receives:

| Variable | Value | Purpose |
|----------|-------|---------|
| `AGENT_NAME` | Agent name from manifest | Self-identification |
| `CTXGRAPH_PORT` | `9100` | Port for shared memory access |
| `<secret>` | From Warden vault | Credentials declared in `credential_refs` |

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
read = ["/data/corpus"]
write = ["/data/output"]

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
| `capabilities` | `max_cpu_percent` | u32 | `25` | CPU limit (1-100), maps to vCPUs |
| `capabilities` | `max_memory_mb` | u32 | `256` | RAM limit in MiB, also sets RLIMIT_AS |
| `capabilities` | `max_api_calls_per_hour` | u32 | `0` | API rate limit |
| `capabilities.network` | `allow` | string[] | `[]` | Allowed outbound domains |
| `capabilities.filesystem` | `read` | string[] | `[]` | Read-only virtiofs shares (absolute paths) |
| `capabilities.filesystem` | `write` | string[] | `[]` | Read-write virtiofs shares (absolute paths) |
| `tools` | `<name>` | `{risk}` | -- | Tool declarations with risk level |

Risk levels: `low`, `medium`, `high`, `critical`.

### Validation Rules

- `agent.name` must be non-empty
- `max_cpu_percent` must be 1-100
- `max_memory_mb` must be > 0
- Network allow entries must be valid hostnames (no spaces)
- Filesystem paths must be absolute (start with `/`)

### Guest Rootfs Layout

Each agent's rootfs lives at `/system/rootfs/<agent-name>/` on the host (read-only SquashFS):

```
/system/rootfs/researcher/
  agent/bin/researcher    ← agent binary (musl static)
  data/                   ← mount point for virtiofs
  data/output/            ← mount point for virtiofs
  etc/resolv.conf         ← DNS config
  etc/hostname            ← agent name
  proc/                   ← mounted by libkrun init
  sys/                    ← mounted by libkrun init
  dev/                    ← mounted by libkrun init
  tmp/
  run/
```

libkrun's built-in init (from libkrunfw) handles mounting /proc, /sys, /dev inside the guest before exec'ing the agent binary.

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

Cage fetches secrets from Warden and notifies Egress before returning.

#### stop

```json
-> {"method": "stop", "agent": "researcher"}
<- {"ok": true}
```

Sends SIGTERM to the VM process and notifies Egress to revoke network access.

### Service Integration

**Warden** (`/run/warden.sock`): Cage requests secrets matching `credential_refs` on agent start. Secrets are injected as environment variables into the VM.

**Egress** (`/run/egress.sock`): Cage sends `add-agent` on start (with declared domains) and `remove-agent` on stop.

## Status

**Implemented:**
- libkrun FFI bindings (create, configure, virtiofs, rlimits, console, enter)
- VmManager with start/stop/list operations
- AGENT.toml manifest parsing and validation (6 unit tests)
- Auto-start of agents with valid rootfs on daemon boot
- Child process forking (cage re-execs with --run-vm)
- TSI networking (agents reach host services via 127.0.0.1)
- Unix socket control API
- Per-agent virtiofs filesystem sharing from AGENT.toml declarations
- Per-agent resource limits (RLIMIT_AS from max_memory_mb, RLIMIT_NPROC=64)
- Per-agent console output to /var/log/cage/<name>.log
- SIGCHLD reaping with exit status tracking
- Auto-restart of crashed agents (non-zero exit, 2-second delay)
- Warden credential injection (secrets → env vars)
- Egress integration (add-agent/remove-agent on start/stop)
- VmConfig serialization tests (3 tests), rlimit tests (2 tests), virtiofs tests (1 test), exit handling tests (3 tests)

**Planned:**
- Exponential backoff on repeated crash restarts
- Agent health checks (periodic liveness probes)
- Hot-reload of agent manifests
- Pause/resume VM support
- Seccomp profiles per agent
