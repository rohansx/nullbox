#!/usr/bin/env bash
#
# smoke-test.sh — End-to-end boot validation for NullBox
#
# Boots NullBox in QEMU, waits for key milestones, and validates
# the full service stack: nulld → egress + ctxgraph + warden → cage → test-agent.
#
# Usage:
#   ./image/scripts/smoke-test.sh [initramfs|iso]
#
# Exit codes:
#   0 — all checks passed
#   1 — one or more checks failed

set -euo pipefail

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUTPUT_DIR="${NULLBOX_ROOT}/build/output"
VMLINUZ="${OUTPUT_DIR}/kernel/x86_64/vmlinuz"
INITRAMFS="${OUTPUT_DIR}/initramfs/initramfs.cpio.gz"
ISO="${OUTPUT_DIR}/nullbox-x86_64.iso"

MODE="${1:-initramfs}"
TIMEOUT=90
LOG_FILE="/tmp/nullbox-smoke-test.log"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

PASS=0
FAIL=0

check() {
    local name="$1"
    local pattern="$2"

    if grep -q "${pattern}" "${LOG_FILE}"; then
        echo -e "  ${GREEN}PASS${NC}  ${name}"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC}  ${name}"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== NullBox Smoke Test ==="
echo "  Mode:    ${MODE}"
echo "  Timeout: ${TIMEOUT}s"
echo ""

# Detect CPU model for KVM
CPU_MODEL="host"
if grep -q "vendor_id.*GenuineIntel" /proc/cpuinfo 2>/dev/null; then
    CPU_MODEL="host,+vmx"
elif grep -q "vendor_id.*AuthenticAMD" /proc/cpuinfo 2>/dev/null; then
    CPU_MODEL="host,+svm"
fi

# Build QEMU command
QEMU_ARGS=(
    -enable-kvm
    -m 4G
    -smp 2
    -cpu "${CPU_MODEL}"
    -nographic
    -serial mon:stdio
    -no-reboot
    -netdev user,id=net0
    -device virtio-net-pci,netdev=net0
)

case "${MODE}" in
    initramfs)
        if [[ ! -f "${VMLINUZ}" ]] || [[ ! -f "${INITRAMFS}" ]]; then
            echo "error: kernel or initramfs not found. Build first."
            exit 1
        fi
        QEMU_ARGS+=(
            -kernel "${VMLINUZ}"
            -initrd "${INITRAMFS}"
            -append "console=ttyS0,115200 loglevel=7"
        )
        ;;
    iso)
        if [[ ! -f "${ISO}" ]]; then
            echo "error: ISO not found. Run build-iso.sh first."
            exit 1
        fi
        QEMU_ARGS+=(-cdrom "${ISO}")
        ;;
    *)
        echo "usage: $0 <initramfs|iso>"
        exit 1
        ;;
esac

# Boot QEMU in background, capture output
echo ">>> Booting NullBox in QEMU..."
rm -f "${LOG_FILE}"
timeout "${TIMEOUT}" qemu-system-x86_64 "${QEMU_ARGS[@]}" 2>&1 | \
    sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g' | tr -d '\r' > "${LOG_FILE}" || true

echo ""
echo "--- Boot Log Checks ---"
echo ""

# Boot chain checks
check "Kernel boots"                     "nullbox: virtual filesystems mounted"
check "Virtual filesystems mounted"      "nullbox: virtual filesystems mounted"
check "SquashFS root found"              "nullbox: mounting embedded SquashFS"
check "Pivot to SquashFS"                "nullbox: pivoting to SquashFS root"

# nulld checks
check "nulld starts as PID 1"           "nulld: checking for persistent data partition"
check "nulld mounts filesystems"        "nulld: installing signal handlers"

# Service startup checks
check "egress starts"                    "egress: starting"
check "egress socket ready"              "egress: listening"
check "ctxgraph starts"                  "ctxgraph:"
check "warden starts"                    "warden: starting"
check "warden socket ready"              "warden: listening"
check "cage starts"                      "cage: starting"
check "KVM available"                    "cage: KVM available"

# Agent checks
check "Agent manifest loaded"           "cage: found agent"
check "Agent auto-started"              "cage: auto-started"

# Egress integration
check "Egress add-agent called"         "egress: adding agent"

# ctxgraph integration (test-agent writes via TSI)
check "test-agent booted in VM"         "test-agent: booted"
check "test-agent ctxgraph write"       "test-agent: wrote to ctxgraph"

# Egress rules
check "Egress rules applied"            "allowed IPs), rules applied"

echo ""
echo "--- Summary ---"
echo ""
echo -e "  ${GREEN}Passed: ${PASS}${NC}"
echo -e "  ${RED}Failed: ${FAIL}${NC}"
echo ""
echo "  Full log: ${LOG_FILE}"
echo ""

if [[ ${FAIL} -gt 0 ]]; then
    echo -e "${YELLOW}>>> Relevant log lines for failed checks:${NC}"
    echo ""
    # Show last 30 lines of boot log for debugging
    tail -30 "${LOG_FILE}" | sed 's/^/  /'
    echo ""
    exit 1
fi

echo -e "${GREEN}=== All checks passed ===${NC}"
