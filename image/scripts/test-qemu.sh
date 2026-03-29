#!/usr/bin/env bash
#
# test-qemu.sh — Boot NullBox in QEMU for development testing
#
# Modes:
#   ./image/scripts/test-qemu.sh kernel    — Boot kernel only (expect "no init" panic)
#   ./image/scripts/test-qemu.sh initramfs — Boot kernel + initramfs
#   ./image/scripts/test-qemu.sh iso       — Boot full ISO
#
# All modes use serial console (no GUI window needed).
# Exit QEMU: Ctrl+A then X

set -euo pipefail

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUTPUT_DIR="${NULLBOX_ROOT}/build/output"
VMLINUZ="${OUTPUT_DIR}/kernel/x86_64/vmlinuz"
INITRAMFS="${OUTPUT_DIR}/initramfs/initramfs.cpio.gz"
ISO="${OUTPUT_DIR}/nullbox-x86_64.iso"

MODE="${1:-initramfs}"
MEMORY="${QEMU_MEMORY:-4G}"
CPUS="${QEMU_CPUS:-2}"

# Common QEMU flags
# Detect nested virt CPU flags
CPU_MODEL="host"
if grep -q "vendor_id.*GenuineIntel" /proc/cpuinfo 2>/dev/null; then
    CPU_MODEL="host,+vmx"
elif grep -q "vendor_id.*AuthenticAMD" /proc/cpuinfo 2>/dev/null; then
    CPU_MODEL="host,+svm"
fi

QEMU_COMMON=(
    -enable-kvm
    -m "${MEMORY}"
    -smp "${CPUS}"
    -cpu "${CPU_MODEL}"
    -nographic
    -serial mon:stdio
    -no-reboot
    # User-mode networking so TSI in nested libkrun VMs can proxy through
    -netdev user,id=net0
    -device virtio-net-pci,netdev=net0
)

case "${MODE}" in
    kernel)
        echo "=== QEMU: Kernel-only boot (expect panic) ==="
        echo "  Exit: Ctrl+A then X"
        echo ""
        if [[ ! -f "${VMLINUZ}" ]]; then
            echo "error: ${VMLINUZ} not found. Run build-kernel.sh first."
            exit 1
        fi
        qemu-system-x86_64 \
            "${QEMU_COMMON[@]}" \
            -kernel "${VMLINUZ}" \
            -append "console=ttyS0,115200 loglevel=7 earlyprintk=serial"
        ;;

    initramfs)
        echo "=== QEMU: Kernel + initramfs boot ==="
        echo "  Exit: Ctrl+A then X"
        echo ""
        if [[ ! -f "${VMLINUZ}" ]]; then
            echo "error: ${VMLINUZ} not found. Run build-kernel.sh first."
            exit 1
        fi
        if [[ ! -f "${INITRAMFS}" ]]; then
            echo "error: ${INITRAMFS} not found. Run build-initramfs.sh first."
            exit 1
        fi
        qemu-system-x86_64 \
            "${QEMU_COMMON[@]}" \
            -kernel "${VMLINUZ}" \
            -initrd "${INITRAMFS}" \
            -append "console=ttyS0,115200 loglevel=7"
        ;;

    iso)
        echo "=== QEMU: Full ISO boot ==="
        echo "  Exit: Ctrl+A then X"
        echo ""
        if [[ ! -f "${ISO}" ]]; then
            echo "error: ${ISO} not found. Run build-iso.sh first."
            exit 1
        fi
        qemu-system-x86_64 \
            "${QEMU_COMMON[@]}" \
            -cdrom "${ISO}"
        ;;

    *)
        echo "usage: $0 <kernel|initramfs|iso>"
        echo ""
        echo "  kernel    — Boot kernel only (validates kernel compiles and boots)"
        echo "  initramfs — Boot kernel + initramfs (validates nulld as PID 1)"
        echo "  iso       — Boot full ISO (full system test)"
        exit 1
        ;;
esac
