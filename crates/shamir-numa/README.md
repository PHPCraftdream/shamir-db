# shamir-numa

NUMA-topology abstraction and per-node replicated read-mostly state for
S.H.A.M.I.R.

On a multi-socket (NUMA) server, a memory access costs ~2× more and shares a
contended interconnect when it crosses a socket boundary (Hennessy & Patterson
§5.2–5.3; Drepper, *What Every Programmer Should Know About Memory*, §5). This
crate lets the database keep hot read-mostly state **node-local**: a reader on
node *N* loads its own replica without reaching across a socket.

## What's here (Фаза 1)

| Item | Role |
|---|---|
| `Topology` (trait) | Discovery + best-effort thread pinning, behind one DI seam |
| `MockTopology` | Test double — simulate any node/CPU layout, no real hardware |
| `FallbackSingleNodeTopology` | One UMA node; used on Windows / macOS / single-socket |
| `NodeReplicated<T>` | One cache-padded `ArcSwap<T>` per node; RCU copy-on-write |
| `CachePadded<T>` | 128-byte alignment to defeat false sharing (Herlihy & Shavit §8) |
| `parse_cpulist` | Pure parser for the Linux `/sys` cpulist format |
| `detect()` | Best-effort topology factory (single-node fallback for now) |

`NodeReplicated` **degrades to a single replica** when the topology reports one
node, behaving exactly like a bare `ArcSwap` — so consumers use it
unconditionally with zero overhead on single-socket / dev / CI machines.

```rust
use std::sync::Arc;
use shamir_numa::{detect, NodeReplicated};

let topo = detect();                       // single-node fallback here
let defs = NodeReplicated::new(topo, vec![/* index definitions */]);

let snapshot = defs.load_local();          // node-local read, O(1)
defs.rcu(|cur| { let mut v = cur.clone(); /* mutate */ v });  // COW, all nodes
```

## Testing strategy (three tiers)

1. **Tier 1 — DI-mock unit tests** (this version). Platform-independent; run on
   every OS via `cargo test -p shamir-numa --lib` and the workspace CI matrix.
2. **Tier 2 — Linux integration** against real `/sys` (Фаза 1b, `LinuxTopology`).
3. **Tier 3 — QEMU NUMA emulation** in CI for multi-socket correctness without
   physical hardware (skeleton in `.github/workflows/numa.yml`, opt-in via a
   `[numa-qemu]` commit-message flag).

Performance numbers require **real multi-socket hardware** and are measured
out-of-CI — QEMU models topology, not latency asymmetry.

## Roadmap

- **Фаза 1b** — `LinuxTopology` (`/sys` probe + `sched_setaffinity`), QEMU CI.
- **Фаза 2** — migrate the hot `ArcSwap` registries (`IndexInfo` #292,
  `SortedIndexManager` #304, validator bindings #289) to `NodeReplicated`.
- **Фаза 3** — pin critical threads (WAL writer, drainer) via config.

See `docs/research/NUMA-DESIGN-2026-06-29.md` for the full design and sources.
