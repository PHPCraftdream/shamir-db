# shamir-numa

NUMA-topology abstraction and per-node replicated read-mostly state for
S.H.A.M.I.R.

On a multi-socket (NUMA) server, a memory access costs ~2× more and shares a
contended interconnect when it crosses a socket boundary (Hennessy & Patterson
§5.2–5.3; Drepper, *What Every Programmer Should Know About Memory*, §5). This
crate lets the database keep hot read-mostly state **node-local**: a reader on
node *N* loads its own replica without reaching across a socket.

## What's here

| Item | Role |
|---|---|
| `Topology` (trait) | Discovery + best-effort thread pinning, behind one DI seam |
| `MockTopology` | Test double — simulate any node/CPU layout, no real hardware |
| `FallbackSingleNodeTopology` | One UMA node; used on non-Linux or failed Linux probes |
| `NodeReplicated<T>` | One cache-padded `ArcSwap<T>` per node; RCU copy-on-write |
| `CachePadded<T>` | 128-byte alignment to defeat false sharing (Herlihy & Shavit §8) |
| `parse_cpulist` | Pure parser for the Linux `/sys` cpulist format |
| `detect()` | Fresh Linux discovery or single-node fallback |
| `detect_cached()` | Process-wide shared topology, including a cached fallback |

`NodeReplicated` **degrades to a single replica** when the topology reports one
node; writes have no mirror loop. Local reads still query the topology and
resolve a replica in constant time before loading its `ArcSwap`.

`detect_cached()` retains its first result for the process lifetime. Use fresh
`detect()`, inject a topology, or restart to observe topology changes; existing
replicas do not update automatically. Linux probe failures produce at most one
process-wide classified diagnostic on stderr.

```rust
use std::sync::Arc;
use shamir_numa::{detect, NodeReplicated};

let topo = detect();                       // fresh discovery or fallback
let defs = NodeReplicated::new(topo, vec![/* index definitions */]);

let snapshot = defs.load_local();          // node-local read, O(1)
defs.rcu(|cur| { let mut v = cur.clone(); /* mutate */ v });  // COW, all nodes
```

## Testing strategy (three tiers)

1. **Tier 1 — DI-mock unit tests** run across the CI OS matrix. They cover
   portable topology behavior, including explicit replica-index fallback to
   node 0 for out-of-range access.
2. **Tier 2 — Linux integration** exercises `LinuxTopology` against real `/sys`.
   Injected unit tests also cover Linux probe parser/error behavior.
3. **Tier 3 — QEMU NUMA smoke** checks guest topology announcements, not Rust
   integration. Opt-in via `[numa-qemu]` in the commit message; implemented in
   `scripts/runners/ci-qemu-numa-test.sh`.

CI uses `./scripts/test.sh -p shamir-numa --full` for the full crate suite.
Local test execution requires explicit user permission.

### Tier-3 QEMU harness — current scope

The harness is a **smoke harness**: it boots a 2-NUMA-node QEMU guest (4 vCPU,
2 GB RAM split 1 GB/node) and verifies that the Linux kernel reports two NUMA
nodes in its serial-console output. This confirms QEMU topology flags are
wired correctly without requiring full cargo test execution inside the guest.

**Why not full `cargo test` inside the guest?** GitHub-hosted `ubuntu-latest`
runners lack KVM; QEMU runs in TCG (software emulation, ~50–100× slower than
native). A Rust build + nextest run inside TCG QEMU reliably exceeds the
30-minute CI cap.

**Path to full integration** (choose one):

- **Option A — Self-hosted KVM runner** (recommended): change `runs-on` in
  `numa.yml` tier3-qemu to `[self-hosted, linux, kvm]`. With KVM the guest
  boots in ~5 s and a `--lib` test run takes ~2 min.
- **Option B — Cross-compile + virtfs 9p**: cross-compile shamir-numa tests
  for `x86_64-unknown-linux-musl` on the host, then run the pre-built binary
  inside a minimal Alpine guest via a 9p-mounted workspace.
- **Option C — Cloud-init + SSH**: boot a full Ubuntu cloud image, inject
  and run cargo via SSH (cacheable ~500 MB image).

See the script header in `scripts/runners/ci-qemu-numa-test.sh` for detailed
implementation notes on each option.

Performance numbers require **real multi-socket hardware** and are measured
out-of-CI — QEMU models topology, not latency asymmetry.

## Roadmap

- Full guest Rust integration remains future work; Linux discovery and QEMU
  topology smoke coverage are already shipped.
- Migrate remaining hot registries to `NodeReplicated` where appropriate.
- Pin critical threads (WAL writer, drainer) via config.

See `docs/dev-artifacts/research/NUMA-DESIGN-2026-06-29.md` for the full design and sources.
