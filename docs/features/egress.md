# egress -- Default-Deny Network Controller

## Overview

egress enforces network isolation for NullBox agents using nftables. It generates an atomic ruleset that blocks all outbound traffic by default, then adds per-agent allow rules based on their AGENT.toml manifests. Critical targets like cloud metadata endpoints (AWS/GCP/Azure IMDSv2, Alibaba) and RFC-1918 private ranges are blocked unconditionally to prevent credential theft and lateral movement.

## Architecture

### TSI Networking Model

With TSI (Transparent Socket Impersonation), agent traffic exits through the host's output chain -- not forwarded through a tap device. The output chain uses IP allowlists to control what agents can reach. This is simpler than tap-based forwarding and works naturally with libkrun microVMs.

### Ruleset Generation

egress generates a complete nftables ruleset as a single atomic file, applied via `nft -f`. This avoids race conditions from incremental rule additions.

```
flush ruleset

table inet nullbox {
  chain output {
    type filter hook output priority 0; policy drop;

    oif lo accept                              # loopback always open
    ct state established,related accept        # return traffic
    udp dport 53 accept                        # DNS resolution
    tcp dport 53 accept                        # DNS resolution

    ip daddr 169.254.169.254/32 drop           # cloud metadata
    ip daddr 100.100.100.200/32 drop           # alibaba metadata
    ip daddr fd00:ec2::254/128 drop            # aws imdsv2 ipv6

    ip daddr 10.0.0.0/8 drop                  # rfc-1918
    ip daddr 172.16.0.0/12 drop               # rfc-1918
    ip daddr 192.168.0.0/16 drop              # rfc-1918

    # Agent: researcher
    ip daddr 104.18.0.1 accept comment "allow for researcher"
    ip daddr 104.18.0.2 accept comment "allow for researcher"

    log prefix "nullbox-deny: " drop           # log + drop everything else
  }

  chain input {
    type filter hook input priority 0; policy drop;
    iif lo accept
    ct state established,related accept
  }
}
```

### Key Components

| File | Purpose |
|------|---------|
| `pkg/egress/src/main.rs` | Daemon: socket listener, agent state, nft apply |
| `pkg/egress/src/firewall.rs` | TSI + legacy ruleset generators |
| `pkg/egress/src/resolve.rs` | DNS resolution (domains → IPs via system resolver) |
| `pkg/egress/src/blocklist.rs` | Hardcoded blocked CIDRs (metadata, RFC-1918, link-local) |
| `pkg/egress/src/lib.rs` | Library re-exports |

### Blocklists

| Category | CIDRs | Reason |
|----------|-------|--------|
| Cloud metadata | `169.254.169.254/32`, `100.100.100.200/32`, `fd00:ec2::254/128` | Prevents IAM credential theft |
| RFC-1918 | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` | Prevents lateral movement to internal networks |
| Link-local | `169.254.0.0/16`, `fe80::/10` | Prevents link-local service discovery |
| All IPv6 | (all) | Blocked from agents in v0.1 to simplify the firewall |

### IP Validation

The `blocklist::is_blocked()` function provides runtime IP checking beyond nftables rules. It performs octet-level matching for IPv4 ranges and blocks all IPv6 from agents in v0.1. Resolved IPs are filtered through this before being added to the ruleset.

## Protocol

### Control Socket

**Path:** `/run/egress.sock` (Unix domain socket, JSON-over-line protocol)

### Commands

**add-agent** -- Register an agent and allow its declared domains:
```json
-> {"method":"add-agent","agent":"researcher","domains":["api.openai.com","api.exa.ai"]}
<- {"ok":true,"agent":"researcher","allowed_ips":4}
```

**remove-agent** -- Revoke an agent's network access:
```json
-> {"method":"remove-agent","agent":"researcher"}
<- {"ok":true,"agent":"researcher"}
```

**list** -- List all registered agents and their allowed IPs:
```json
-> {"method":"list"}
<- {"agents":[{"agent":"researcher","domains":["api.openai.com"],"allowed_ips":["104.18.0.1"]}]}
```

### Cage Integration

When cage starts an agent, it sends `add-agent` to egress with the agent's `capabilities.network.allow` domains from AGENT.toml. When cage stops an agent, it sends `remove-agent`. This ensures the firewall tracks exactly which agents are running.

### Resolution Pipeline

1. cage starts agent → sends `add-agent` with domain list to egress
2. egress resolves domains via system DNS (`/etc/resolv.conf`)
3. Resolved IPs are filtered through `blocklist::is_blocked()` (drops RFC-1918, metadata, IPv6)
4. Full TSI ruleset is regenerated with all current agent rules
5. Ruleset is written atomically (tmp + rename) to `/run/egress-rules.nft`
6. Applied via `nft -f`

## Status

**Implemented:**
- TSI-mode default-deny ruleset (output chain policy: drop)
- Cloud metadata blocking (AWS, GCP, Azure, DigitalOcean, Alibaba)
- RFC-1918 and link-local blocking
- DNS resolution (system resolver with IP passthrough)
- Per-agent IP allowlists from resolved domains
- Blocked-IP filtering before ruleset generation
- Atomic ruleset application via `nft -f`
- JSON socket protocol: add-agent, remove-agent, list
- Cage → egress integration (auto-notify on agent start/stop)
- Loopback + DNS + established/related always allowed
- Log prefix on denied traffic for debugging
- Legacy forward-chain mode preserved for tap-based networking
- 22 unit tests (firewall, blocklist, resolver)

**Not yet implemented:**
- `nft` binary is not yet included in the SquashFS image
- IPv6 allow rules (all IPv6 blocked from agents in v0.1)
- Periodic DNS re-resolution (domains may change IPs)
- OTA blocklist updates
- Per-agent rate limiting
