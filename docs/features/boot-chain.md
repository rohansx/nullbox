# boot-chain -- Boot Chain

## Overview

NullBox boots through a four-stage chain: firmware loads the kernel and initramfs, the initramfs mounts the SquashFS root image and pivots into it, then nulld takes over as PID 1 and starts all services. The entire root filesystem is read-only SquashFS; writable state lives on tmpfs mounts created by nulld.

## Architecture

### Boot Stages

```
Stage 1: Firmware (BIOS/UEFI/QEMU)
    |
    v
Stage 2: GRUB / QEMU -kernel
    |  loads vmlinuz + initramfs.cpio.gz
    v
Stage 3: initramfs /init (busybox shell script)
    |  mounts SquashFS at /newroot
    |  configures network (if present)
    |  switch_root -> /newroot
    v
Stage 4: nulld (PID 1, /system/bin/nulld)
    |  mounts /proc, /sys, /dev, /tmp, /run, /var
    |  loads /system/config/nulld.toml
    |  starts: egress -> ctxgraph -> cage
    v
Stage 5: cage auto-starts agents
    |  scans /agent/*.toml manifests
    |  spawns KVM microVMs via libkrun
    v
Agents running in isolated VMs
```

### Stage 1-2: Firmware and Bootloader

**Production:** BIOS or UEFI loads GRUB, which loads the kernel and initramfs.

**Development:** QEMU directly loads the kernel and initramfs:

```bash
qemu-system-x86_64 \
    -kernel build/output/kernel/vmlinuz \
    -initrd build/output/initramfs/initramfs.cpio.gz \
    -enable-kvm \
    -m 2G \
    -nographic \
    -append "console=ttyS0"
```

### Stage 3: initramfs

The initramfs contains a minimal busybox-based init script (`image/scripts/build-initramfs.sh`). Its job is to bridge from kernel boot to the SquashFS root:

1. **Mount virtual filesystems** -- proc, sysfs, devtmpfs
2. **Find SquashFS root** -- Either embedded in the initramfs (`/nullbox.squashfs`) or on a block device partition
3. **Configure network** -- If eth0 or enp0s3 exists, sets IP 10.0.2.15/24 with gateway 10.0.2.2 (QEMU user networking / TSI)
4. **Verify nulld** -- Checks that `/newroot/system/bin/nulld` exists
5. **switch_root** -- Pivots to `/newroot` and execs `/system/bin/nulld`

If SquashFS is not found or nulld is missing, the init drops to a rescue shell (if busybox is present) or sleeps forever.

#### SquashFS Discovery

The initramfs tries two methods to find the root:

| Method | Path | Use Case |
|--------|------|----------|
| Embedded | `/nullbox.squashfs` inside initramfs | ISO images, QEMU testing |
| Block device scan | `/dev/sda*`, `/dev/vda*`, `/dev/nvme*` | Bare metal, persistent installs |

For block devices, it mounts each partition as SquashFS and checks for `/system/bin/nulld` to verify it found the right one.

### Stage 4: nulld (PID 1)

After switch_root, nulld is PID 1. The initramfs mounts are cleaned up. nulld performs its own mount sequence:

| Mount | Type | Size | Flags | Purpose |
|-------|------|------|-------|---------|
| `/proc` | proc | -- | nosuid,nodev,noexec | Process information |
| `/sys` | sysfs | -- | nosuid,nodev,noexec | Kernel/device info |
| `/dev` | devtmpfs | -- | nosuid | Device nodes |
| `/dev/pts` | devpts | -- | nosuid,noexec | Pseudo-terminals |
| `/dev/shm` | tmpfs | -- | nosuid,nodev | Shared memory |
| `/tmp` | tmpfs | 256 MiB | nosuid,nodev,noexec | Temporary files |
| `/run` | tmpfs | 64 MiB | nosuid,nodev,noexec | Runtime state (sockets) |
| `/var` | tmpfs | 512 MiB | nosuid,nodev | Writable data (logs, ctxgraph DB) |

After mounting, nulld creates required directories under `/var`:
- `/var/log`
- `/var/lib/ctxgraph`
- `/agent`

Then it loads service config and starts services in dependency order.

### Stage 5: Service Startup

Default service start order (topological sort):

```
1. egress      (no dependencies)
2. ctxgraph    (no dependencies)
3. cage        (depends on: egress, ctxgraph)
```

egress and ctxgraph have no interdependencies and may start in any order relative to each other. cage waits for both.

Once cage is running, it scans `/agent/*.toml` for manifests and auto-starts any agent whose rootfs exists at `/system/rootfs/<agent-name>/`.

## Configuration

### Build Scripts

| Script | Purpose |
|--------|---------|
| `kernel/scripts/build-kernel.sh` | Build the hardened kernel |
| `image/scripts/build-initramfs.sh` | Build the initramfs (cpio.gz) |
| `image/scripts/build-squashfs.sh` | Build the SquashFS root image |
| `image/scripts/build-iso.sh` | Package into bootable ISO |
| `image/scripts/prepare-agent-rootfs.sh` | Prepare per-agent rootfs directories |
| `image/scripts/test-qemu.sh` | Launch in QEMU for testing |

### Build Outputs

```
build/output/kernel/vmlinuz           Kernel image
build/output/initramfs/initramfs.cpio.gz  Initramfs archive
build/output/squashfs/nullbox.squashfs    Root filesystem
```

### Kernel Command Line

For QEMU development:

```
console=ttyS0
```

For production, additional hardening parameters may be added (e.g., `lockdown=integrity`, `init_on_alloc=1`).

## API / Protocol

The boot chain has no runtime API. It is a one-way pipeline from firmware to nulld. Once nulld is running, all management happens through the service sockets (see [nulld](nulld.md), [cage](cage.md), [nullctl](nullctl.md)).

## Status

**Implemented:**
- initramfs build script with embedded SquashFS support
- busybox-based init with SquashFS discovery and switch_root
- Network configuration for QEMU/TSI (eth0, enp0s3)
- Rescue shell fallback on boot failure
- nulld mounts all required virtual filesystems with security flags
- QEMU test script for rapid iteration
- Full boot chain tested: kernel to initramfs to SquashFS to nulld to cage to agent VM

**Planned:**
- Verified boot (dm-verity on SquashFS)
- Secure Boot chain (signed kernel + initramfs)
- Persistent /var partition (overlay on SquashFS)
- ISO image packaging for bare-metal deployment
- ARM64 initramfs and kernel builds
