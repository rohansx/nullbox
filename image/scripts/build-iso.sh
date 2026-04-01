#!/usr/bin/env bash
#
# build-iso.sh — Build bootable NullBox ISO for x86_64
#
# Creates an ISO that can boot in QEMU or be written to USB.
# Uses GRUB for both BIOS and EFI boot.
#
# Usage:
#   ./image/scripts/build-iso.sh
#
# Prerequisites:
#   - Kernel built:     build/output/kernel/x86_64/vmlinuz
#   - initramfs built:  build/output/initramfs/initramfs.cpio.gz
#   - grub-mkrescue, xorriso

set -euo pipefail

PRODUCTION=0
ARCH="x86_64"
for arg in "$@"; do
    case "${arg}" in
        --production) PRODUCTION=1 ;;
        --arch=*) ARCH="${arg#*=}" ;;
    esac
done

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="${NULLBOX_ROOT}/build/iso-staging"
OUTPUT_DIR="${NULLBOX_ROOT}/build/output"
VMLINUZ="${OUTPUT_DIR}/kernel/${ARCH}/vmlinuz"
INITRAMFS="${OUTPUT_DIR}/initramfs/initramfs.cpio.gz"
ISO_OUTPUT="${OUTPUT_DIR}/nullbox-${ARCH}.iso"

echo "=== NullBox ISO Build ==="

# Check prerequisites
for file in "${VMLINUZ}" "${INITRAMFS}"; do
    if [[ ! -f "${file}" ]]; then
        echo "error: missing ${file}"
        echo "build the kernel and initramfs first"
        exit 1
    fi
done

for tool in grub-mkrescue xorriso; do
    if ! command -v "${tool}" &>/dev/null; then
        echo "error: ${tool} not found. Install: pacman -S grub xorriso"
        exit 1
    fi
done

# Clean previous build
rm -rf "${BUILD_DIR:?}"
mkdir -p "${BUILD_DIR}/boot/grub"

# Copy kernel and initramfs
cp "${VMLINUZ}" "${BUILD_DIR}/boot/vmlinuz"
cp "${INITRAMFS}" "${BUILD_DIR}/boot/initramfs.cpio.gz"

# Create GRUB config
if [[ "${PRODUCTION}" == "1" ]]; then
    EXTRA_CMDLINE=" nullbox.production=1"
else
    EXTRA_CMDLINE=""
fi

cat > "${BUILD_DIR}/boot/grub/grub.cfg" << GRUB_EOF
insmod serial
if serial --speed=115200 --unit=0; then
    terminal_output serial console
fi

set timeout=3
set default=0

menuentry "NullBox v0.1" {
    linux /boot/vmlinuz console=ttyS0,115200 console=tty0 loglevel=4${EXTRA_CMDLINE}
    initrd /boot/initramfs.cpio.gz
}

menuentry "NullBox v0.1 (verbose)" {
    linux /boot/vmlinuz console=ttyS0,115200 console=tty0 loglevel=7 earlyprintk=serial${EXTRA_CMDLINE}
    initrd /boot/initramfs.cpio.gz
}
GRUB_EOF

if [[ "${PRODUCTION}" != "1" ]]; then
    cat >> "${BUILD_DIR}/boot/grub/grub.cfg" << 'GRUB_EOF'

menuentry "NullBox v0.1 (rescue)" {
    linux /boot/vmlinuz console=ttyS0,115200 console=tty0 loglevel=7 nullbox.rescue=1
    initrd /boot/initramfs.cpio.gz
}
GRUB_EOF
fi

# Build ISO with grub-mkrescue (handles both BIOS and EFI)
echo ">>> Building ISO..."
grub-mkrescue -o "${ISO_OUTPUT}" "${BUILD_DIR}" 2>/dev/null

ISO_SIZE=$(du -h "${ISO_OUTPUT}" | cut -f1)

echo ""
echo "=== ISO build complete ==="
echo "  Output: ${ISO_OUTPUT}"
echo "  Size:   ${ISO_SIZE}"
echo ""
echo "  Boot in QEMU:"
echo "    qemu-system-x86_64 \\"
echo "      -enable-kvm \\"
echo "      -m 4G \\"
echo "      -cpu host \\"
echo "      -nographic \\"
echo "      -serial mon:stdio \\"
echo "      -cdrom ${ISO_OUTPUT}"
echo ""
echo "  Or boot kernel+initrd directly (faster):"
echo "    qemu-system-x86_64 \\"
echo "      -enable-kvm \\"
echo "      -m 4G \\"
echo "      -cpu host \\"
echo "      -nographic \\"
echo "      -serial mon:stdio \\"
echo "      -kernel ${VMLINUZ} \\"
echo "      -initrd ${INITRAMFS} \\"
echo "      -append 'console=ttyS0,115200 loglevel=7'"
