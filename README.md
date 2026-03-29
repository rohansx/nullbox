# NullBox

An immutable, minimal Linux OS purpose-built for AI agents.

No SSH. No shell. No package manager. No systemd. Just agents.

## What It Is

NullBox is a hardened Linux distribution where the entire OS exists to run autonomous AI agents — and nothing else. The root filesystem is read-only SquashFS. Every agent runs in its own microVM. Network access is default-deny. Secrets are AES-256-GCM encrypted at rest.

## Architecture

```
EFI/BIOS → Linux 6.18 (KSPP-hardened) → initramfs → SquashFS root (read-only)
                                                          ↓
                                                    nulld (PID 1)
                                                    ├── egress     — default-deny nftables firewall
                                                    ├── ctxgraph   — shared agent memory (SQLite)
                                                    ├── warden     — encrypted secret vault
                                                    └── cage       — per-agent microVM (libkrun/KVM)
                                                         ├── agent-1
                                                         ├── agent-2
                                                         └── ...
```

## Components

| Binary | Purpose |
|--------|---------|
| **nulld** | PID 1 — mounts filesystems, starts services in dependency order, reaps children, handles shutdown |
| **cage** | Spawns per-agent microVMs via libkrun (KVM), enforces CPU/memory caps, injects secrets as env vars |
| **egress** | Default-deny network controller — agents declare allowed domains in AGENT.toml, all else is dropped |
| **warden** | AES-256-GCM encrypted vault with PBKDF2 key derivation — secrets never touch disk in plaintext |
| **ctxgraph** | Content-addressed shared memory (SHA-256 keys, SQLite backend) — agents share context without seeing each other |
| **nullctl** | CLI for managing agents, vault secrets, and system status |

## Kernel

Custom Linux 6.18 built with Clang + ThinLTO. Key hardening:

- **`CONFIG_MODULES=n`** — no loadable kernel modules, ever. Attack surface fixed at compile time.
- **`CONFIG_IO_URING=n`** — disabled (bypasses syscall monitoring).
- **Clang CFI** — control flow integrity via `CONFIG_CFI_CLANG=y`.
- **KSPP defaults** — KASLR, stack protector, hardened usercopy, FORTIFY_SOURCE, zero-init allocations.
- **Seccomp + Landlock + AppArmor** — mandatory access control stack.
- **eBPF** — enabled for audit probes (BPF LSM).
- **KVM** — Intel + AMD for cage microVMs.
- **Built-in NIC drivers** — Intel (e1000e, igb, ixgbe, i40e, ice), Realtek (r8169), Broadcom (tg3, bnx2, bnxt), Mellanox, AMD, plus WiFi (iwlwifi, ath9k/10k/11k, rtw88/89).

## Building

Requires: Rust (nightly), Clang/LLVM, musl target, squashfs-tools, grub, xorriso.

```bash
# Install Rust musl target
rustup target add x86_64-unknown-linux-musl

# Build all binaries (static linking via musl)
cargo build --release --target x86_64-unknown-linux-musl

# Build kernel (downloads Linux 6.18, compiles with Clang)
./kernel/scripts/build-kernel.sh

# Build SquashFS root filesystem
./image/scripts/build-squashfs.sh

# Build initramfs
./image/scripts/build-initramfs.sh

# Build bootable ISO
./image/scripts/build-iso.sh
```

The ISO is a hybrid BIOS+EFI image (~60MB). Flash it to USB or boot in QEMU.

## Installing

### USB flash drive

```bash
# Find your USB device (BE CAREFUL — this erases the drive)
lsblk
sudo dd if=build/output/iso/nullbox.iso of=/dev/sdX bs=4M status=progress
sync
```

Boot from USB. NullBox will auto-detect NICs via built-in drivers and attempt DHCP.

### QEMU (development)

```bash
qemu-system-x86_64 \
    -kernel build/output/kernel/bzImage \
    -initrd build/output/initramfs/initramfs.cpio.gz \
    -append "console=ttyS0" \
    -nographic \
    -m 2G \
    -enable-kvm \
    -cpu host
```

### Persistent storage (optional)

NullBox scans for an ext4 partition containing a `.nullbox-data` sentinel file. If found, vault secrets and ctxgraph state survive reboots.

```bash
# Create a persistent data partition
mkfs.ext4 -L nullbox-data /dev/sdX2
mount /dev/sdX2 /mnt
touch /mnt/.nullbox-data
umount /mnt
```

## Agent manifest

Agents are defined in TOML:

```toml
[agent]
name = "researcher"
version = "0.1.0"

[capabilities]
max_cpu_percent = 25
max_memory_mb = 256

[capabilities.network]
allow = ["api.openai.com", "httpbin.org"]

[capabilities.filesystem]
read = ["/data"]
write = ["/data/output"]
```

Place manifests in `/agent/` on the SquashFS image.

## CLI usage

```bash
# Start/stop agents
nullctl start researcher
nullctl stop researcher
nullctl status

# Manage secrets
nullctl vault set OPENAI_KEY sk-...
nullctl vault list
nullctl vault delete OPENAI_KEY
```

## What doesn't exist

No cron. No dbus. No systemd. No sshd. No shell. No interactive login. No package manager. No mutable root filesystem. Six binaries in `/system/bin/`.

## Smoke test

```bash
./image/scripts/smoke-test.sh
```

Boots the full image in QEMU and validates 19 checkpoints: kernel boot, filesystem mounts, nulld startup, all services running, agent microVM launch, network enforcement, and ctxgraph writes.

## License

See [LICENSE](LICENSE).
