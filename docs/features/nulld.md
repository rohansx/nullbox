# nulld -- PID 1 Service Supervisor

## Overview

nulld is the init process (PID 1) for NullBox. It replaces systemd with a purpose-built Rust binary that mounts filesystems, starts services in dependency order, reaps orphan processes, and handles graceful shutdown. Because PID 1 exiting causes a kernel panic, nulld wraps all execution in `catch_unwind` and falls back to a recovery hold if anything goes wrong.

## Architecture

### Boot Phases

1. **Verify PID 1** -- nulld refuses to run as any other PID.
2. **Mount filesystems** -- proc, sys, devtmpfs, devpts, /dev/shm, /tmp (256 MiB tmpfs), /run (64 MiB tmpfs), /var (512 MiB tmpfs). All mounts use restrictive flags (nosuid, nodev, noexec where appropriate).
3. **Install signal handlers** -- SIGCHLD for child reaping, SIGTERM for shutdown.
4. **Load configuration** -- Reads `/system/config/nulld.toml`. Falls back to built-in defaults if the file is missing.
5. **Bind control socket** -- `/run/nulld.sock` is ready before services start.
6. **Start services** -- Topological sort via Kahn's algorithm, then spawn each in order.
7. **Main loop** -- Reaps exited children, schedules restarts with exponential backoff, polls the control socket for commands. Ticks every 500 ms.
8. **Shutdown** -- On SIGTERM or control socket `shutdown` command, stops services in reverse dependency order. Each service gets SIGTERM, then SIGKILL after a 5-second grace period.

### Key Components

| File | Purpose |
|------|---------|
| `cmd/nulld/src/main.rs` | Entry point, panic recovery, kernel log output |
| `cmd/nulld/src/mount.rs` | Filesystem mount table and mount logic |
| `cmd/nulld/src/config.rs` | TOML config parser, built-in service defaults |
| `cmd/nulld/src/service.rs` | ServiceDef type, Kahn's topological sort |
| `cmd/nulld/src/supervisor.rs` | Process lifecycle, restart backoff, status reporting |
| `cmd/nulld/src/control.rs` | Unix socket server (non-blocking, polled from main loop) |
| `cmd/nulld/src/signal.rs` | SIGCHLD reaping, SIGTERM shutdown flag |

### Restart Backoff

When a service exits and its restart policy allows it, nulld schedules a restart with exponential backoff:

```
Attempt 0:  1s
Attempt 1:  2s
Attempt 2:  4s
Attempt 3:  8s
Attempt 4: 16s
Attempt 5+: 30s (cap)
```

### Logging

All log output goes to `/dev/kmsg` (the kernel log buffer), which is visible on the serial console. Falls back to stderr if `/dev/kmsg` is unavailable.

## Configuration

### /system/config/nulld.toml

```toml
[service.egress]
binary = "/system/bin/egress"
args = []
depends_on = []
restart = "always"

[service.ctxgraph]
binary = "/system/bin/ctxgraph"
depends_on = []

[service.cage]
binary = "/system/bin/cage"
depends_on = ["egress", "ctxgraph"]
restart = "on-failure"
```

### Service Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `binary` | string | required | Absolute path to the executable |
| `args` | string[] | `[]` | Command-line arguments |
| `depends_on` | string[] | `[]` | Services that must start first |
| `restart` | string | `"always"` | Restart policy: `always`, `on-failure`, `never` |

### Built-in Defaults

When no config file exists, nulld uses hardcoded defaults:
- **egress** -- `/system/bin/egress`, no deps, always restart
- **ctxgraph** -- `/system/bin/ctxgraph`, no deps, always restart
- **cage** -- `/system/bin/cage`, depends on egress + ctxgraph, always restart

## API / Protocol

### Control Socket

**Path:** `/run/nulld.sock` (Unix domain socket, newline-delimited JSON)

#### status

```json
-> {"method": "status"}
<- {"services": [
     {"name": "egress", "state": "running", "pid": 42, "restart_count": 0},
     {"name": "ctxgraph", "state": "running", "pid": 43, "restart_count": 0},
     {"name": "cage", "state": "running", "pid": 44, "restart_count": 0}
   ]}
```

#### shutdown

```json
-> {"method": "shutdown"}
<- {"ok": true}
```

#### Error

```json
-> {"method": "bogus"}
<- {"error": "unknown method: bogus"}
```

## Status

**Implemented:**
- Filesystem mounting with proper security flags
- Dependency-ordered service startup (Kahn's algorithm)
- SIGCHLD reaping, SIGTERM graceful shutdown
- Exponential backoff restarts (1s to 30s cap)
- Non-blocking control socket (status, shutdown)
- Panic recovery (catch_unwind + recovery hold)
- Kernel log output via /dev/kmsg

**Planned:**
- Health check probes per service
- Service resource limits (cgroups)
- Readiness notification protocol (service signals when ready)
