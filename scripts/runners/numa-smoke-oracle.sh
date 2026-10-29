#!/usr/bin/env bash

numa_has_configured_nodes() {
    awk '
        /NUMA: Node[[:space:]]+0[[:space:]]+\[mem[[:space:]]+0x[[:xdigit:]]+-0x[[:xdigit:]]+\]/ { node0 = 1 }
        /NUMA: Node[[:space:]]+1[[:space:]]+\[mem[[:space:]]+0x[[:xdigit:]]+-0x[[:xdigit:]]+\]/ { node1 = 1 }
        END { exit !(node0 && node1) }
    '
}
