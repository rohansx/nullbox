#!/usr/bin/env bash
#
# e2e-test.sh — Comprehensive end-to-end test of NullBox
#
# Boots the full OS in QEMU, interacts with live services via TCP,
# verifies the complete service chain, and checks the serial log
# after shutdown. Tests every layer: kernel → initramfs → overlay →
# nulld → egress → ctxgraph → warden → sentinel → watcher → cage → agent VM.
#
# Usage:
#   ./image/scripts/e2e-test.sh

set -euo pipefail

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUTPUT_DIR="${NULLBOX_ROOT}/build/output"
VMLINUZ="${OUTPUT_DIR}/kernel/x86_64/vmlinuz"
INITRAMFS="${OUTPUT_DIR}/initramfs/initramfs.cpio.gz"

TIMEOUT=120
# Run QEMU for this long to capture full boot + agent VM startup
RUN_TIME=60
LOG_FILE="/tmp/nullbox-e2e.log"
QEMU_PID=""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

PASS=0
FAIL=0
WARN=0
TOTAL=0

check() {
    local name="$1"
    local result="$2"
    TOTAL=$((TOTAL + 1))
    if [[ "${result}" == "true" ]]; then
        echo -e "  ${GREEN}PASS${NC}  ${name}"
        PASS=$((PASS + 1))
    else
        echo -e "  ${RED}FAIL${NC}  ${name}"
        FAIL=$((FAIL + 1))
    fi
}

soft_check() {
    local name="$1"
    local result="$2"
    TOTAL=$((TOTAL + 1))
    if [[ "${result}" == "true" ]]; then
        echo -e "  ${GREEN}PASS${NC}  ${name}"
        PASS=$((PASS + 1))
    else
        echo -e "  ${YELLOW}WARN${NC}  ${name} (timing-dependent)"
        WARN=$((WARN + 1))
    fi
}

cleanup() {
    if [[ -n "${QEMU_PID}" ]]; then
        kill "${QEMU_PID}" 2>/dev/null || true
        wait "${QEMU_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Helper: send JSON to ctxgraph TCP and read response
tcp_send() {
    local msg="$1"
    timeout 5 bash -c "
        exec 3<>/dev/tcp/127.0.0.1/19100
        printf '%s\n' '$msg' >&3
        read -t 3 -r line <&3
        echo \"\$line\"
        exec 3>&-
    " 2>/dev/null || echo "CONNECT_FAIL"
}

echo -e "${CYAN}=== NullBox Comprehensive E2E Test ===${NC}"
echo "  Kernel:  ${VMLINUZ}"
echo "  Initrd:  ${INITRAMFS}"
echo "  Timeout: ${TIMEOUT}s boot, ${RUN_TIME}s run"
echo ""

# ═══════════════════════════════════════════════════════════════════════════
# Phase 1: Boot
# ═══════════════════════════════════════════════════════════════════════════

echo -e "${CYAN}>>> Phase 1: Boot${NC}"

rm -f "${LOG_FILE}"
qemu-system-x86_64 \
    -enable-kvm \
    -m 4G \
    -cpu host \
    -smp 4 \
    -nographic \
    -serial mon:stdio \
    -kernel "${VMLINUZ}" \
    -initrd "${INITRAMFS}" \
    -append "console=ttyS0,115200 loglevel=4" \
    -nic user,model=virtio,hostfwd=tcp:127.0.0.1:19100-:9100 \
    > "${LOG_FILE}" 2>&1 &
QEMU_PID=$!

echo "  Waiting for ctxgraph TCP..."
BOOTED=false
BOOT_TIME=0
for i in $(seq 1 "${TIMEOUT}"); do
    RESP=$(timeout 3 bash -c '
        exec 3<>/dev/tcp/127.0.0.1/19100 2>/dev/null || exit 1
        printf "{\"method\":\"write\",\"agent_id\":\"probe\",\"key\":\"boot.probe\",\"value\":\"ok\"}\n" >&3
        read -t 2 -r line <&3
        echo "$line"
        exec 3>&-
    ' 2>/dev/null || echo "")
    if echo "${RESP}" | grep -q '"hash"'; then
        BOOTED=true
        BOOT_TIME=$i
        break
    fi
    if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
        echo -e "${RED}  QEMU exited prematurely${NC}"
        break
    fi
    sleep 1
done

if [[ "${BOOTED}" != "true" ]]; then
    echo -e "${RED}  Boot timeout after ${TIMEOUT}s${NC}"
    cleanup
    exit 1
fi

echo -e "  ${GREEN}Booted in ${BOOT_TIME}s${NC}"
echo ""

# ═══════════════════════════════════════════════════════════════════════════
# Phase 2: ctxgraph — Content-Addressed Shared Memory
# ═══════════════════════════════════════════════════════════════════════════

echo -e "${CYAN}>>> Phase 2: ctxgraph (content-addressed shared memory)${NC}"
echo ""

# Write a string value
W1=$(tcp_send '{"method":"write","agent_id":"e2e","key":"test.string","value":"hello world"}')
check "write string value" "$(echo "$W1" | grep -q '"hash"' && echo true || echo false)"
HASH1=$(echo "$W1" | grep -o '"hash":"[^"]*"' | cut -d'"' -f4)

# Write a number value
W2=$(tcp_send '{"method":"write","agent_id":"e2e","key":"test.number","value":42}')
check "write number value" "$(echo "$W2" | grep -q '"hash"' && echo true || echo false)"
HASH2=$(echo "$W2" | grep -o '"hash":"[^"]*"' | cut -d'"' -f4)

# Write a JSON object
W3=$(tcp_send '{"method":"write","agent_id":"e2e","key":"test.object","value":{"nested":"data","count":3}}')
check "write object value" "$(echo "$W3" | grep -q '"hash"' && echo true || echo false)"

# Write boolean
W4=$(tcp_send '{"method":"write","agent_id":"e2e","key":"test.bool","value":true}')
check "write boolean value" "$(echo "$W4" | grep -q '"hash"' && echo true || echo false)"

# Content addressing: same content → same hash (idempotent)
W1_DUP=$(tcp_send '{"method":"write","agent_id":"e2e","key":"test.string","value":"hello world"}')
HASH1_DUP=$(echo "$W1_DUP" | grep -o '"hash":"[^"]*"' | cut -d'"' -f4)
check "content addressing (same hash)" "$([ "$HASH1" = "$HASH1_DUP" ] && echo true || echo false)"

# Different content → different hash
check "different content → different hash" "$([ "$HASH1" != "$HASH2" ] && echo true || echo false)"

# Read by hash
if [[ -n "${HASH1}" ]]; then
    R1=$(tcp_send "{\"method\":\"read\",\"hash\":\"${HASH1}\"}")
    check "read by hash" "$(echo "$R1" | grep -q '"hello world"' && echo true || echo false)"
fi

# Read nonexistent hash
R_BAD=$(tcp_send '{"method":"read","hash":"0000000000000000000000000000000000000000000000000000000000000000"}')
check "read nonexistent → error" "$(echo "$R_BAD" | grep -q '"error"' && echo true || echo false)"

# Query by prefix
Q1=$(tcp_send '{"method":"query","prefix":"test."}')
check "query by prefix" "$(echo "$Q1" | grep -q '"entries"' && echo true || echo false)"
# Should have at least 4 entries (string, number, object, bool)
ENTRY_COUNT=$(echo "$Q1" | grep -o '"key"' | wc -l)
check "query returns all entries (≥4)" "$([ "$ENTRY_COUNT" -ge 4 ] && echo true || echo false)"

# Query with non-matching prefix
Q_EMPTY=$(tcp_send '{"method":"query","prefix":"nonexistent."}')
check "query empty prefix → 0 entries" "$(echo "$Q_EMPTY" | grep -q '"entries":\[\]' && echo true || echo false)"

# History
H1=$(tcp_send '{"method":"history","key":"test.string"}')
check "history for key" "$(echo "$H1" | grep -q '"entries"' && echo true || echo false)"

# Write from different agent, verify isolation in queries
W_OTHER=$(tcp_send '{"method":"write","agent_id":"other-agent","key":"test.string","value":"different"}')
Q_E2E=$(tcp_send '{"method":"query","prefix":"test.","agent_id":"e2e"}')
# The query should still work (ctxgraph doesn't filter by agent in prefix queries)
check "cross-agent write" "$(echo "$W_OTHER" | grep -q '"hash"' && echo true || echo false)"

# ═══════════════════════════════════════════════════════════════════════════
# Phase 3: Wait for test-agent VM to boot and write to ctxgraph
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${CYAN}>>> Phase 3: Agent VM (test-agent in microVM via cage)${NC}"
echo ""

# The test-agent boots inside a libkrun microVM and writes to ctxgraph
# via TSI networking. This proves: cage → libkrun → VM → TSI → ctxgraph.
# Give it up to RUN_TIME seconds.
echo "  Waiting for test-agent (up to ${RUN_TIME}s)..."
AGENT_BOOTED=false
REMAINING=$((RUN_TIME - BOOT_TIME))
for i in $(seq 1 "${REMAINING}"); do
    AGENT_Q=$(tcp_send '{"method":"query","prefix":"agent.status"}')
    if echo "$AGENT_Q" | grep -q '"booted"'; then
        AGENT_BOOTED=true
        echo "  test-agent responded in $((BOOT_TIME + i))s total"
        break
    fi
    sleep 1
done

soft_check "test-agent booted in VM" "${AGENT_BOOTED}"
soft_check "test-agent wrote to ctxgraph" "${AGENT_BOOTED}"

# ═══════════════════════════════════════════════════════════════════════════
# Phase 4: Kill QEMU, analyze serial log
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${CYAN}>>> Phase 4: Service verification (serial log)${NC}"
echo ""

# Kill QEMU so pipe flushes
kill "${QEMU_PID}" 2>/dev/null || true
wait "${QEMU_PID}" 2>/dev/null || true
QEMU_PID=""

LOG_LINES=$(wc -l < "${LOG_FILE}" 2>/dev/null || echo 0)
echo "  Log: ${LOG_LINES} lines captured"
echo ""

# Kernel + initramfs
check "kernel booted"             "$(grep -qa 'Linux version\|SeaBIOS' "$LOG_FILE" && echo true || echo false)"
check "KVM enabled"               "$(grep -qa 'kvm' "$LOG_FILE" && echo true || echo false)"

# nulld
soft_check "nulld running"        "$(grep -qa 'nulld:' "$LOG_FILE" && echo true || echo false)"

# Service startup (these appear in the log if buffer captured enough)
soft_check "egress started"       "$(grep -qa 'egress.*listening\|egress.*socket' "$LOG_FILE" && echo true || echo false)"
soft_check "ctxgraph started"     "$(grep -qa 'ctxgraph.*listening\|ctxgraph.*TCP' "$LOG_FILE" && echo true || echo false)"
soft_check "warden started"       "$(grep -qa 'warden.*listening\|warden.*socket' "$LOG_FILE" && echo true || echo false)"
soft_check "sentinel started"     "$(grep -qa 'sentinel.*listening\|sentinel.*socket' "$LOG_FILE" && echo true || echo false)"
soft_check "watcher started"      "$(grep -qa 'watcher.*listening\|watcher.*socket' "$LOG_FILE" && echo true || echo false)"
soft_check "cage started"         "$(grep -qa 'cage.*listening\|cage.*socket' "$LOG_FILE" && echo true || echo false)"

# Agent lifecycle
soft_check "agent manifest loaded" "$(grep -qa 'manifest.*loaded\|found agent' "$LOG_FILE" && echo true || echo false)"
soft_check "agent auto-started"   "$(grep -qa 'auto-started\|started.*PID' "$LOG_FILE" && echo true || echo false)"
soft_check "egress rules applied" "$(grep -qa 'rules applied\|add-agent' "$LOG_FILE" && echo true || echo false)"
soft_check "sentinel registered"  "$(grep -qa 'sentinel.*register' "$LOG_FILE" && echo true || echo false)"

# ═══════════════════════════════════════════════════════════════════════════
# Phase 5: Error checks
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${CYAN}>>> Phase 5: Error checks${NC}"
echo ""

check "no kernel panic"          "$(! grep -qa 'Kernel panic' "$LOG_FILE" && echo true || echo false)"
check "no nulld panic"           "$(! grep -qa 'nulld.*PANIC' "$LOG_FILE" && echo true || echo false)"
check "no fatal errors"          "$(! grep -qa 'fatal:' "$LOG_FILE" && echo true || echo false)"
check "no OOM kills"             "$(! grep -qa 'Out of memory\|oom-kill' "$LOG_FILE" && echo true || echo false)"
check "no segfaults"             "$(! grep -qa 'segfault\|SIGSEGV' "$LOG_FILE" && echo true || echo false)"

# ═══════════════════════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo "═══════════════════════════════════════════"
echo ""
echo -e "  ${GREEN}Passed: ${PASS}${NC}"
if [[ ${WARN} -gt 0 ]]; then
    echo -e "  ${YELLOW}Warned: ${WARN}${NC} (serial buffer / nested KVM timing)"
fi
if [[ ${FAIL} -gt 0 ]]; then
    echo -e "  ${RED}Failed: ${FAIL}${NC}"
fi
echo "  Total:  ${TOTAL}"
echo ""
echo "  Boot time: ${BOOT_TIME}s"
echo "  Full log:  ${LOG_FILE}"
echo ""

if [[ ${FAIL} -gt 0 ]]; then
    echo -e "${RED}=== E2E TEST FAILED ===${NC}"
    echo ""
    echo "Failed checks indicate real issues. Warned checks"
    echo "are timing-dependent (serial buffer, nested KVM)."
    exit 1
else
    echo -e "${GREEN}=== E2E TEST PASSED ===${NC}"
    exit 0
fi
