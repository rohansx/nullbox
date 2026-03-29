# Egress — Default-Deny Network Controller

> Nothing leaves the machine unless explicitly allowed.

**Layer:** NullBox Layer 4

---

## Why Egress Exists

Egress controls what the entire OS can reach at the network level. Separate from Warden (credentials) and Cage (per-agent rules). It's the outermost network boundary — if Cage's per-agent rules miss something, Egress catches it.

---

## Default-Deny Categories

Categories blocked at OS level **before** any agent rule is evaluated:

| Category | What's Blocked | Why |
|---|---|---|
| **Cloud metadata** | `169.254.169.254`, `fd00:ec2::254` | Prevents IMDS credential theft (AWS/GCP/Azure) |
| **Private networks** | RFC-1918 ranges from agent traffic | Prevents SSRF into internal networks |
| **DNS rebinding** | Once a domain resolves to an IP, binding is locked | Prevents rebinding to malicious IPs |
| **Known malicious** | Tor exit nodes, known malicious IP ranges | Updated via OTA |

---

## DNS-to-IP Binding

```
Agent declares: network.allow = ["api.openai.com"]

First resolution:
  api.openai.com -> 104.18.7.192
  Binding locked: api.openai.com = 104.18.7.192

Rebinding attempt:
  api.openai.com -> 192.168.1.100 (attacker's internal IP)
  BLOCKED by Egress. Watcher logs DNS rebinding attempt.
```

---

## Integration Points

| Layer | How Egress connects |
|---|---|
| **Cage** | Per-agent network rules applied at the VM's virtual NIC level |
| **Warden** | Agent traffic must pass through Warden after Egress allows it |
| **Watcher** | All Egress decisions logged: allow, block, DNS resolution, rebinding attempts |
| **Phoenix** | Network partition detection triggers Phoenix offline mode |

---

## All Egress Decisions Logged

Every network decision is recorded by Watcher with:
- Full DNS resolution chain
- Source agent identity
- Destination IP + port
- Decision (allow/block) + reason
- Timestamp
