# Cage — Per-Agent MicroVM Isolation

> Each agent gets its own kernel. Not a container — a microVM with hardware-level isolation.

**Layer:** NullBox Layer 2
**Technology:** libkrun — library-based KVM virtualization

---

## Why Cage Exists

Containers share the host kernel. A kernel exploit in a container reaches the host. A microVM has its own kernel — compromise it, and you have that kernel only. The host is unreachable.

Current state:
- **microsandbox** provides microVM isolation but runs as a daemon on a host OS — it's a tool, not an OS primitive
- **Docker** shares the host kernel — container escapes are a well-documented attack class
- **E2B** is a cloud sandbox — no local option
- **No agent runtime** provides hardware-isolated per-agent execution at the OS level

Cage makes microVM isolation a first-class OS primitive. Every agent runs in its own VM by default. Not optional. Not configurable. Structural.

---

## How Cage Works

### Capability Manifest (AGENT.toml)

Every agent declares exactly what it needs:

```toml
[agent]
name = "researcher"
version = "1.2.0"

[capabilities]
network.allow = ["api.perplexity.ai", "api.exa.ai", "api.openai.com"]
filesystem.read = ["/data/research"]
filesystem.write = ["/data/research/output"]
shell = false
credential_refs = ["OPENAI_KEY", "EXA_KEY"]
accelerator = "none"
max_cpu_percent = 40
max_memory_mb = 512
max_api_calls_per_hour = 200

[tools]
send_email = { risk = "low" }
read_files = { risk = "low" }
write_files = { risk = "medium" }
delete_files = { risk = "critical" }
execute_payment = { risk = "critical" }
```

### Enforcement at the Hypervisor Level

Cage reads the manifest and creates a microVM with **exactly** these permissions:

- **Network:** The microVM's virtual NIC only routes to declared domains. Not software policy. Network topology.
- **Filesystem:** Only declared paths are mounted into the VM. No access to host filesystem.
- **CPU/Memory:** Hard limits at the hypervisor. Agent cannot exceed declared quota.
- **Credentials:** Only declared `credential_refs` are available via Warden.
- **Accelerator:** GPU/NPU pinned to this specific VM if declared.
- **Shell:** If `shell = false`, no shell binary exists in the VM.

### MicroVM Boot Time

libkrun VMs boot in **under 200ms**. Fast enough for on-demand agent spawning — no need to keep VMs running when agents are idle.

---

## Cage Lifecycle

```
nullctl agent start researcher
  -> Cage reads AGENT.toml
  -> Creates microVM with declared capabilities
  -> Boots in <200ms
  -> Agent process starts inside VM
  -> All traffic routes through Warden -> Sentinel -> CloakPipe pipeline

nullctl agent pause researcher
  -> Cage suspends VM (state preserved in memory, CPU released)

nullctl agent resume researcher
  -> Cage resumes VM from suspended state

nullctl agent stop researcher
  -> Cage reclaims VM (destroyed, ephemeral data wiped)
```

---

## Why MicroVMs Beat Containers

| Property | Containers (Docker) | MicroVMs (Cage) |
|---|---|---|
| Kernel | Shared with host | Own kernel per agent |
| Kernel exploit | Reaches host | Contained in VM |
| Network isolation | Software rules (iptables) | Hardware virtual NIC |
| Filesystem isolation | Namespaces (bypassable) | Separate rootfs per VM |
| Boot time | ~500ms | <200ms (libkrun) |
| Memory overhead | ~10MB | ~30MB |
| Security boundary | OS-level | Hardware-level (KVM) |

The 20MB memory overhead is the price of actual security.

---

## Integration Points

| Layer | How Cage connects |
|---|---|
| **Warden** | All agent outbound routes through Warden — enforced at the VM's network level |
| **Egress** | VM's virtual NIC connects to Egress controller for default-deny networking |
| **Sentinel** | Traffic pipeline runs outside the VM, in the OS layer |
| **Watcher** | Every Cage lifecycle event logged: spawn, pause, resume, quota hit, reclaim |
| **Gate** | `risk = "critical"` tools trigger Gate — VM suspends until human approves |
| **Phoenix** | Phoenix monitors VM health via eBPF, snapshots and restarts on anomaly |
| **Accelerator** | GPU/NPU pinned to VM per AGENT.toml declaration |
| **Swarm** | Each agent in a swarm has its own VM — coordination via ctxgraph, never shared memory |

---

## Build Notes

- **Core technology:** libkrun (library-based KVM)
- **Language:** Rust wrapper around libkrun
- **Dependencies:** Linux KVM (CONFIG_KVM=y), VirtIO drivers (CONFIG_VIRTIO=y)
