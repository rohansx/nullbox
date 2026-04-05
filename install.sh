#!/usr/bin/env bash
#
# install.sh — One-command NullBox installer
#
# What it does:
#   1. Installs QEMU (if not present)
#   2. Downloads the NullBox ISO (64MB)
#   3. Runs NullBox in a VM on your existing OS
#   4. Opens the web dashboard in your browser
#
# Your OS is NOT replaced. NullBox runs inside a lightweight VM.
#
# Usage:
#   curl -fsSL https://nullbox.dev/install.sh | bash
#   ./install.sh [--iso path/to/nullbox.iso] [--memory 4096] [--cpus 4]
#
# Supports: Linux (x86_64 with KVM), macOS (Intel + Apple Silicon)

set -euo pipefail

NULLBOX_VERSION="0.1.0"
ISO_URL="https://github.com/rohansx/nullbox/releases/download/v${NULLBOX_VERSION}/nullbox-x86_64.iso"
NULLBOX_DIR="${HOME}/.nullbox"
MEMORY=4096
CPUS=4
ISO_PATH=""

for arg in "$@"; do
    case "${arg}" in
        --iso=*) ISO_PATH="${arg#*=}" ;;
        --memory=*) MEMORY="${arg#*=}" ;;
        --cpus=*) CPUS="${arg#*=}" ;;
    esac
done

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

echo -e "${CYAN}${BOLD}"
echo "  _   _       _ _ ____            "
echo " | \ | |_   _| | | __ )  _____  __"
echo " |  \| | | | | | |  _ \ / _ \ \/ /"
echo " | |\  | |_| | | | |_) | (_) >  < "
echo " |_| \_|\__,_|_|_|____/ \___/_/\_\\"
echo -e "${NC}"
echo "  The Operating System for AI Agents"
echo "  v${NULLBOX_VERSION}"
echo ""

OS="$(uname -s)"
ARCH="$(uname -m)"

# ── Install QEMU ─────────────────────────────────────────────────────────

install_qemu() {
    echo -e "${CYAN}>>> Installing QEMU...${NC}"

    case "${OS}" in
        Linux)
            if command -v pacman &>/dev/null; then
                sudo pacman -S --needed --noconfirm qemu-system-x86 2>/dev/null
            elif command -v apt-get &>/dev/null; then
                sudo apt-get install -y qemu-system-x86 qemu-utils 2>/dev/null
            elif command -v dnf &>/dev/null; then
                sudo dnf install -y qemu-system-x86-core 2>/dev/null
            elif command -v zypper &>/dev/null; then
                sudo zypper install -y qemu-x86 2>/dev/null
            else
                echo -e "${RED}Cannot auto-install QEMU. Install it manually:${NC}"
                echo "  https://www.qemu.org/download/"
                exit 1
            fi
            ;;
        Darwin)
            if command -v brew &>/dev/null; then
                brew install qemu
            else
                echo -e "${RED}Homebrew required. Install it:${NC}"
                echo '  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"'
                echo "  Then run this script again."
                exit 1
            fi
            ;;
    esac
}

# ── Check prerequisites ──────────────────────────────────────────────────

echo -e "${CYAN}>>> Checking prerequisites...${NC}"

# QEMU
if ! command -v qemu-system-x86_64 &>/dev/null; then
    install_qemu
fi

QEMU_VER=$(qemu-system-x86_64 --version | head -1)
echo "  QEMU: ${QEMU_VER}"

# KVM (Linux only)
ACCEL="tcg"
if [[ "${OS}" == "Linux" ]]; then
    if [[ -e /dev/kvm ]]; then
        ACCEL="kvm"
        echo -e "  KVM:  ${GREEN}available${NC} (native speed)"
    else
        echo -e "  KVM:  ${RED}not available${NC} (will run slower)"
        echo "  Enable VT-x/AMD-V in BIOS for best performance."
    fi
elif [[ "${OS}" == "Darwin" ]]; then
    # macOS: use HVF on Intel, TCG on Apple Silicon (x86 emulation)
    if [[ "${ARCH}" == "x86_64" ]]; then
        ACCEL="hvf"
        echo -e "  HVF:  ${GREEN}available${NC} (Intel Mac)"
    else
        echo "  Note: Apple Silicon — x86 emulation (slower). ARM64 ISO coming soon."
    fi
fi
echo ""

# ── Download ISO ─────────────────────────────────────────────────────────

mkdir -p "${NULLBOX_DIR}"
ISO_DEST="${NULLBOX_DIR}/nullbox-x86_64.iso"

if [[ -n "${ISO_PATH}" && -f "${ISO_PATH}" ]]; then
    echo -e "${CYAN}>>> Using local ISO: ${ISO_PATH}${NC}"
    cp "${ISO_PATH}" "${ISO_DEST}"
elif [[ -f "${ISO_DEST}" ]]; then
    echo -e "${CYAN}>>> ISO already downloaded: ${ISO_DEST}${NC}"
else
    echo -e "${CYAN}>>> Downloading NullBox ISO (~64MB)...${NC}"
    if command -v curl &>/dev/null; then
        curl -fSL --progress-bar -o "${ISO_DEST}" "${ISO_URL}"
    elif command -v wget &>/dev/null; then
        wget -q --show-progress -O "${ISO_DEST}" "${ISO_URL}"
    fi
fi
echo ""

# ── Create launcher script ───────────────────────────────────────────────

LAUNCHER="${NULLBOX_DIR}/start.sh"
cat > "${LAUNCHER}" << LAUNCHER_EOF
#!/usr/bin/env bash
# NullBox launcher — run with: ~/.nullbox/start.sh
ISO="${ISO_DEST}"
MEMORY=${MEMORY}
CPUS=${CPUS}
ACCEL=${ACCEL}

echo "Starting NullBox v${NULLBOX_VERSION}..."
echo "  Dashboard: http://localhost:8080"
echo "  Memory:    \${MEMORY}MB"
echo "  CPUs:      \${CPUS}"
echo "  Accel:     \${ACCEL}"
echo ""
echo "  Press Ctrl-A X to stop."
echo ""

exec qemu-system-x86_64 \\
    -accel \${ACCEL} \\
    -m \${MEMORY} \\
    -cpu \$([ "\${ACCEL}" = "kvm" ] && echo "host" || echo "max") \\
    -smp \${CPUS} \\
    -nographic \\
    -serial mon:stdio \\
    -boot d \\
    -cdrom "\${ISO}" \\
    -nic user,model=virtio,hostfwd=tcp::8080-:8080,hostfwd=tcp::19100-:9100,hostfwd=tcp::19200-:9200
LAUNCHER_EOF
chmod +x "${LAUNCHER}"

# ── Create stop script ───────────────────────────────────────────────────

cat > "${NULLBOX_DIR}/stop.sh" << 'STOP_EOF'
#!/usr/bin/env bash
pkill -f "qemu-system.*nullbox" 2>/dev/null && echo "NullBox stopped." || echo "NullBox not running."
STOP_EOF
chmod +x "${NULLBOX_DIR}/stop.sh"

# ── Start NullBox ────────────────────────────────────────────────────────

echo -e "${CYAN}>>> Starting NullBox...${NC}"
echo ""

# Run in background
nohup "${LAUNCHER}" > "${NULLBOX_DIR}/nullbox.log" 2>&1 &
QEMU_PID=$!
echo "${QEMU_PID}" > "${NULLBOX_DIR}/nullbox.pid"

# Wait for dashboard to come up
echo "  Waiting for services..."
READY=false
for i in $(seq 1 60); do
    if timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/8080 2>/dev/null && exec 3>&-' 2>/dev/null; then
        READY=true
        echo -e "  ${GREEN}Ready in ${i}s${NC}"
        break
    fi
    if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
        echo -e "${RED}  QEMU exited. Check ${NULLBOX_DIR}/nullbox.log${NC}"
        break
    fi
    sleep 1
done

echo ""
if [[ "${READY}" == "true" ]]; then
    echo -e "${GREEN}${BOLD}  NullBox is running!${NC}"
    echo ""
    echo "  Dashboard:  http://localhost:8080"
    echo ""
    echo "  Commands:"
    echo "    ~/.nullbox/start.sh    Start NullBox"
    echo "    ~/.nullbox/stop.sh     Stop NullBox"
    echo ""
    echo "  Logs: ~/.nullbox/nullbox.log"
    echo ""

    # Open browser (best effort)
    if command -v xdg-open &>/dev/null; then
        xdg-open "http://localhost:8080" 2>/dev/null &
    elif command -v open &>/dev/null; then
        open "http://localhost:8080" 2>/dev/null &
    fi
else
    echo -e "${RED}  NullBox did not start in time.${NC}"
    echo "  Check: ${NULLBOX_DIR}/nullbox.log"
    echo "  Try: ${LAUNCHER}"
fi
