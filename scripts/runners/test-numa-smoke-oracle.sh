#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/numa-smoke-oracle.sh"

expect_rejected() {
    local name="$1" input="$2"
    if numa_has_configured_nodes <<< "${input}"; then
        printf 'FAIL: %s was accepted\n' "${name}" >&2
        exit 1
    fi
}

expect_accepted() {
    local name="$1" input="$2"
    if ! numa_has_configured_nodes <<< "${input}"; then
        printf 'FAIL: %s was rejected\n' "${name}" >&2
        exit 1
    fi
}

expect_rejected empty ''
expect_rejected duplicate_only $'NUMA: Node 0 [mem 0x0-0x1]\nNUMA: Node 0 [mem 0x2-0x3]'
expect_rejected wrong_ids $'NUMA: Node 2 [mem 0x0-0x1]\nNUMA: Node 3 [mem 0x2-0x3]'
expect_rejected numeric_prefixes $'NUMA: Node 10 [mem 0x0-0x1]\nNUMA: Node 11 [mem 0x2-0x3]'
expect_rejected one_configured_id $'NUMA: Node 0 [mem 0x0-0x1]\nNUMA: Node 11 [mem 0x2-0x3]'
expect_rejected malformed $'NUMA: Node 0x [mem 0x0-0x1]\nNUMA: Node 1 [memory 0x2-0x3]'
expect_accepted reordered_duplicates $'[    0.0] NUMA: Node 1 [mem 0x2-0x3]\n[    0.1] NUMA: Node 0 [mem 0x0-0x1]\n[    0.2] NUMA: Node 1 [mem 0x2-0x3]'
printf 'NUMA smoke oracle fixtures passed\n'
