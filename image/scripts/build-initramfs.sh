#!/usr/bin/env bash
#
# build-initramfs.sh — Build minimal initramfs for NullBox
#
# The initramfs is the bridge between kernel boot and SquashFS root.
# It contains a tiny init script that:
#   1. Mounts the SquashFS root image
#   2. Pivots into it
#   3. Execs nulld as PID 1
#
# Usage:
#   ./image/scripts/build-initramfs.sh
#
# Expects: build/output/squashfs/nullbox.squashfs (or builds without it for kernel-only testing)

set -euo pipefail

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="${NULLBOX_ROOT}/build/initramfs"
OUTPUT_DIR="${NULLBOX_ROOT}/build/output/initramfs"
SQUASHFS="${NULLBOX_ROOT}/build/output/squashfs/nullbox.squashfs"

echo "=== NullBox initramfs Build ==="

mkdir -p "${BUILD_DIR}" "${OUTPUT_DIR}"

# Clean previous build
rm -rf "${BUILD_DIR:?}"/*

# Create directory structure
mkdir -p "${BUILD_DIR}"/{bin,dev,proc,sys,mnt/root,newroot,tmp}

# Create device nodes (need root or fakeroot)
# These are essential for early boot before devtmpfs is mounted
if [[ $EUID -eq 0 ]]; then
    mknod -m 622 "${BUILD_DIR}/dev/console" c 5 1
    mknod -m 666 "${BUILD_DIR}/dev/null" c 1 3
    mknod -m 666 "${BUILD_DIR}/dev/zero" c 1 5
    mknod -m 666 "${BUILD_DIR}/dev/tty" c 5 0
else
    echo "  (skipping device nodes — run as root or use fakeroot for production builds)"
fi

# Copy nulld binary (statically linked)
NULLD_BIN="${NULLBOX_ROOT}/target/x86_64-unknown-linux-musl/release/nulld"
if [[ -f "${NULLD_BIN}" ]]; then
    cp "${NULLD_BIN}" "${BUILD_DIR}/bin/nulld"
    echo "  Copied nulld (musl static binary)"
else
    # Fallback: use debug build or regular build
    NULLD_BIN="${NULLBOX_ROOT}/target/release/nulld"
    if [[ -f "${NULLD_BIN}" ]]; then
        cp "${NULLD_BIN}" "${BUILD_DIR}/bin/nulld"
        echo "  Copied nulld (release binary — not static, dev only)"
    else
        echo "  WARNING: nulld binary not found. Building minimal init only."
    fi
fi

# Copy busybox for rescue/debug (removed in production builds)
if command -v busybox &>/dev/null; then
    cp "$(command -v busybox)" "${BUILD_DIR}/bin/busybox"
    # Create essential symlinks
    for cmd in sh ls cat mount umount mkdir switch_root ip udhcpc grep sleep; do
        ln -sf busybox "${BUILD_DIR}/bin/${cmd}"
    done
    echo "  Copied busybox (rescue shell — remove for production)"
fi

# Copy SquashFS image into initramfs if it exists
if [[ -f "${SQUASHFS}" ]]; then
    cp "${SQUASHFS}" "${BUILD_DIR}/nullbox.squashfs"
    echo "  Embedded SquashFS root image"
    SQUASHFS_EMBEDDED=1
else
    echo "  No SquashFS image found — init will attempt disk mount"
    SQUASHFS_EMBEDDED=0
fi

# Create init script
cat > "${BUILD_DIR}/init" << 'INIT_EOF'
#!/bin/sh
#
# NullBox initramfs init
# Mounts SquashFS root, pivots, execs nulld as PID 1.
#

echo "nullbox: initramfs starting"

# Mount essential virtual filesystems
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev

echo "nullbox: virtual filesystems mounted"

# Find and mount the SquashFS root
SQUASHFS_FOUND=0

# Option 1: SquashFS embedded in initramfs
if [ -f /nullbox.squashfs ]; then
    echo "nullbox: mounting embedded SquashFS root"
    mount -t squashfs -o ro,loop /nullbox.squashfs /newroot
    SQUASHFS_FOUND=1
fi

# Option 2: SquashFS on a labeled partition
if [ "${SQUASHFS_FOUND}" -eq 0 ]; then
    echo "nullbox: searching for SquashFS partition (LABEL=nullbox-root)..."

    # Wait for devices to settle
    sleep 1

    # Try to find partition by scanning /dev
    for dev in /dev/sda* /dev/vda* /dev/nvme*; do
        [ -b "${dev}" ] || continue
        if mount -t squashfs -o ro "${dev}" /newroot 2>/dev/null; then
            if [ -f /newroot/system/bin/nulld ]; then
                echo "nullbox: found SquashFS root on ${dev}"
                SQUASHFS_FOUND=1
                break
            fi
            umount /newroot 2>/dev/null
        fi
    done
fi

if [ "${SQUASHFS_FOUND}" -eq 0 ]; then
    echo "nullbox: ERROR — cannot find SquashFS root!"
    echo "nullbox: dropping to rescue shell (if available)"
    if [ -x /bin/sh ]; then
        exec /bin/sh
    fi
    # No shell — sleep forever
    while true; do sleep 3600; done
fi

# Verify nulld exists in the new root
if [ ! -x /newroot/system/bin/nulld ]; then
    echo "nullbox: ERROR — /system/bin/nulld not found in SquashFS root!"
    if [ -x /bin/sh ]; then
        exec /bin/sh
    fi
    while true; do sleep 3600; done
fi

# Configure network — find first real NIC and bring it up via DHCP
ip link set lo up

# Find the first non-loopback network interface
NETIF=""
for iface in /sys/class/net/*; do
    name=$(basename "${iface}")
    [ "${name}" = "lo" ] && continue
    [ -d "${iface}" ] || continue
    NETIF="${name}"
    break
done

if [ -n "${NETIF}" ]; then
    echo "nullbox: configuring network (${NETIF})"
    ip link set "${NETIF}" up

    # Wait briefly for link to come up
    sleep 1

    # Try DHCP first (busybox udhcpc)
    if [ -x /bin/udhcpc ]; then
        echo "nullbox: running DHCP on ${NETIF}"
        # Create minimal udhcpc script
        mkdir -p /usr/share/udhcpc
        cat > /usr/share/udhcpc/default.script << 'DHCP_SCRIPT'
#!/bin/sh
case "$1" in
    bound|renew)
        ip addr flush dev "$interface"
        ip addr add "$ip/${mask:-24}" dev "$interface"
        if [ -n "$router" ]; then
            ip route add default via "$router" dev "$interface"
        fi
        if [ -n "$dns" ]; then
            : > /etc/resolv.conf
            for d in $dns; do
                echo "nameserver $d" >> /etc/resolv.conf
            done
        fi
        ;;
esac
DHCP_SCRIPT
        chmod +x /usr/share/udhcpc/default.script
        udhcpc -i "${NETIF}" -n -q -t 5 -T 3 2>/dev/null
        if [ $? -eq 0 ]; then
            ADDR=$(ip -4 addr show "${NETIF}" | grep -o 'inet [^ ]*' | head -1 | cut -d' ' -f2)
            echo "nullbox: network configured via DHCP (${ADDR})"
        else
            echo "nullbox: DHCP failed, falling back to static"
            ip addr add 10.0.2.15/24 dev "${NETIF}"
            ip route add default via 10.0.2.2
            echo "nullbox: network configured (10.0.2.15 static fallback)"
        fi
    else
        # No udhcpc — use QEMU-compatible static config
        ip addr add 10.0.2.15/24 dev "${NETIF}"
        ip route add default via 10.0.2.2
        echo "nullbox: network configured (10.0.2.15 static)"
    fi
else
    echo "nullbox: WARNING — no network interface found"
fi

echo "nullbox: pivoting to SquashFS root"

# Clean up initramfs mounts before pivot
umount /proc 2>/dev/null
umount /sys 2>/dev/null
umount /dev 2>/dev/null

# Pivot root: switch_root moves to newroot and execs nulld
exec switch_root /newroot /system/bin/nulld

# If switch_root fails, we end up here
echo "nullbox: FATAL — switch_root failed!"
while true; do sleep 3600; done
INIT_EOF

chmod +x "${BUILD_DIR}/init"

# Build cpio archive
echo ">>> Creating initramfs cpio archive..."
cd "${BUILD_DIR}"
find . | cpio -o -H newc 2>/dev/null | gzip -9 > "${OUTPUT_DIR}/initramfs.cpio.gz"
cd "${NULLBOX_ROOT}"

INITRAMFS_SIZE=$(du -h "${OUTPUT_DIR}/initramfs.cpio.gz" | cut -f1)

echo ""
echo "=== initramfs build complete ==="
echo "  Output: ${OUTPUT_DIR}/initramfs.cpio.gz"
echo "  Size:   ${INITRAMFS_SIZE}"
