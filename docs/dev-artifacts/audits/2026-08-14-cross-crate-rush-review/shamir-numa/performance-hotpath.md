# shamir-numa — Performance & O(x->0)

## Summary

The crate's hot paths are genuinely O(1) and allocation-free: `load_local` is one
dynamic call + bounds check + `ArcSwap::load`, and `LinuxTopology::current_node` is a
vDSO `sched_getcpu` plus one Fx-hashed `std::collections::HashMap` probe (`TFxMap` is
`HashMap<K, V, BuildHasherDefault<FxHasher>>`, not `scc` — so the workspace's O(N)
`scc::len()` ban has no exposure here). All allocation happens on cold
construction/probe paths. Findings below are limited: a constant-factor overhead that
contradicts the crate's own "zero overhead" single-node claim (now on a live consumer
path in `shamir-index`), one unbounded-allocation edge in the public `parse_cpulist`,
a doc-accuracy nit on the `rcu` mirror window, and the absence of any in-crate
micro-bench for the read path despite two live consumers. No hidden O(N)/O(N²) loops,
per-op allocations, or unbounded buffering on live paths were found.

## Findings

### 1. `load_local` does not meet its documented "identical to a bare `ArcSwap` — zero overhead" contract

**File:line:** `crates/shamir-numa/src/node_replicated.rs:71-73` (claim at
`crates/shamir-numa/src/lib.rs:28-29` and `README.md:24-26`; hot-path blessing at
`src/topology.rs:18-20`)

**Severity:** low

**Issue:** `load_local` unconditionally resolves the calling thread's node via
`self.topology.current_node()` through `Arc<dyn Topology>`. That is a non-inlinable
virtual call even for `FallbackSingleNodeTopology` (whose `current_node` returns the
constant `NodeId(0)`), plus the `replica()` clamp branch — i.e. ~2 extra operations and
one indirect call per read compared to a bare `ArcSwap::load`, on the single-node
configuration the docs sell as exactly equivalent. This is no longer theoretical:
`shamir-index` (Фаза 2, partially landed) calls `load_local()` per operation — e.g.
`crates/shamir-index/src/base_index/index_info.rs:288/293` invoke it just to ask
`.len()` / `.is_empty()`, and `sorted_index_manager.rs:530/549` likewise — so the
overhead is paid at query rate.

**Failure scenario:** none (pure constant factor); the cost is the documented
zero-overhead claim being false, multiplied by every trivial registry read.

**Suggested fix:** short-circuit the degenerate case before touching the topology —
`if self.replicas.len() == 1 { return self.replicas[0].load(); }` (a boxed-slice
`len()` is a field load, far cheaper than the vcall) — or replace the trait object
with a small `enum TopologyKind { Fallback, Linux, Mock }` dispatch so `current_node`
can inline and constant-fold.

### 2. `parse_cpulist` expands ranges without a span cap — one token can attempt an unbounded allocation

**File:line:** `crates/shamir-numa/src/cpulist.rs:36-43` (`cpus.extend(lo..=hi)`)

**Severity:** low

**Issue:** range endpoints parse as unconstrained `usize` and are fed straight into
`extend(lo..=hi)`. A token like `0-9999999999` — or a corrupted sysfs value such as
`0-18446744073709551615` — makes a single `extend` call attempt a multi-GB-to-exabyte
capacity reservation, aborting the process on allocation failure in one step. The
current production caller (`LinuxTopology::probe`, `src/linux.rs:52-93`) feeds it
kernel-controlled `/sys` text at startup, but the function is `pub` and its own doc
(`cpulist.rs:11-12`) advertises standalone use for "tooling that inspects `/proc` /
`/sys` cpu masks", i.e. non-kernel-controlled input is an intended use.

**Failure scenario:** parsing a malformed/hostile cpulist string with one huge range
→ immediate OOM abort of the embedding process (DB or tooling), not a graceful skip
the way reversed ranges and garbage tokens already are (`cpulist.rs:20-22`).

**Suggested fix:** cap the span of a single range — skip any `hi - lo` wider than a
sane `MAX_CPUS` bound (e.g. `1 << 20`), mirroring the existing "reversed ranges are
skipped" policy — and/or enforce a hard cap on total output length.

### 3. `rcu` mirror loop: old/new visibility window documented as "a few nanoseconds" is actually O(nodes) remote stores

**File:line:** `crates/shamir-numa/src/node_replicated.rs:96-108` (claim at lines
25-27: "a few nanoseconds")

**Severity:** nit

**Issue:** the mirror phase issues one `ArcSwap::store` per remaining replica; each
store to a remote node's cache-padded cell is a cross-socket RFO (~100-300 ns each on
the hardware the crate docs cite), so on an 8-16-socket host both the per-write cost
and the eventual-consistency window are µs-scale O(N), not nanoseconds. Writes are
rare by design (read-mostly registries — consumers call `rcu` on DDL-scale events
only), so no code change is warranted on the perf lens; the doc constant is what
drifts.

**Suggested fix:** correct the doc to "a remote store per node, O(num_nodes) on the
interconnect". Side note for the concurrency-theme reviewer (out of scope here): the
un-versioned mirror loop interleaves freely between concurrent `rcu` callers, so the
shape of the stale-value window it can open deserves a look under that lens.

### 4. No micro-bench for the `load_local` read path despite two live consumers

**File:line:** `crates/shamir-numa/` (no `benches/` directory; `README.md:74-75`
defers all perf numbers to multi-socket hardware)

**Severity:** nit

**Issue:** `shamir-index` already migrated `IndexInfo` and `SortedIndexManager` onto
`NodeReplicated`, so the `load_local` path is live at query rate today, yet the crate
ships no `bench_scale_tool::Harness` bench (workspace convention per `CLAUDE.md`) even
for the single-socket case. The README's "perf numbers require real multi-socket
hardware" rationale does not cover finding #1, which is measurable on any box.

**Suggested fix:** add a small `benches/node_replicated.rs` comparing bare
`ArcSwap::load` vs `NodeReplicated::load_local` (1 replica) and across mock
multi-node topologies, so finding #1's fix is gated by numbers.
