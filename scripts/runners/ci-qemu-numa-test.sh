#!/usr/bin/env bash
# =============================================================================
# ci-qemu-numa-test.sh — NUMA Tier-3 QEMU smoke harness
# =============================================================================
#
# SCOPE (current): SMOKE LEVEL
#   Boots a 2-NUMA-node QEMU guest, verifies the guest kernel exposes
#   two NUMA nodes via the kernel serial-console NUMA discovery messages,
#   then exits. Confirms QEMU correctly models multi-socket topology.
#
# WHY SMOKE AND NOT FULL CARGO-TEST:
#   GitHub-hosted ubuntu-latest runners lack KVM; QEMU runs in TCG (software
#   emulation) which is ~50–100× slower than native. A Rust build + nextest
#   run inside TCG QEMU would reliably exceed the 30-minute CI timeout.
#   See: https://github.com/actions/runner-images/issues/183
#
# PATH TO FULL INTEGRATION (TODOs):
#
#   Option A — Self-hosted runner with KVM (RECOMMENDED):
#     Change numa.yml `runs-on` to [self-hosted, linux, kvm].
#     With KVM, guest boots in ~5s. Full Rust test run takes ~2 min.
#     TODO: Provision a KVM-enabled self-hosted runner, update numa.yml.
#
#   Option B — Cross-compile binary + virtfs 9p share:
#     1. Cross-compile shamir-numa tests for x86_64-unknown-linux-musl on
#        the host (fast, no QEMU needed for compilation):
#           cargo build --tests -p shamir-numa --target x86_64-unknown-linux-musl
#     2. Boot QEMU with a minimal Alpine initrd.
#     3. Mount workspace via:
#           -virtfs local,path=.,mount_tag=workspace,security_model=none
#        and run the pre-compiled test binary inside the guest.
#     TODO: Build Alpine initrd with 9p+virtfs compiled in; extract test
#     binary path from `cargo test --no-run` artefact output; handle
#     nextest vs bare test runner inside the guest.
#
#   Option C — Cloud-init + SSH (heavier):
#     Boot a full Ubuntu cloud image, inject test commands via cloud-init or
#     scp, poll SSH, run cargo inside. Requires ~500 MB image download
#     (cacheable with actions/cache keyed on Ubuntu release).
#     TODO: Prepare cloud-init user-data, add SSH retry loop, cache image.
#
# WHAT THIS SCRIPT DOES NOW:
#   1. Downloads Alpine Linux virt ISO (~60 MB, cached at ~/.cache/).
#   2. Boots QEMU with 2-NUMA-node topology (4 vCPU, 2 GB RAM).
#   3. Polls serial console for kernel NUMA discovery messages:
#        "NUMA: Node 0 [mem …]"
#        "NUMA: Node 1 [mem …]"
#      These appear within 30–90 s of boot start even under TCG, before
#      userland is reached.
#   4. Exits 0 if 2 nodes found; exits 1 on timeout or wrong count.
#
# QEMU TOPOLOGY:
#   -smp 4,sockets=2,cores=2,threads=1   4 vCPU across 2 sockets
#   -m 2G                                 2 GB total RAM
#   node 0: CPUs 0-1, mem 1 GB  (socket 0)
#   node 1: CPUs 2-3, mem 1 GB  (socket 1)
#
# REQUIREMENTS (installed by numa.yml install step):
#   qemu-system-x86 qemu-utils
#   curl (pre-installed on ubuntu-latest)
#   sha256sum (pre-installed, part of coreutils)
#
# =============================================================================
set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# WORKSPACE_DIR is exported for future Option B use (cargo cross-build).
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
export WORKSPACE_DIR

# Alpine Linux "virt" ISO — minimal kernel + initramfs, boots fast.
# "virt" flavour has virtio drivers built in; no installer, just a live env.
ALPINE_VERSION="3.21.3"
ALPINE_IMAGE_URL="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/x86_64/alpine-virt-${ALPINE_VERSION}-x86_64.iso"
ALPINE_IMAGE_SHA256="bbdbe9a08fc5f7547c73b68a34a96b1e80e33c26f3faed8e37a0b13da83de025"

WORK_DIR="${TMPDIR:-/tmp}/shamir-numa-qemu-$$"
IMAGE_CACHE_DIR="${HOME}/.cache/shamir-numa-qemu"
ISO_PATH="${IMAGE_CACHE_DIR}/alpine-virt-${ALPINE_VERSION}-x86_64.iso"

# Serial console output file — we parse this to detect NUMA topology.
SERIAL_LOG="${WORK_DIR}/serial.log"

# How long to wait for kernel NUMA messages (seconds).
# TCG on GitHub runners is slow (~30–90 s to kernel NUMA init).
# 180 s is conservative; within the 30-min job timeout.
BOOT_TIMEOUT=180

# QEMU process ID (set after launch).
QEMU_PID=""

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '[numa-qemu] %s\n' "$*"; }
die()  { printf '[numa-qemu] ERROR: %s\n' "$*" >&2; exit 1; }

cleanup() {
    local rc=$?
    if [[ -n "${QEMU_PID}" ]] && kill -0 "${QEMU_PID}" 2>/dev/null; then
        log "Terminating QEMU (pid=${QEMU_PID})…"
        kill "${QEMU_PID}" 2>/dev/null || true
        # Give it 5 s to exit cleanly before SIGKILL.
        local i=0
        while kill -0 "${QEMU_PID}" 2>/dev/null && [[ $i -lt 5 ]]; do
            sleep 1
            i=$((i + 1))
        done
        kill -9 "${QEMU_PID}" 2>/dev/null || true
    fi
    if [[ -d "${WORK_DIR}" ]]; then
        rm -rf "${WORK_DIR}"
    fi
    exit "${rc}"
}
trap cleanup EXIT INT TERM

require_cmd() {
    local cmd="$1"
    local pkg="${2:-${cmd}}"
    command -v "${cmd}" >/dev/null 2>&1 \
        || die "Required command '${cmd}' not found. Install via: sudo apt-get install -y ${pkg}"
}

# ---------------------------------------------------------------------------
# Dependency check
# ---------------------------------------------------------------------------

require_cmd qemu-system-x86_64 qemu-system-x86
require_cmd curl               curl
require_cmd sha256sum          coreutils

# ---------------------------------------------------------------------------
# Fetch Alpine ISO (with cache)
# ---------------------------------------------------------------------------

mkdir -p "${IMAGE_CACHE_DIR}" "${WORK_DIR}"

if [[ -f "${ISO_PATH}" ]]; then
    log "Alpine ISO found in cache: ${ISO_PATH}"
    cached_sha="$(sha256sum "${ISO_PATH}" | awk '{print $1}')"
    if [[ "${cached_sha}" != "${ALPINE_IMAGE_SHA256}" ]]; then
        log "Cache integrity mismatch (got ${cached_sha}); re-downloading…"
        rm -f "${ISO_PATH}"
    else
        log "Cache integrity OK."
    fi
fi

if [[ ! -f "${ISO_PATH}" ]]; then
    log "Downloading Alpine virt ISO ${ALPINE_VERSION} (~60 MB)…"
    curl -fSL --retry 3 --retry-delay 5 \
        -o "${ISO_PATH}.tmp" \
        "${ALPINE_IMAGE_URL}"

    actual_sha="$(sha256sum "${ISO_PATH}.tmp" | awk '{print $1}')"
    if [[ "${actual_sha}" != "${ALPINE_IMAGE_SHA256}" ]]; then
        rm -f "${ISO_PATH}.tmp"
        die "SHA256 mismatch: expected=${ALPINE_IMAGE_SHA256}, got=${actual_sha}"
    fi

    mv "${ISO_PATH}.tmp" "${ISO_PATH}"
    log "Download complete and verified."
fi

# ---------------------------------------------------------------------------
# Compose and launch QEMU
# ---------------------------------------------------------------------------
#
# NUMA topology flags explained:
#   -object memory-backend-ram,id=mem0,size=1G
#       Creates a named 1 GB memory back-end "mem0" (NUMA node 0's RAM).
#   -object memory-backend-ram,id=mem1,size=1G
#       Creates a named 1 GB memory back-end "mem1" (NUMA node 1's RAM).
#   -numa node,cpus=0-1,nodeid=0,memdev=mem0
#       NUMA node 0: logical CPUs 0 and 1, backed by mem0.
#   -numa node,cpus=2-3,nodeid=1,memdev=mem1
#       NUMA node 1: logical CPUs 2 and 3, backed by mem1.
#   -smp 4,sockets=2,cores=2,threads=1
#       4 vCPUs arranged as 2 sockets × 2 cores × 1 thread.
#
# Acceleration:
#   -accel tcg,thread=multi  — multi-threaded TCG (best without KVM).
#   No -enable-kvm: GitHub-hosted runners don't expose /dev/kvm.
#
# Output:
#   -nographic              — no display; only serial output.
#   -serial file:<log>      — write serial console to a file for parsing.
#   -no-reboot              — exit QEMU instead of rebooting (prevents hang).
#
# Networking:
#   -net nic,model=virtio -net user  — user-mode NAT; no TAP / bridge needed.
#
QEMU_ARGS=(
    -nographic
    -no-reboot
    -m 2G
    -smp "4,sockets=2,cores=2,threads=1"
    -object "memory-backend-ram,id=mem0,size=1G"
    -object "memory-backend-ram,id=mem1,size=1G"
    -numa "node,cpus=0-1,nodeid=0,memdev=mem0"
    -numa "node,cpus=2-3,nodeid=1,memdev=mem1"
    -accel "tcg,thread=multi"
    -machine "type=q35"
    -cpu "qemu64"
    -net "nic,model=virtio"
    -net "user"
    -cdrom "${ISO_PATH}"
    -boot "order=d"
    -serial "file:${SERIAL_LOG}"
)

log "Booting QEMU 2-NUMA-node guest (TCG, no KVM)…"
log "Topology: 2 sockets × 2 cores = 4 vCPU; 2 GB RAM split 1 GB/node"

qemu-system-x86_64 "${QEMU_ARGS[@]}" &
QEMU_PID=$!
log "QEMU started (PID=${QEMU_PID})"

# ---------------------------------------------------------------------------
# Poll serial log for kernel NUMA discovery messages
# ---------------------------------------------------------------------------
#
# Linux kernel prints lines like the following early in boot (before userland):
#   NUMA: Node 0 [mem 0x00000000-0x3fffffff]
#   NUMA: Node 1 [mem 0x40000000-0x7fffffff]
#
# These appear regardless of whether the guest fully boots to a login prompt.
# Polling the serial log file is race-free (we only append / read, no IPC).

log "Polling serial log for kernel NUMA messages (timeout=${BOOT_TIMEOUT}s)…"

FOUND_NODES=0
ELAPSED=0
POLL_INTERVAL=5

while [[ ${ELAPSED} -lt ${BOOT_TIMEOUT} ]]; do
    # Abort early if QEMU died unexpectedly.
    if ! kill -0 "${QEMU_PID}" 2>/dev/null; then
        log "QEMU process exited (PID=${QEMU_PID})."
        QEMU_PID=""
        break
    fi

    if [[ -f "${SERIAL_LOG}" ]]; then
        FOUND_NODES="$(grep -c 'NUMA: Node [0-9]' "${SERIAL_LOG}" 2>/dev/null || true)"
        if [[ "${FOUND_NODES}" -ge 2 ]]; then
            log "Found ${FOUND_NODES} NUMA node announcement(s)."
            break
        fi
    fi

    sleep "${POLL_INTERVAL}"
    ELAPSED=$((ELAPSED + POLL_INTERVAL))
done

# ---------------------------------------------------------------------------
# Evaluate result and report
# ---------------------------------------------------------------------------

log "--- Serial log tail (last 40 lines) ---"
tail -n 40 "${SERIAL_LOG}" 2>/dev/null || log "(serial log is empty or missing)"
log "--- End of serial log ---"

if [[ "${FOUND_NODES}" -ge 2 ]]; then
    log "PASS: guest kernel reported ${FOUND_NODES} NUMA node(s). 2-node topology correctly emulated."
    exit 0
fi

# Failure path: dump full log for diagnosis.
log "--- Full serial log ---"
cat "${SERIAL_LOG}" 2>/dev/null || log "(serial log is empty or missing)"
log "--- End full serial log ---"

if [[ ${ELAPSED} -ge ${BOOT_TIMEOUT} ]]; then
    die "TIMEOUT after ${BOOT_TIMEOUT}s: observed ${FOUND_NODES}/2 NUMA nodes. See serial log above."
else
    die "QEMU exited early; observed ${FOUND_NODES}/2 NUMA nodes. See serial log above."
fi
