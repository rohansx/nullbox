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

PRODUCTION=0
for arg in "$@"; do
    case "${arg}" in
        --production) PRODUCTION=1 ;;
        --arch=*) ARCH="${arg#*=}" ;;
    esac
done

NULLBOX_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${ARCH:-${TARGET:-x86_64-unknown-linux-musl}}"
if [[ -n "${ARCH:-}" ]]; then
    TARGET="${ARCH}-unknown-linux-musl"
fi
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

# Sentinel is a separate project — build and copy from its own directory
SENTINEL_DIR="${NULLBOX_ROOT}/../sentinel"
if [[ -d "${SENTINEL_DIR}" ]]; then
    SENTINEL_BIN="${SENTINEL_DIR}/target/${TARGET}/release/sentinel"
    SENTINEL_BIN_FB="${SENTINEL_DIR}/target/release/sentinel"
    if [[ -f "${SENTINEL_BIN}" ]]; then
        cp "${SENTINEL_BIN}" "${BUILD_DIR}/system/bin/sentinel"
        echo "  Copied sentinel (musl static)"
    elif [[ -f "${SENTINEL_BIN_FB}" ]]; then
        cp "${SENTINEL_BIN_FB}" "${BUILD_DIR}/system/bin/sentinel"
        echo "  Copied sentinel (release)"
    else
        echo "  WARNING: sentinel binary not found — build it with: cd ../sentinel && cargo build --release"
    fi
else
    echo "  WARNING: sentinel project not found at ${SENTINEL_DIR}"
fi

# Watcher is a separate project — build and copy from its own directory
WATCHER_DIR="${NULLBOX_ROOT}/../watcher"
if [[ -d "${WATCHER_DIR}" ]]; then
    WATCHER_BIN="${WATCHER_DIR}/target/${TARGET}/release/watcher"
    WATCHER_BIN_FB="${WATCHER_DIR}/target/release/watcher"
    if [[ -f "${WATCHER_BIN}" ]]; then
        cp "${WATCHER_BIN}" "${BUILD_DIR}/system/bin/watcher"
        echo "  Copied watcher (musl static)"
    elif [[ -f "${WATCHER_BIN_FB}" ]]; then
        cp "${WATCHER_BIN_FB}" "${BUILD_DIR}/system/bin/watcher"
        echo "  Copied watcher (release)"
    else
        echo "  WARNING: watcher binary not found — build it with: cd ../watcher && cargo build --release"
    fi
else
    echo "  WARNING: watcher project not found at ${WATCHER_DIR}"
fi

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

[service.sentinel]
binary = "/system/bin/sentinel"
args = []
depends_on = []
restart = "always"

[service.watcher]
binary = "/system/bin/watcher"
args = []
depends_on = []
restart = "always"

[service.cage]
binary = "/system/bin/cage"
depends_on = ["egress", "ctxgraph", "warden", "sentinel", "watcher"]
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

# Create example AGENT.toml for testing (dev only)
if [[ "${PRODUCTION}" != "1" ]]; then
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
fi

# Generate warden master key for dev/testing (not for production)
if [[ "${PRODUCTION}" != "1" ]]; then
    echo ">>> Generating warden master key..."
    mkdir -p "${BUILD_DIR}/vault"
    dd if=/dev/urandom bs=32 count=1 2>/dev/null > "${BUILD_DIR}/vault/master.key"
    chmod 600 "${BUILD_DIR}/vault/master.key"
    echo "  Generated /vault/master.key (32 bytes, dev only — use sealed secrets in production)"
fi

# Tighten permissions
echo ">>> Tightening permissions..."
# cage needs world-readable (555): libkrun VMM drops privileges internally.
# Other binaries are static and run as root — 555 is safe, 500 breaks cage.
chmod 555 "${BUILD_DIR}/system/bin/"*
chmod 644 "${BUILD_DIR}/system/config/nulld.toml"
chmod 755 "${BUILD_DIR}/etc"
chmod 644 "${BUILD_DIR}/etc/"*
if [[ -d "${BUILD_DIR}/vault" ]]; then
    chmod 700 "${BUILD_DIR}/vault"
fi

# Production safety checks
if [[ "${PRODUCTION}" == "1" ]]; then
    KEY_FILES=$(find "${BUILD_DIR}" -name "*.key" 2>/dev/null || true)
    if [[ -n "${KEY_FILES}" ]]; then
        echo "error: production build contains key files:"
        echo "${KEY_FILES}"
        exit 1
    fi
    echo "  Mode: PRODUCTION (no test-agent, no dev keys)"
fi

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
