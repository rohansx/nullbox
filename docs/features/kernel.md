# kernel -- KSPP-Hardened Linux Kernel

## Overview

NullBox uses a custom Linux 6.18 kernel built with Clang/ThinLTO and hardened according to KSPP (Kernel Self-Protection Project) guidelines. The kernel is configured with the minimum set of features required for NullBox's workload: KVM for agent microVMs, nftables for network control, SquashFS for the root filesystem, and virtio for guest-host communication. Dangerous features like loadable modules and io_uring are disabled at compile time.

## Architecture

### Build Toolchain

- **Compiler:** Clang with ThinLTO (required for CFI)
- **Base version:** Linux 6.18
- **Config:** `kernel/config/x86_64_defconfig`
- **Build script:** `kernel/scripts/build-kernel.sh`

### Security Hardening

#### No Loadable Modules (CONFIG_MODULES=n)

The single most important security decision. All drivers are compiled directly into the kernel. This eliminates:
- `insmod` / `modprobe` attack vectors
- eBPF rootkits loaded via modules
- Custom kernel module injection
- The attack surface is fixed permanently at compilation time

#### Control Flow Integrity (CFI)

```
CONFIG_CFI_CLANG=y
CONFIG_LTO_CLANG_THIN=y
```

Clang CFI prevents control-flow hijacking by validating indirect function call targets at runtime. Requires ThinLTO for whole-program analysis.

#### io_uring Disabled

```
# CONFIG_IO_URING is not set
```

io_uring bypasses all syscall monitoring (demonstrated by the ARMO "Curing" rootkit PoC). Agent workloads are network-bound, not disk-bound, so the performance benefit does not justify the attack surface.

#### Memory Protection (KSPP)

| Config | Purpose |
|--------|---------|
| `CONFIG_RANDOMIZE_BASE=y` | KASLR -- randomize kernel base address |
| `CONFIG_RANDOMIZE_MEMORY=y` | Randomize physical memory mapping |
| `CONFIG_PAGE_TABLE_ISOLATION=y` | Mitigate Meltdown-class attacks |
| `CONFIG_STACKPROTECTOR_STRONG=y` | Stack canaries on all functions |
| `CONFIG_VMAP_STACK=y` | Guard pages between kernel stacks |
| `CONFIG_INIT_STACK_ALL_ZERO=y` | Zero-initialize all stack variables |
| `CONFIG_HARDENED_USERCOPY=y` | Validate userspace copy bounds |
| `CONFIG_FORTIFY_SOURCE=y` | Compile-time buffer overflow detection |
| `CONFIG_INIT_ON_ALLOC_DEFAULT_ON=y` | Zero-fill heap allocations |
| `CONFIG_INIT_ON_FREE_DEFAULT_ON=y` | Zero-fill freed memory |
| `CONFIG_SLAB_FREELIST_RANDOM=y` | Randomize SLAB freelist order |
| `CONFIG_SLAB_FREELIST_HARDENED=y` | Harden SLAB metadata |
| `CONFIG_SHUFFLE_PAGE_ALLOCATOR=y` | Randomize page allocation |
| `CONFIG_RANDOM_KMALLOC_CACHES=y` | Randomize kmalloc cache selection |

#### Disabled Dangerous Features

| Config | Reason |
|--------|--------|
| `CONFIG_KEXEC` | Prevents runtime kernel replacement |
| `CONFIG_DEVKMEM` | Prevents /dev/kmem access to kernel memory |
| `CONFIG_BINFMT_MISC` | Prevents arbitrary binary format registration |
| `CONFIG_USELIB` | Obsolete syscall, attack surface |
| `CONFIG_MODIFY_LDT_SYSCALL` | Prevents LDT modification attacks |
| `CONFIG_KPROBES` | Prevents dynamic kernel instrumentation |
| `CONFIG_COMPAT_BRK` | Disables compatibility heap layout |

### Mandatory Access Control

```
CONFIG_LSM="landlock,lockdown,yama,apparmor,bpf"
```

Three LSMs are active:
- **AppArmor** -- Per-process MAC profiles
- **Landlock** -- Unprivileged sandboxing
- **Yama** -- ptrace restrictions

Lockdown LSM is set to integrity mode (`CONFIG_LOCK_DOWN_KERNEL_FORCE_INTEGRITY`), preventing unsigned code from running in kernel context.

### Syscall Filtering

```
CONFIG_SECCOMP=y
CONFIG_SECCOMP_FILTER=y
```

Seccomp-BPF is available for cage to apply per-agent syscall profiles.

### Virtualization (for cage)

```
CONFIG_KVM=y
CONFIG_KVM_INTEL=y
CONFIG_KVM_AMD=y
CONFIG_VHOST=y
CONFIG_VHOST_NET=y
CONFIG_VHOST_VSOCK=y
```

KVM provides hardware-accelerated virtualization for agent microVMs. vhost-vsock enables efficient host-guest communication.

### VirtIO Drivers (built-in)

| Driver | Purpose |
|--------|---------|
| `CONFIG_VIRTIO_PCI` | PCI transport for virtio devices |
| `CONFIG_VIRTIO_BLK` | Block devices (rootfs) |
| `CONFIG_VIRTIO_NET` | Network interfaces |
| `CONFIG_VIRTIO_CONSOLE` | Serial console |
| `CONFIG_VIRTIO_BALLOON` | Memory ballooning |
| `CONFIG_VIRTIO_MMIO` | MMIO transport |
| `CONFIG_VIRTIO_INPUT` | Input devices |

### Networking

nftables is built-in for the egress firewall:

```
CONFIG_NF_TABLES=y
CONFIG_NF_TABLES_INET=y
CONFIG_NFT_CT=y
CONFIG_NFT_MASQ=y
CONFIG_NFT_NAT=y
```

Bridge, VLAN, TUN/TAP, macvtap, and veth are built-in for cage networking.

### Filesystem

```
CONFIG_SQUASHFS=y
CONFIG_SQUASHFS_ZSTD=y
CONFIG_OVERLAY_FS=y
```

SquashFS with zstd compression is the root filesystem format. OverlayFS is available for writable layers.

### eBPF

```
CONFIG_BPF=y
CONFIG_BPF_SYSCALL=y
CONFIG_BPF_JIT=y
CONFIG_BPF_JIT_ALWAYS_ON=y
CONFIG_BPF_LSM=y
```

eBPF is enabled for future audit probes (Watcher), not for general use.

### Confidential Computing (future)

```
CONFIG_AMD_MEM_ENCRYPT=y
CONFIG_INTEL_TDX_GUEST=y
```

AMD SEV and Intel TDX support are enabled for future confidential computing workloads.

## Configuration

The kernel config lives at `kernel/config/x86_64_defconfig`. Build with:

```bash
kernel/scripts/build-kernel.sh
```

## Status

**Implemented:**
- Full KSPP-hardened defconfig for x86_64
- No loadable modules
- CFI via Clang ThinLTO
- io_uring disabled
- KVM + vhost-vsock for cage microVMs
- All virtio drivers built-in
- nftables built-in for egress
- SquashFS + zstd for rootfs
- Seccomp + AppArmor + Landlock LSMs
- Memory hardening (KASLR, stack protector, zero-init, freelist hardening)

**Planned:**
- Kernel lockdown enforcement in production builds
- Per-agent seccomp profiles applied by cage
- AppArmor profiles for NullBox services
- eBPF audit probes (Watcher)
- ARM64 / aarch64 kernel config
