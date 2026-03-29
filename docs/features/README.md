# NullBox Features

NullBox is a hardened runtime for AI agents. Each agent runs in its own KVM microVM with default-deny networking and content-addressed shared memory.

## Feature Index

| Feature | Description | Source | Status |
|---------|-------------|--------|--------|
| [nulld](nulld.md) | PID 1 service supervisor | `cmd/nulld/` | Working |
| [cage](cage.md) | Per-agent microVM isolation via libkrun | `pkg/cage/` | Working |
| [egress](egress.md) | Default-deny network controller (nftables) | `pkg/egress/` | Partial |
| [ctxgraph](ctxgraph.md) | Content-addressed shared agent memory | `pkg/ctxgraph/` | Working |
| [nullctl](nullctl.md) | CLI management tool | `cmd/nullctl/` | Working |
| [kernel](kernel.md) | KSPP-hardened Linux kernel | `kernel/` | Working |
| [boot-chain](boot-chain.md) | Boot chain (kernel to initramfs to SquashFS to nulld) | `image/` | Working |

## Service Dependency Graph

```
nulld (PID 1)
  |
  +-- egress        (no dependencies)
  +-- ctxgraph      (no dependencies)
  +-- cage           (depends on: egress, ctxgraph)
        |
        +-- agent-1  (KVM microVM)
        +-- agent-2  (KVM microVM)
        +-- ...
```

nulld starts services in topological order: egress and ctxgraph first (in parallel), then cage once both are running. cage auto-starts agents that have a valid rootfs.

## System Layout

```
/system/bin/nulld          PID 1 supervisor
/system/bin/cage           microVM manager
/system/bin/egress         network controller
/system/bin/ctxgraph       shared memory daemon
/system/config/nulld.toml  service configuration
/system/rootfs/<agent>/    per-agent rootfs (read-only SquashFS)
/agent/*.toml              agent manifests
/run/nulld.sock            nulld control socket
/run/cage.sock             cage control socket
/run/egress.sock           egress control socket
/run/ctxgraph.sock         ctxgraph Unix socket
/var/lib/ctxgraph/db.sqlite  ctxgraph database
```
