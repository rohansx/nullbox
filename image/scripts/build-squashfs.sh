#!/usr/bin/env bash
#
# build-squashfs.sh — Build the NullBox SquashFS root filesystem
#
# Creates a read-only SquashFS image containing:
#   - /system/bin/  — all NullBox binaries (statically linked)
#   - /system/config/ — nulld.toml service configuration
#   - /etc/ — minimal system config (hostname, resolv.conf)
#   - Empty mount points for tmpfs, overlay, etc.
#
# Usage:
#   ./image/scripts/build-squashfs.sh [--target x86_64-unknown-linux-musl]

set -euo pipefail

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
BUILD_DIR="${NULLBOX_ROOT}/build/squashfs-staging"
OUTPUT_DIR="${NULLBOX_ROOT}/build/output/squashfs"

echo "=== NullBox SquashFS Build ==="
echo "  Target: ${TARGET}"

# Check for mksquashfs
if ! command -v mksquashfs &>/dev/null; then
    echo "error: mksquashfs not found. Install: pacman -S squashfs-tools"
    exit 1
fi

mkdir -p "${OUTPUT_DIR}"

# Clean previous build
rm -rf "${BUILD_DIR:?}"

# Create filesystem layout (from ARCHITECTURE.md)
mkdir -p "${BUILD_DIR}"/{system/bin,system/config,etc,tmp,var/lib/ctxgraph,agent,vault,snapshots,proc,sys,dev,run,data}
mkdir -p "${BUILD_DIR}/dev"/{pts,shm}

# Copy binaries
RELEASE_DIR="${NULLBOX_ROOT}/target/${TARGET}/release"
FALLBACK_DIR="${NULLBOX_ROOT}/target/release"

copy_binary() {
    local name="$1"
    if [[ -f "${RELEASE_DIR}/${name}" ]]; then
        cp "${RELEASE_DIR}/${name}" "${BUILD_DIR}/system/bin/${name}"
        echo "  Copied ${name} (musl static)"
    elif [[ -f "${FALLBACK_DIR}/${name}" ]]; then
        cp "${FALLBACK_DIR}/${name}" "${BUILD_DIR}/system/bin/${name}"
        echo "  Copied ${name} (release — not static, dev only)"
    else
        echo "  WARNING: ${name} binary not found, skipping"
    fi
}

echo ">>> Copying binaries..."
copy_binary nulld
copy_binary nullctl
copy_binary egress
copy_binary ctxgraph
copy_binary warden
copy_binary cage

# cage links dynamically against libkrun — copy required shared libraries
echo ">>> Copying shared libraries for cage..."
mkdir -p "${BUILD_DIR}/usr/lib"
CAGE_BIN="${BUILD_DIR}/system/bin/cage"
if [[ -f "${CAGE_BIN}" ]] && ldd "${CAGE_BIN}" &>/dev/null; then
    # Copy libkrun and libkrunfw
    for lib in libkrun libkrunfw; do
        SO_FILE=$(find /usr/lib -maxdepth 1 -name "${lib}.so.*" ! -type l | head -1)
        if [[ -n "${SO_FILE}" ]]; then
            cp "${SO_FILE}" "${BUILD_DIR}/usr/lib/"
            # Create symlinks
            SONAME=$(objdump -p "${SO_FILE}" 2>/dev/null | grep SONAME | awk '{print $2}')
            if [[ -n "${SONAME}" ]]; then
                ln -sf "$(basename "${SO_FILE}")" "${BUILD_DIR}/usr/lib/${SONAME}"
            fi
            ln -sf "$(basename "${SO_FILE}")" "${BUILD_DIR}/usr/lib/${lib}.so"
            echo "  Copied ${lib}"
        fi
    done
    # Copy the dynamic linker
    if [[ -f /lib64/ld-linux-x86-64.so.2 ]]; then
        mkdir -p "${BUILD_DIR}/lib64"
        cp /lib64/ld-linux-x86-64.so.2 "${BUILD_DIR}/lib64/"
        echo "  Copied ld-linux"
    fi
    # Copy all other shared lib deps
    ldd "${CAGE_BIN}" 2>/dev/null | grep "=> /" | awk '{print $3}' | while read -r lib; do
        if [[ ! -f "${BUILD_DIR}/usr/lib/$(basename "${lib}")" ]]; then
            cp "${lib}" "${BUILD_DIR}/usr/lib/"
        fi
    done
    echo "  Copied runtime dependencies"
fi

# Copy nft binary and its shared libraries (needed by egress)
echo ">>> Copying nftables tooling..."
NFT_BIN=$(command -v nft 2>/dev/null || true)
if [[ -n "${NFT_BIN}" ]]; then
    cp "${NFT_BIN}" "${BUILD_DIR}/system/bin/nft"
    chmod +x "${BUILD_DIR}/system/bin/nft"
    echo "  Copied nft"
    # Copy nft's shared library dependencies
    ldd "${NFT_BIN}" 2>/dev/null | grep "=> /" | awk '{print $3}' | while read -r lib; do
        BASENAME=$(basename "${lib}")
        if [[ ! -f "${BUILD_DIR}/usr/lib/${BASENAME}" ]]; then
            cp "${lib}" "${BUILD_DIR}/usr/lib/"
            echo "  Copied ${BASENAME}"
        fi
    done
    # Ensure dynamic linker is present
    if [[ -f /lib64/ld-linux-x86-64.so.2 ]] && [[ ! -f "${BUILD_DIR}/lib64/ld-linux-x86-64.so.2" ]]; then
        mkdir -p "${BUILD_DIR}/lib64"
        cp /lib64/ld-linux-x86-64.so.2 "${BUILD_DIR}/lib64/"
        echo "  Copied ld-linux"
    fi
else
    echo "  WARNING: nft not found — egress cannot apply rules at runtime"
fi

# Create nulld.toml service configuration
cat > "${BUILD_DIR}/system/config/nulld.toml" << 'EOF'
# NullBox v0.1 service configuration
# Services are started in dependency order by nulld.

[service.egress]
binary = "/system/bin/egress"
args = []
depends_on = []
restart = "always"

[service.ctxgraph]
binary = "/system/bin/ctxgraph"
args = []
depends_on = []
restart = "always"

[service.warden]
binary = "/system/bin/warden"
args = []
depends_on = []
restart = "always"

[service.cage]
binary = "/system/bin/cage"
depends_on = ["egress", "ctxgraph", "warden"]
restart = "always"
EOF
echo "  Created nulld.toml"

# Create minimal /etc
echo "nullbox" > "${BUILD_DIR}/etc/hostname"
echo "nameserver 1.1.1.1" > "${BUILD_DIR}/etc/resolv.conf"
cat > "${BUILD_DIR}/etc/os-release" << 'EOF'
NAME="NullBox"
VERSION="0.1.0"
ID=nullbox
PRETTY_NAME="NullBox v0.1.0"
HOME_URL="https://github.com/nullbox-os/nullbox"
EOF
echo "  Created /etc files"

# Create example AGENT.toml for testing
mkdir -p "${BUILD_DIR}/agent"
cat > "${BUILD_DIR}/agent/test-agent.toml" << 'EOF'
[agent]
name = "test-agent"
version = "0.1.0"

[capabilities]
max_cpu_percent = 25
max_memory_mb = 256

[capabilities.network]
allow = ["httpbin.org"]

[capabilities.filesystem]
read = ["/data"]
write = ["/data/output"]
EOF
echo "  Created test AGENT.toml"

# Build and install test-agent rootfs
echo ">>> Preparing test-agent rootfs..."
TEST_AGENT_BIN="${RELEASE_DIR}/test-agent"
if [[ ! -f "${TEST_AGENT_BIN}" ]]; then
    TEST_AGENT_BIN="${NULLBOX_ROOT}/target/${TARGET}/release/test-agent"
fi
if [[ -f "${TEST_AGENT_BIN}" ]]; then
    AGENT_ROOTFS="${BUILD_DIR}/system/rootfs/test-agent"
    chmod +x "${NULLBOX_ROOT}/image/scripts/prepare-agent-rootfs.sh"
    "${NULLBOX_ROOT}/image/scripts/prepare-agent-rootfs.sh" test-agent "${TEST_AGENT_BIN}" "${AGENT_ROOTFS}"
else
    echo "  WARNING: test-agent binary not found, skipping rootfs"
fi

# Generate warden master key for dev/testing
echo ">>> Generating warden master key..."
mkdir -p "${BUILD_DIR}/vault"
dd if=/dev/urandom bs=32 count=1 2>/dev/null > "${BUILD_DIR}/vault/master.key"
chmod 600 "${BUILD_DIR}/vault/master.key"
echo "  Generated /vault/master.key (32 bytes, dev only — use sealed secrets in production)"

# Build SquashFS image with zstd compression
echo ">>> Building SquashFS image..."
mksquashfs "${BUILD_DIR}" "${OUTPUT_DIR}/nullbox.squashfs" \
    -comp zstd \
    -Xcompression-level 19 \
    -all-root \
    -noappend \
    -quiet

SQUASHFS_SIZE=$(du -h "${OUTPUT_DIR}/nullbox.squashfs" | cut -f1)

echo ""
echo "=== SquashFS build complete ==="
echo "  Output:  ${OUTPUT_DIR}/nullbox.squashfs"
echo "  Size:    ${SQUASHFS_SIZE}"
echo "  Target:  <100MB"
echo ""
echo "  Contents:"
ls -la "${BUILD_DIR}/system/bin/"
