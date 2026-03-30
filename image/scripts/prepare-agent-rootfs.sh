#!/usr/bin/env bash
#
# prepare-agent-rootfs.sh — Create a minimal rootfs directory for a libkrun agent VM.
#
# Usage:
#   ./prepare-agent-rootfs.sh <agent-name> <binary-path> <rootfs-dir>
#
# Example:
#   ./prepare-agent-rootfs.sh test-agent target/x86_64-unknown-linux-musl/release/test-agent /tmp/rootfs/test-agent

set -euo pipefail

AGENT_NAME="${1:?usage: prepare-agent-rootfs.sh <name> <binary> <rootfs-dir>}"
BINARY="${2:?usage: prepare-agent-rootfs.sh <name> <binary> <rootfs-dir>}"
ROOTFS="${3:?usage: prepare-agent-rootfs.sh <name> <binary> <rootfs-dir>}"

if [[ ! -f "${BINARY}" ]]; then
    echo "error: binary not found: ${BINARY}"
    exit 1
fi

# Create minimal rootfs layout
# libkrun's init.krun handles /proc, /sys, /dev mounts inside the guest
mkdir -p "${ROOTFS}"/{agent/bin,tmp,proc,sys,dev,etc,run}

# Create mount points for virtiofs shares declared in AGENT.toml
# Default dirs that agents commonly declare in capabilities.filesystem
mkdir -p "${ROOTFS}"/{data,data/output}

# Copy the agent binary
cp "${BINARY}" "${ROOTFS}/agent/bin/${AGENT_NAME}"
chmod +x "${ROOTFS}/agent/bin/${AGENT_NAME}"

# Minimal /etc
echo "nameserver 1.1.1.1" > "${ROOTFS}/etc/resolv.conf"
echo "${AGENT_NAME}" > "${ROOTFS}/etc/hostname"

echo "  Prepared rootfs for '${AGENT_NAME}' at ${ROOTFS}"
