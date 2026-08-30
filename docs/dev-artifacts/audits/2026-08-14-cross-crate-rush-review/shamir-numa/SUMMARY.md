# shamir-numa — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-numa/` — NUMA topology discovery (`detect()`, `LinuxTopology` /
`FallbackSingleNodeTopology` / `MockTopology`, sysfs `cpulist` parsing) and the
`NodeReplicated<T>` read-mostly registry (one cache-line-padded `ArcSwap<T>` cell per
NUMA node, CAS-based `rcu`). Consumed live by `shamir-index` for its index-definition
registries.

Review basis: the seven 2026-08-14 cross-crate lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and deduplicated into distinct root-cause defects,
following the synthesis convention of the calibrated exemplars
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. During
synthesis, key `file:line` citations were spot-checked against the crate source
(`node_replicated.rs`, `linux.rs`, `cpulist.rs`, `detect.rs`, `lib.rs`,
`tests/node_replicated_tests.rs`) and the live-consumer claim against
`crates/shamir-index/src/`. Read-only synthesis — no build, no tests, no lint, no
source modifications.

## Executive summary

The crate is small, ideology-clean, and almost pillar-perfect (fully lock-free
`NodeReplicated`, one sanctioned test-fixture `Mutex`, `TFxMap` reverse index, no
async, thorough `src/tests/` coverage of the portable core), but it ships one
workspace-headline defect: **`NodeReplicated`'s mirror phase is unversioned, so two
concurrent `rcu`/`store` writers can leave every non-zero-node replica stranded on a
superseded value indefinitely** — permanent replica divergence, on live
`shamir-index` DDL paths, invisible to CI because the only concurrency test asserts
node 0 (the one cell the CAS already protects). Fix that first (epoch/versioned
mirror or a converging mirror pass) together with the every-replica Red test; then
close the two Linux process-abort/mis-pin classes (unbounded `parse_cpulist` range
expansion; `CPU_SET` without a `CPU_SETSIZE` bound on ≥1024-CPU hosts) and
`probe()`'s blanket per-node error swallow that can silently feed a broken topology
into production registries. Everything else is medium/low doc-accuracy and API-hygiene
work.

---

## 1. correctness-tdd

The portable core (`parse_cpulist`, fallback/mock topologies, `CachePadded`) has real
behavioural coverage in the prescribed `src/tests/` layout. The flagship primitive does
not: the mirror half of `rcu` — the crate's own addition over bare `ArcSwap` — has zero
multi-threaded coverage, so its headline race cannot fail any test today.

### 1.1 — high — *(same defect as 2.1)* — `rcu` mirror loop permanently strands non-node-0 replicas on a stale value
- Full write-up at **2.1** (primary: concurrency-lockfree; also filed by
  api-wire-protocol as 5.1 — three lenses, one defect). Correctness framing: the race
  directly contradicts the consistency-model doc (`node_replicated.rs:22-30`), which
  promises a "few nanoseconds" window and "eventual consistency"; under concurrent
  writers the window is unbounded and convergence is not guaranteed.

### 1.2 — medium — Concurrency test asserts node 0 only — the mirror half of `rcu` has zero multi-threaded coverage
- File:line: `crates/shamir-numa/src/tests/node_replicated_tests.rs:95-117`
  (`concurrent_rcu_does_not_lose_updates_on_node_zero`, node-0-only assertion at
  `:116`).
- Issue: the test verifies exactly the guarantee arc-swap already provides (node-0 CAS)
  and nothing about the crate's own addition (the mirror to nodes 1..n). Its name
  promises "does not lose updates", but replication freshness under contention — the
  entire point of `NodeReplicated` — is unasserted, so finding 2.1 cannot fail any test
  today. There is also no `store`-vs-`rcu` interleaving test and no convergence check on
  a >2-replica mock. A Red-test hole against CLAUDE.md's Red/Green/Refactor discipline.
- Failure scenario: the mirror race (2.1) ships — and keeps shipping — with a green
  suite: node 1's replica sits at `7997` instead of `8000` after the 8×1000 increment
  storm and every test stays green.
- Suggested fix: after the joins, assert `load_node(NodeId(n)) == threads * per` for
  **every** n. That assertion is intermittently red *today* — land it together with the
  2.1 fix; add a `store`/`rcu` interleave test and a 4-node-mock convergence storm.
- Also flagged by: concurrency-lockfree (2.2).

### 1.3 — medium — *(same defect as 5.2)* — out-of-range `store_node`/`load_node` silently redirect to node 0
- Full write-up at **5.2** (primary: api-wire-protocol). Correctness framing: a write
  with a stale `NodeId` (topology swapped/shrunk, or a stale `num_nodes()` snapshot)
  silently overwrites a *different* node's staged replica — a wrong-target silent write
  for an operation that can fail semantically.

### 1.4 — medium — *(same defect as 6.3)* — fixed 1024-CPU `cpu_set_t`; CPUs ≥ 1024 pin wrongly, fail, or panic
- Full write-up at **6.3** (primary: error-handling-lifecycle; also filed by
  security-crypto as 3.1 — three lenses, one defect). Correctness framing: a node
  *straddling* CPU 1024 yields a partial mask and `sched_setaffinity` returns `Ok(())`
  while the thread can still migrate onto the node's high CPUs — a silent no-op pin
  violating the documented post-condition.

### 1.5 — low — *(same defect as 6.4)* — `LinuxTopology::probe` decision logic is untestable as written
- Full write-up at **6.4** (primary: error-handling-lifecycle). Correctness framing:
  hard-coded `/sys` paths mean the non-trivial dense-slot mapping of non-contiguous
  online node ids (`0,2`), missing-cpulist best-effort, and the CPU→node reverse map are
  never exercised by a deterministic test; a slot-mapping regression passes the suite
  unless the CI host happens to have a node-numbering hole.

### 1.6 — low — *(same defect as 7.1)* — inline `#[cfg(test)] mod tests` inside `linux.rs`
- Full write-up at **7.1** (primary: style-claude-md; also filed by api-wire-protocol
  as 5.6 — three lenses, one defect).

### 1.7 — low — Dead doctest in `cpulist.rs` — assertions never compiled or run
- File:line: `crates/shamir-numa/src/cpulist.rs:24-28`; policy:
  `crates/shamir-numa/Cargo.toml:9-13` (`doctest = false`, doctests banned project-wide).
- Issue: the doc example's two `assert_eq!`s are never executed — never even compiled —
  giving the illusion of verified coverage; the cases duplicate `mixed_ranges_and_indices`
  and `empty_is_empty` in `cpulist_tests.rs`, which do run.
- Failure scenario: if the parser drifts from the example, nothing fails; the rendered
  docs assert wrong behaviour indefinitely.
- Suggested fix: render the example as non-compiled illustrative text (the convention the
  Cargo.toml comment prescribes) or drop its assertions in favour of a pointer to
  `cpulist_tests.rs`.
- Also flagged by: style-claude-md (7.3).

### 1.8 — low — *(same defect as 6.1)* — `parse_cpulist` allocates unboundedly on a large range token
- Full write-up at **6.1** (primary: error-handling-lifecycle; also filed by
  security-crypto, performance-hotpath, api-wire-protocol — five lenses, one defect).

### 1.9 — nit — Redundant guards in `detect()` / `probe()`
- File:line: `crates/shamir-numa/src/detect.rs:26`; `crates/shamir-numa/src/linux.rs:69-70`.
- Issue: `topo.num_nodes() > 0` in `detect()` is dead — `probe()` already returns
  `Err(Unsupported)` when the node list is empty (`linux.rs:62-64`, verified);
  `node_ids_sorted.sort_unstable()` is a no-op — `parse_cpulist` already returns sorted,
  deduplicated output. Harmless, but they imply reachable states that the callees
  already exclude.
- Failure scenario: none.
- Suggested fix: drop both, or replace with a one-line comment citing the enforcing
  invariant upstream.

## 2. concurrency-lockfree

Otherwise a faithful implementation of the five pillars: per-node cache-padded
`ArcSwap` cells + CAS-based `rcu` are the sanctioned lock-free shape (arc-swap's `rcu`
is a load+CAS retry loop — the closure is *not* run under an internal lock, verified
against arc-swap 1.9 vs the pinned `"1.7"`); the only `Mutex` in the crate is
`MockTopology::pin_log`, a test fixture with the required inline contention-model
comment; `cpu_to_node` is `TFxMap`; no async anywhere (so no guard-across-`.await`);
no `scc::*` (no O(N) `len()` exposure); `current_node` is `sched_getcpu` + one Fx-hashed
probe, O(1) and allocation-free. One substantive defect:

### 2.1 — high — `rcu`/`store` mirror phase can overwrite newer replicas with a stale value — non-zero nodes diverge from node 0 indefinitely *(workspace top-3 headline; flagged independently by three lenses)*
- File:line: `crates/shamir-numa/src/node_replicated.rs:96-108` (mirror loop at
  `:99-107`, node-0 re-read `load_full()` at `:103`; in-code invariant comment
  `:100-102`); `store` at `:82-87` has the same unsynchronized multi-replica publish
  shape; contract doc at `:20-30` ("a few nanoseconds … **eventual consistency**", the
  "few nanoseconds" claim at `:26`); `store`'s doc (`:81`) does not discuss concurrency
  at all.
- Issue: `rcu` linearises only on the node-0 cell. The mirror phase re-reads node 0 once
  *after its own CAS* and then blindly `store`s that snapshot to every other replica —
  with no ordering or versioning tying a thread's mirror pass to the node-0 value that
  is latest *at mirror time*. The in-code comment claims reading it back "keeps the
  mirror consistent with the value that actually won the CAS" — true only for a single
  concurrent writer. Two writers interleave so the *loser's* mirrors land last; the same
  applies to `rcu` racing a plain `store` (which never consults node 0; its per-replica
  stores interleave arbitrarily, including a mix of two publications across replicas).
  The perf lens adds: even single-writer, the "few nanoseconds" window is actually
  O(num_nodes) remote cross-socket RFO stores (~100–300 ns each on the hardware the docs
  cite) — µs-scale on 8–16 sockets; the doc constant is wrong on both axes.
- Failure scenario (verified against source): on a multi-node host — (1) T1's `rcu` CAS
  wins on node 0 → node 0 = X1; T1's `load_full()` reads X1. (2) T2's `rcu` CAS wins →
  node 0 = X2; T2 mirrors X2 to nodes 1..N. (3) T1 now mirrors its earlier X1 to nodes
  1..N. Final state: node 0 = X2, nodes 1..N = **X1**. Readers only `load()` their own
  node's cell, so nothing re-synchronises: divergence persists until the next successful
  `rcu`/`store` — potentially forever on an idle registry. Live consumers make
  concurrent writers plausible: `shamir-index` calls `.rcu(...)` from many distinct DDL
  paths (`crates/shamir-index/src/base_index/sorted_index_manager.rs:508, 633, 844, 954,
  1145, 1628`; `index_info.rs:252, 270` — re-verified during synthesis) plus a plain
  `store` (`sorted_index_manager.rs:2784`), so a stale index-definition list on a
  non-zero node is a silent correctness hazard (a query on node 1 consults a
  dropped/renamed index definition) with no error and no log. Mitigations: single-node
  topologies (Windows/CI/dev) never execute the mirror loop, and node 0 is always
  correct. CI-blindness: see 1.2 — no test asserts the mirror half.
- Suggested fix: give the publish a linearisable version — an `AtomicU64` epoch advanced
  by the node-0 CAS winner, with the mirror installing `(epoch, Arc<T>)` only if the
  target's epoch is older (CAS retry); or the lighter converging-mirror variant — after
  each mirror pass, re-read node 0 and redo the pass while it changed, exiting only when
  a full pass completes with node 0 stable (the last writer to finish mirroring then
  always converges all replicas); or single-flight the whole rcu+mirror through one
  lock — the write path is cold, so a sanctioned low-frequency `std::sync::Mutex` with
  an inline contention-model comment fits CLAUDE.md. At minimum, correct the doc-comment
  and in-code invariant claims to describe the actual (weaker) guarantee, and document
  `store`'s guarantee explicitly.
- Also flagged by: correctness-tdd (1.1), api-wire-protocol (5.1, as a wrong public
  consistency contract), performance-hotpath (4.3 nit — the O(nodes) window-cost doc
  constant; folded into this defect since the same sentence and the same doc fix are
  involved).

### 2.2 — medium — *(same defect as 1.2)* — concurrency tests never assert mirror convergence
- Full write-up at **1.2** (primary: correctness-tdd). Concurrency framing: node 0 is
  exactly the cell the CAS loop protects, so the suite is structurally blind to finding
  2.1's failure mode; no `store`-vs-`rcu` interleave test, no >2-replica storm.

### 2.3 — low — `detect()` re-probes `/sys` on every call — no memoized variant, and consumers already call it per-instance construction
- File:line: `crates/shamir-numa/src/detect.rs:24-43` (Linux arm at `:24-31` performs
  1 + `num_nodes` blocking `std::fs::read_to_string` calls per invocation); consumer
  sites `crates/shamir-index/src/base_index/index_info.rs:142, 152, 167, 204, 340`
  (including the `Deserialize` impl — i.e. per hydrated record) and
  `sorted_index_manager.rs:299-302` (re-verified during synthesis).
- Issue: `detect()` is framed as a bootstrap helper, but the API offers no cached/shared
  form and no doc note that the result must be reused. On Linux each call repeats
  O(nodes) blocking file reads — synchronous I/O on (potentially) an async-runtime
  thread, and repeated non-constant per-instance work, contrary to pillars 2 and 3; the
  returned topology is a fresh object each time, so replicas are never shared across
  those instances.
- Failure scenario: a table-hydration path deserialises `IndexInfo`s in a loop; on a
  4-node host each record costs ~5 sysfs reads on the calling (async) thread — hidden
  per-op I/O no benchmark attributes to `detect()`.
- Suggested fix: add a `OnceLock<Arc<dyn Topology>>`-backed `detect_shared()` (or cache
  inside `detect()`) plus a doc note that probing is bootstrap-grade blocking I/O and the
  returned `Arc` must be reused; migrate the `shamir-index` call sites.

## 3. security-crypto

No authentication, crypto, or network surface at all — no HMAC/SCRAM/TLS, no secrets,
no command execution, no env reads. The entire security boundary is (a) the two `unsafe`
libc calls in the Linux topology impl and (b) parsing of kernel-generated `/sys` cpulist
text. The `unsafe` blocks are otherwise well-annotated (zero-init via `CPU_ZERO`,
stack-pointer validity for the syscall duration, `pid = 0`, accurate ABI argument) and
path construction interpolates only parsed `usize` values — no traversal, no injection,
no timing side-channel surface. Findings:

### 3.1 — medium — *(same defect as 6.3)* — `CPU_SET` fed sysfs CPU indices with no `CPU_SETSIZE` bound
- Full write-up at **6.3** (primary: error-handling-lifecycle; also filed by
  correctness-tdd as 1.4). Security framing: on ≥1024-CPU hosts the pin path panics
  inside the `unsafe` block ("index out of bounds: the len is 16 but the index is 56")
  — a crash reachable purely from kernel-reported data, violating the crate's
  `Result`-based error contract; the `// SAFETY:` comment's soundness argument is
  incomplete (omits the `CPU_SETSIZE` range precondition).

### 3.2 — low — *(same defect as 6.1)* — `parse_cpulist` expands ranges with no cap
- Full write-up at **6.1** (primary: error-handling-lifecycle; also filed by
  performance-hotpath, api-wire-protocol, correctness-tdd). Security framing: the
  infallible public API aborts the process on large well-formed input; all current
  callers feed trusted kernel-generated sysfs, so there is no exploit path *today* —
  the risk is any future untrusted source (config value, client-supplied string). The
  same unbounded expansion applies to node ids parsed from `online`, which later size
  `NodeReplicated::new`'s per-node replica array, amplifying the allocation.

### 3.3 — nit — *(same defect as 6.2)* — `probe` swallows per-node cpulist read errors into an empty node
- Full write-up at **6.2** (primary: error-handling-lifecycle). Security framing: the
  fail-soft mapping destroys the diagnostic trail on the crate's only external input
  surface — a pin failure surfaces as a bare `EINVAL` with no hint that discovery
  silently degraded.

## 4. performance-hotpath

Hot paths are genuinely O(1) and allocation-free: `load_local` is one dynamic call +
bounds check + `ArcSwap::load`; `current_node` is a vDSO `sched_getcpu` + one Fx-hashed
probe (`TFxMap` is `HashMap<K, V, BuildHasherDefault<FxHasher>>`, not `scc` — the
O(N) `scc::len()` ban has no exposure). All allocation happens on cold
construction/probe paths. No hidden O(N)/O(N²) loops, per-op allocations, or unbounded
buffering on live paths. Findings:

### 4.1 — low — `load_local` does not meet its documented "identical to a bare `ArcSwap` — zero overhead" contract
- File:line: `crates/shamir-numa/src/node_replicated.rs:71-73`; claims at
  `crates/shamir-numa/src/lib.rs:28-29` ("zero overhead", verified) and
  `README.md:24-26`; hot-path blessing at `src/topology.rs:18-20`.
- Issue: `load_local` unconditionally resolves the calling thread's node via
  `self.topology.current_node()` through `Arc<dyn Topology>` — a non-inlinable virtual
  call even for `FallbackSingleNodeTopology` (whose `current_node` returns the constant
  `NodeId(0)`), plus the `replica()` clamp branch — ~2 extra operations and one indirect
  call per read compared to a bare `ArcSwap::load`, on exactly the single-node
  configuration the docs sell as equivalent. No longer theoretical: `shamir-index`
  calls `load_local()` per operation (`index_info.rs:288/293` just to ask `.len()` /
  `.is_empty()`; `sorted_index_manager.rs:530/549` likewise), so the overhead is paid
  at query rate.
- Failure scenario: none (pure constant factor); the cost is the documented
  zero-overhead claim being false, multiplied by every trivial registry read.
- Suggested fix: short-circuit the degenerate case before touching the topology —
  `if self.replicas.len() == 1 { return self.replicas[0].load(); }` (a boxed-slice
  `len()` is a field load) — or replace the trait object with a small
  `enum TopologyKind { Fallback, Linux, Mock }` dispatch so `current_node` can inline
  and constant-fold.

### 4.2 — low — *(same defect as 6.1)* — `parse_cpulist` expands ranges without a span cap
- Full write-up at **6.1** (primary: error-handling-lifecycle). Perf framing: a single
  token (`0-9999999999`, or a corrupted sysfs `0-18446744073709551615`) makes one
  `extend` call attempt a multi-GB-to-exabyte capacity reservation — an immediate OOM
  abort rather than the graceful skip that reversed ranges and garbage tokens already
  get (`cpulist.rs:20-22`).

### 4.3 — nit — *(folded into 2.1)* — `rcu` mirror window documented as "a few nanoseconds" is actually O(nodes) remote stores
- Same doc sentence as 2.1 (`node_replicated.rs:25-27`); the mirror phase issues one
  `ArcSwap::store` per remaining replica, each a cross-socket RFO (~100–300 ns), so the
  per-write cost and the visibility window are µs-scale O(N) on 8–16-socket hosts.
  Writes are rare by design, so no code change beyond 2.1's — the doc constant is what
  drifts. Counted once, inside 2.1.

### 4.4 — nit — No micro-bench for the `load_local` read path despite two live consumers
- File:line: `crates/shamir-numa/` (no `benches/` directory; `README.md:74-75` defers
  all perf numbers to multi-socket hardware).
- Issue: `shamir-index` already runs `load_local` at query rate, yet the crate ships no
  `bench_scale_tool::Harness` bench (workspace convention per CLAUDE.md) even for the
  single-socket case — and the README rationale doesn't cover 4.1, which is measurable
  on any box.
- Failure scenario: 4.1's fix lands ungated by numbers; a future regression in the read
  path is invisible.
- Suggested fix: add a small `benches/node_replicated.rs` comparing bare
  `ArcSwap::load` vs `NodeReplicated::load_local` (1 replica) and across mock multi-node
  topologies.

## 5. api-wire-protocol

The public surface (`Topology` trait, `NodeReplicated<T>`, `detect()`, two topologies,
`parse_cpulist`, `CachePadded`) is small, thoroughly rustdoc'd, and trivially compliant
with the builder-only query rule — zero `serde`/`serde_json` usage anywhere under the
crate (grep-verified by the lens), no wire format exists, so serialization/versioning
rules are N/A. Compliance note: `AffinityError` is not `#[non_exhaustive]` (matchable —
good for the mock tests); adding variants later is a breaking change, acceptable at
`0.1.0-alpha.1` / `publish = false`, revisit if ever published. Dominant theme:
public-contract accuracy.

### 5.1 — high — *(same defect as 2.1)* — documented consistency contract is wrong under concurrent writers
- Full write-up at **2.1** (primary: concurrency-lockfree; also filed by correctness-tdd
  as 1.1). API-contract framing: the wire-facing public doc promises an "eventual
  consistency" that implies convergence and delivers neither — a replica can diverge
  *permanently*; `store`'s doc doesn't discuss concurrency whatsoever, so consumers
  cannot even know which (weaker) guarantee they get from that path.

### 5.2 — medium — `NodeReplicated` silently clamps out-of-range `NodeId` to node 0, masking caller bugs
- File:line: `crates/shamir-numa/src/node_replicated.rs:77-79` (`load_node`),
  `:116-118` (`store_node`), `:120-128` (`replica` clamp — verified).
- Issue: `load_node(NodeId(9))` on a 2-node topology silently returns node 0's replica;
  `store_node` likewise *writes* node 0 — including clobbering node 0 with data intended
  for another node. The crate applies three different conventions to the same
  out-of-range input: `cores_on_node` → empty slice (`topology.rs:25-27`),
  `pin_current_thread_to_node` → `Err(AffinityError::NodeOutOfRange)`
  (`topology.rs:35-42`), `NodeReplicated` → silent clamp. Only the first two are
  documented at the trait level.
- Failure scenario: a caller passes a raw kernel node id or a config-supplied node
  number `>= num_nodes` (plausible on sparse-node hosts — see 5.4); every store lands on
  node 0's replica, silently corrupting the registry for node-0 readers while other
  nodes never see the update — no error, no log signal.
- Suggested fix: return `Result<_, AffinityError::NodeOutOfRange>` from
  `load_node`/`store_node` (they are explicit, non-hot-path inspection/staging APIs), or
  at minimum `debug_assert!` plus a logged warning on the release-path clamp. Keep the
  clamp only for the `load_local` ← `current_node()` handoff, documented as such.
- Also flagged by: correctness-tdd (1.3).

### 5.3 — medium — Crate-level docs and README describe the shipped API as future work
- File:line: `crates/shamir-numa/src/lib.rs:34-40`; `crates/shamir-numa/README.md:12-22,
  39-46, 77-79`; `crates/shamir-numa/src/tests/mod.rs:1-6` (all verified against
  source: `LinuxTopology` is exported at `lib.rs:59-60` and `detect()` probes `/sys` at
  `detect.rs:23-31`).
- Issue: `lib.rs`'s "Scope of this version (Фаза 1)" says "Platform-independent skeleton
  only" and that "the real `LinuxTopology` (`/sys` probe + `sched_setaffinity`) … land
  in Фаза 1b" — yet `linux.rs` is implemented, exported, and `detect()` uses it. The
  README's item table omits `LinuxTopology`, calls `detect()` "single-node fallback for
  now", and lists as Roadmap both "Фаза 1b — `LinuxTopology`" and "Фаза 2 — migrate the
  hot `ArcSwap` registries to `NodeReplicated`" — but the Фаза 2 migration has already
  happened (`shamir-index` consumes `NodeReplicated::new(detect(), ...)` today).
  `src/tests/mod.rs` likewise claims Tier-2 Linux tests "land in Фаза 1b" while
  `tests/linux_topology.rs` exists.
- Failure scenario: a consumer reading the README assumes `detect()` never returns a
  multi-node topology on Linux (bakes in single-replica assumptions or skips NUMA
  handling); a reviewer of `lib.rs` wrongly concludes affinity is unimplemented and
  duplicates it.
- Suggested fix: one docs-only commit rewriting the lib.rs scope section, README table,
  and Roadmap to match the shipped surface (list `LinuxTopology`, describe `detect()`'s
  real Linux behaviour, mark Фазы 1b/2 as done); refresh the tier claims in
  `src/tests/mod.rs`. Leaves only the QEMU Tier-3 harness as future work.
- Also flagged by: style-claude-md (7.2).

### 5.4 — low — Dense-slot `NodeId` discards the kernel's raw NUMA node id with no recovery API
- File:line: `crates/shamir-numa/src/node.rs:3-9`; `crates/shamir-numa/src/linux.rs:66-69,
  84-90`.
- Issue: `LinuxTopology::probe` correctly maps raw sysfs node ids from
  `/sys/devices/system/node/online` (which can be sparse, e.g. `0,2` after hotplug) onto
  dense 0-based replica slots — then drops the raw ids entirely. `NodeId` is documented
  as a dense slot (good), but there is no `raw_node_id(slot)`/`slot_for(raw)` accessor
  on `Topology`, and the trait doc's "The logical CPUs that belong to `node`" invites
  passing a raw sysfs/libnuma node number — which silently returns the *wrong node's*
  CPUs (compounding 5.2). Фаза 3's planned pinning of named threads, and any tooling
  correlating with `numactl`/`mbind`/`set_mempolicy` (which take raw node ids), has no
  way back.
- Failure scenario: config says "pin WAL writer to NUMA node 2" (operator reads `2` from
  `numactl --hardware`); on a sparse-node host dense slot 2 is kernel node 3 — the
  thread pins to the wrong socket, with no API to detect or correct it.
- Suggested fix: add `fn raw_node_id(&self, node: NodeId) -> usize` (default: identity)
  to `Topology`, or at minimum loudly document in `NodeId`'s doc and the trait docs that
  `NodeId` is a replica slot and never a kernel node number.

### 5.5 — low — Public API returns `arc_swap::Guard`, welding the crate's semver to a foreign dependency type
- File:line: `crates/shamir-numa/src/node_replicated.rs:69-79`.
- Issue: `load_local`/`load_node` return `arc_swap::Guard<Arc<T>>`, making `arc-swap` a
  public API dependency: any arc-swap release that changes `Guard` is a breaking change
  for `shamir-numa` consumers, and every caller must understand `Guard`'s debt-slot
  semantics (including its interaction with concurrent writers).
- Failure scenario: an arc-swap upgrade that alters `Guard`'s type/semantics breaks
  every downstream consumer of the registry read path at the public boundary.
- Suggested fix: return plain `Arc<T>` via `load_full()` at the public boundary (one
  refcount clone; negligible for read-mostly registries), or wrap in a newtype. Keep
  `Guard` internal if a benchmark ever justifies it.

### 5.6 — low — *(same defect as 7.1)* — inline `#[cfg(all(test, ...))] mod tests` in `linux.rs`
- Full write-up at **7.1** (primary: style-claude-md; also filed by correctness-tdd as
  1.6).

### 5.7 — low — Test-coverage claims vs reality: `detect()` untested off-Linux, phantom CI workflow in the README
- File:line: `crates/shamir-numa/src/detect.rs:23-43`; `crates/shamir-numa/src/linux.rs:52-93,
  119-157`; `crates/shamir-numa/README.md:44-46, 63`; `crates/shamir-numa/src/tests/`.
- Issue: the non-Linux `detect()` dispatch — the path every Windows/macOS dev box and CI
  runner actually executes — has zero tests anywhere (`fallback_tests.rs` covers
  `FallbackSingleNodeTopology` directly, never the `detect` dispatch; the Linux-side
  `tests/linux_topology.rs` is `#![cfg(target_os = "linux")]`). `NodeReplicated`
  out-of-range clamping (5.2's behaviour) is untested. README's Tier-3 section points at
  `.github/workflows/numa.yml` ("Opt-in via `[numa-qemu]` flag") — `scripts/ci-qemu-numa-test.sh`
  exists, but no `numa.yml` workflow exists anywhere in the repo, so the documented
  opt-in mechanism doesn't exist. (The Linux probe *error-path* coverage gap is its own
  deduped defect — see 6.4.)
- Failure scenario: the documented QEMU opt-in cannot be followed; a `detect()` dispatch
  regression on non-Linux passes every test; the clamp behaviour is unpinned.
- Suggested fix: per-platform `detect()` smoke test; `from_parts`-style constructor for
  `LinuxTopology` (shared with 6.4's seam); a clamp test for `load_node`/`store_node`;
  either add the `numa.yml` workflow or delete the README references to it.
- Overlap notes: its error-path facet dedupes into 6.4; its clamp-test facet into 5.2.

### 5.8 — low — *(same defect as 6.1)* — `parse_cpulist` allocates unboundedly on adversarial range tokens
- Full write-up at **6.1** (primary: error-handling-lifecycle). API framing: the `pub`
  parser's own doc (`cpulist.rs:11-12`) advertises standalone use for "tooling that
  inspects `/proc` / `/sys` cpu masks" — i.e. non-kernel-controlled input is an
  intended use, so the unbounded expansion is a public-contract gap, not just a
  hardening nit.

### 5.9 — nit — Inconsistent out-of-range constructor conventions between the two topology constructors
- File:line: `crates/shamir-numa/src/mock.rs:46-47` vs `crates/shamir-numa/src/fallback.rs:29-34`.
- Issue: `MockTopology::with_nodes(0, _)` panics ("a topology must have at least one
  node") while `FallbackSingleNodeTopology::with_cpus(0)` silently clamps to 1 — both
  encode the same invariant ("a topology has ≥ 1 node") with opposite conventions;
  CLAUDE.md's stance prefers panics only for genuine programmer-bug invariants.
- Failure scenario: none directly; convention drift invites a third variant.
- Suggested fix: pick one convention (panic-on-invalid reads better for a
  construction-time invariant) and document both constructors identically.

### 5.10 — nit — README's "run tests" instruction bypasses the mandated central test entry point
- File:line: `crates/shamir-numa/README.md:41-42` (`cargo test -p shamir-numa --lib`).
- Issue: CLAUDE.md makes `./scripts/test.sh` the contract; a single-crate `--lib` raw
  run sits inside the documented letter of the exception, but READMEs are where the
  convention gets copied from, and the sanctioned form carries nextest, timeouts, and
  the perimeter guard.
- Suggested fix: point at `./scripts/test.sh -p shamir-numa` (and `--full` for the Linux
  integration test).

## 6. error-handling-lifecycle

The crate largely honours the error ideology: one `thiserror` enum (`AffinityError`)
with `#[from] std::io::Error`, `Result` on every fallible entry point, deliberate
infallible read paths, and defensive clamping (`replica()`, `cores_on_node`,
`current_node`) instead of indexing panics. The genuine gaps are concentrated on the
Linux-only probe error paths — none of which is tested anywhere, and none of which is
even compiled by the primary Windows gate.

### 6.1 — medium — `parse_cpulist` expands unbounded ranges — malformed input can panic/abort the process *(flagged by five lenses)*
- File:line: `crates/shamir-numa/src/cpulist.rs:40` (`cpus.extend(lo..=hi)`; range
  handling `:36-43` — verified); feed points `src/linux.rs:57` (`online`) and `:76`
  (per-node cpulist).
- Issue: `RangeInclusive<usize>` is `TrustedLen`, so `Vec::extend` reserves
  `(hi - lo + 1) * 8` bytes in one shot with no upper bound on `hi`. A token like
  `0-99999999999` attempts a ~800 GB reservation (allocation failure → uncatchable
  abort); `0-18446744073709551615` overflows the capacity computation (`capacity
  overflow` panic). This is the one malformed input in a function whose documented
  policy is otherwise "skip silently" (garbage tokens, reversed ranges) — inconsistent
  with its own best-effort contract. The same unbounded expansion on node ids from
  `online` later sizes `NodeReplicated::new`'s per-node replica array, amplifying the
  allocation.
- Failure scenario: the function is `pub` and its docs explicitly advertise it "for
  tooling that inspects `/proc` / `/sys` cpu masks" — i.e. strings that need not come
  from the kernel. A corrupt or hostile cpulist string fed by such tooling (or a future
  config surface) kills the whole process during a parse that is supposed to be
  best-effort.
- Suggested fix: bound the expansion (e.g. skip any range wider than a sane cap —
  `MAX_CPUS` ≈ `1 << 20`, or 4096–8192 matching sysfs `NR_CPUS` reality) while keeping
  the infallible signature; add an error-path test (`huge_range_is_skipped` — the suite
  covers garbage/reversed/whitespace tokens but no range-size bound).
- Also flagged by: security-crypto (3.2), performance-hotpath (4.2),
  api-wire-protocol (5.8), correctness-tdd (1.8).

### 6.2 — medium — `probe()` swallows every per-node cpulist I/O error, contradicting its doc and yielding a silently broken topology
- File:line: `crates/shamir-numa/src/linux.rs:75-79` (`Err(_) => Vec::new()` at `:78` —
  verified); doc at `:50-51`; contrast `fs_read_trim` at `:167-173`; fallback gate at
  `detect.rs:24-31`.
- Issue: the doc says only "A missing per-node `cpulist` is treated as an empty CPU list
  (best-effort)", but the blanket `Err(_) => Vec::new()` swallows *all* error kinds —
  `EACCES` (hardened container), `EIO`, truncated reads — the exact split `fs_read_trim`
  20 lines earlier carefully encodes as `Unsupported` vs `Syscall`. A real I/O failure
  becomes a zero-CPU node indistinguishable from a legitimately empty one.
- Failure scenario: (a) `pin_current_thread_to_node` on such a node builds an empty
  `cpu_set_t`, `sched_setaffinity` fails with `EINVAL`, and the caller sees a baffling
  `AffinityError::Syscall` far from the root cause; (b) if every node's cpulist read
  fails while the `online` read succeeded, `probe()` still returns `Ok` with
  `num_nodes() >= 1`, so `detect()` returns that broken topology instead of degrading to
  `FallbackSingleNodeTopology` (it only falls back when `probe()` errors or reports zero
  nodes) — and `shamir-index` consumes `detect()` directly, so the broken topology flows
  into production registries.
- Suggested fix: match on the error kind — `NotFound` → empty vec (the documented
  best-effort case), anything else → propagate with `?`; additionally have `detect()`
  degrade to the fallback when the probed topology owns zero CPUs in total.
- Also flagged by: security-crypto (3.3, nit).

### 6.3 — medium — `CPU_SET` without a `CPU_SETSIZE` bound: silent mask truncation (glibc) / out-of-bounds write (musl) / bounds panic *(flagged by three lenses)*
- File:line: `crates/shamir-numa/src/linux.rs:139-150` (loop at `:142-144`, `CPU_SET` at
  `:143` — verified; `// SAFETY:` block `:129-138`).
- Issue: `libc::CPU_SET(cpu.0, &mut cpu_set)` on a fixed 1024-bit `cpu_set_t` (128
  bytes) is silently ignored for `cpu.0 >= 1024` under glibc — libc 0.2.x's Rust helper
  indexes `__bits[cpu / 64]` over a `[c_ulong; 16]`, so debug builds trip the
  `debug_assert!` and release builds hit the bounds-checked index and panic; musl's
  `CPU_SET` macro has no bounds check at all — an out-of-bounds stack write — and the
  crate's own README plans an `x86_64-unknown-linux-musl` build for the QEMU tier. On a
  host where the highest CPU number reaches 1024+ (dense 4/8-socket big iron; distro
  kernels configure `NR_CPUS` up to 8192; large VMs), the pin path fails — reachable
  purely from kernel-reported data, no attacker needed. The SAFETY comment covers
  zero-init, pointer validity, `cpusetsize` ABI, and `pid = 0` but never mentions the
  `CPU_SETSIZE` limit — the documented soundness argument for the unsafe block is
  incomplete.
- Failure scenario: 8-socket server, node 7 owns CPUs 3584-4095; startup pins worker
  threads per node; pinning to node 7 panics ("index out of bounds: the len is 16 but
  the index is 56") inside the `unsafe` block and aborts the process (glibc release); a
  node *straddling* 1024 yields a partial mask and `sched_setaffinity` returns `Ok`
  while the thread can still migrate onto the node's high CPUs — silent no-op pin; a
  musl build corrupts the stack.
- Suggested fix: size the mask dynamically with `libc::CPU_ALLOC`/`CPU_ALLOC_SIZE` from
  the highest known CPU (max over `cores_on_node` + 1), or explicitly filter/reject
  `cpu.0 >= libc::CPU_SETSIZE` (skip under the same best-effort policy the parser uses,
  or return a dedicated `AffinityError` variant); document the limit in the SAFETY
  block; extracting a pure `fn build_cpu_set(&[CpuId]) -> Result<libc::cpu_set_t,
  AffinityError>` would also make the ≥1024 path unit-testable off-Linux.
- Also flagged by: security-crypto (3.1), correctness-tdd (1.4).

### 6.4 — medium — No error-path tests for the Linux probe layer; error branches never compiled on the primary gate
- File:line: `crates/shamir-numa/src/linux.rs:167-173` (`fs_read_trim` mapping),
  `:52-93` (`probe` branches), `:179-200` (inline tests, happy-path only — verified).
- Issue: `AffinityError::Unsupported` and `AffinityError::Syscall` are never constructed
  or asserted in any test that runs on the CI matrix — only `NodeOutOfRange` is
  (mock_tests, fallback_tests). `fs_read_trim`'s NotFound-vs-other mapping, `probe()`'s
  empty-`online` → `Unsupported` branch, and the per-node swallow from 6.2 are all
  untested: there is no seam to inject a missing/unreadable sysfs file (reads and parse
  are not separated, unlike the purely-tested `parse_cpulist`; the probe's dense-slot
  mapping of non-contiguous node ids is likewise never deterministically exercised).
  The inline Linux-only test module covers only happy paths against a real host, as does
  `tests/linux_topology.rs`. And because `linux.rs` is `cfg(target_os = "linux")`, none
  of these error branches is even compiled by the Windows dev-host gate that CLAUDE.md
  makes mandatory.
- Failure scenario: any regression in the error mapping — flipping
  `Unsupported`/`Syscall` in `fs_read_trim`, or widening the per-node swallow further —
  passes the entire suite unnoticed.
- Suggested fix: separate read from parse in the probe (take file contents or a tiny
  read trait / `from_parts(online, cpulists)` constructor), unit-test the mapping and
  both `probe()` failure branches platform-independently; add the huge-range cpulist
  test (6.1); keep the Linux-only tests in a `tests/` directory per the documented
  layout (7.1).
- Also flagged by: correctness-tdd (1.5).

### 6.5 — low — `detect()` degrades silently — swallowed probe error has no observability
- File:line: `crates/shamir-numa/src/detect.rs:24-31` (`if let Ok(topo) = ...` —
  verified).
- Issue: the probe error is discarded with no log. The workspace standardises on
  `tracing` elsewhere; this crate depends on nothing, so a deployment that silently lost
  NUMA awareness (missing sysfs, probe `Syscall`) is indistinguishable from a genuine
  single-socket host.
- Failure scenario: NUMA-replicated index registries (`shamir-index` already calls
  `detect()` on construction paths) run degraded in production with zero trace evidence
  of why; the misperformance is diagnosed only by archaeology.
- Suggested fix: emit `warn!`/`info!` (or at least record the probe error) when the
  Linux probe fails and the fallback is chosen; an optional `tracing` dependency
  suffices.

### 6.6 — nit — `AffinityError::Unsupported` is overloaded and carries no source
- File:line: `crates/shamir-numa/src/error.rs:10-16`.
- Issue: the same variant covers "sysfs hierarchy missing" (from `probe`) and "platform
  genuinely has no affinity", while the doc asks callers to treat it as a soft no-op;
  with no `source` or context, a caller logging the error cannot distinguish "container
  without sysfs" from "kernel without NUMA support". The rest of the enum follows the
  thiserror/`#[from]` discipline exactly.
- Failure scenario: misleading diagnostics only.
- Suggested fix: attach context (`Unsupported { path: &'static str }`) or split a
  `SysfsUnavailable` variant, keeping `Unsupported` for the genuine platform case.

## 7. style-claude-md

Largely exemplary: `src/tests/mod.rs` is a manifest-only re-export file (the only
`mod.rs` in the crate), wired through `lib.rs`'s `#[cfg(test)] mod tests;`;
implementation lives in flat sibling files; all imports sit at file headers; every file
owns exactly one primary export (`node.rs`'s `NodeId`+`CpuId` is a legitimately
closely-coupled identifier pair, which the rule explicitly permits). One genuine
violation plus the stale scope docs.

### 7.1 — medium — Inline `#[cfg(test)] mod tests` in an implementation file *(flagged by three lenses)*
- File:line: `crates/shamir-numa/src/linux.rs:179-200` (verified:
  `#[cfg(all(test, target_os = "linux"))] mod tests` with
  `probe_on_real_linux_host_succeeds`, `current_node_is_in_range`); convention: CLAUDE.md
  "Test organisation" rule 5 ("Never embed `#[cfg(test)] mod tests { ... }` inline …
  Move them to the `tests/` directory").
- Issue: every other module routes its tests through `src/tests/`, but `linux.rs`
  carries an embedded two-test module, so the crate's test inventory is split across two
  layouts. The crate's own sibling pattern already supports cfg-gated entries in the
  manifest.
- Failure scenario: `src/tests/mod.rs`'s manifest does not list these tests, so a reader
  triaging `src/tests/` — or a future refactor that moves/renames `linux.rs`, or a
  cleanup that strips the inline block — can silently lose the crate's only
  real-sysfs `LinuxTopology` coverage.
- Suggested fix: move the two tests to `src/tests/linux_tests.rs` wired from
  `src/tests/mod.rs` with `#[cfg(all(test, target_os = "linux"))] pub mod linux_tests;`
  (the `cfg` gate is needed because `src/tests/` compiles on every platform while
  `linux.rs` is Linux-only). Delete the inline module. The existing `use super::*;`
  inside the block is a documented import exception; the block itself is not.
- Also flagged by: correctness-tdd (1.6), api-wire-protocol (5.6).

### 7.2 — low — *(same defect as 5.3)* — stale "Фаза 1 scope" docs contradict shipped code
- Full write-up at **5.3** (primary: api-wire-protocol). Style framing: because the
  discipline rules forbid touching unrelated comments piecemeal, these staleness spots
  otherwise never get corrected — they need the docs-only-sweep convention to land.

### 7.3 — nit — *(same defect as 1.7)* — doc example in `cpulist.rs` is never compiled or executed
- Full write-up at **1.7** (primary: correctness-tdd). Style framing: with
  `doctest = false` the example is sanctioned as illustration, but it *asserts* specific
  behaviour (`"0-1,4"` → `[CpuId(0), CpuId(1), CpuId(4)]`) with no mechanism catching
  drift — drop the assertions in favour of one illustrative call plus a pointer to
  `cpulist_tests.rs`, so the unit tests remain the single verified source of truth.

---

## Finding counts

| Severity | Lens-tagged findings | Finding numbers (a dedup group counts once in the right-hand column) |
|---|---|---|
| critical | 0 | — |
| high | 3 | 2.1 (mirror race — one defect, three lenses + perf nit folded: 1.1, 5.1, 4.3) |
| medium | 12 | 1.2 + 2.2 (node-0-only test — two lenses) · 5.2 + 1.3 (OOB NodeId clamp — two lenses) · 6.1 + 1.8 + 3.2 + 4.2 + 5.8 (parse_cpulist unbounded — five lenses) · 6.2 + 3.3 (probe error swallow — two lenses) · 6.3 + 1.4 + 3.1 (CPU_SETSIZE — three lenses) · 6.4 + 1.5 (probe untestable — two lenses) · 5.3 + 7.2 (stale Фаза docs — two lenses) · 7.1 + 1.6 + 5.6 (inline tests — three lenses) |
| low | 15 | 1.7 + 7.3 (dead doctest — two lenses) · 2.3 (detect() re-probe) · 4.1 (zero-overhead claim) · 5.4 (raw node id lost) · 5.5 (Guard in public API) · 5.7 (coverage claims / phantom numa.yml) · 6.5 (silent degrade) |
| nit | 8 | 1.9 (redundant guards) · 4.4 (no read-path bench) · 5.9 (constructor conventions) · 5.10 (README test cmd) · 6.6 (Unsupported overload) |
| **total** | **38** | lens-tagged findings; **21 distinct defects** after dedup (9 cross-lens groups + 12 single-lens items; the 4.3 nit folds into 2.1) |

Deduplicated defect census: **0 critical, 1 high, 8 medium, 7 low, 5 nit = 21 distinct
defects** (38 lens-tagged findings as filed). The 38 lens-tagged total reconciles with
the workspace `SUMMARY.md` per-crate row for `shamir-numa` (0/3/12/15/8 = 38, pre-dedup
as documented there); the health-scorecard verdict ("moderate — one real concurrency
bug (replica mirror race); otherwise pillar-clean") matches this synthesis.

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Close the `NodeReplicated` mirror race.** Make the publish phase linearisable:
   epoch-versioned mirror (install `(epoch, Arc<T>)` only over an older epoch) or the
   converging-mirror variant (re-read node 0 after each pass; redo while it changed);
   alternatively single-flight `rcu`+`store` behind a sanctioned low-frequency
   `Mutex` with an inline contention-model comment (write path is cold). Correct the
   consistency-model doc, `store`'s doc, and the "a few nanoseconds" constant. Closes
   **2.1** (+1.1, 5.1, 4.3) — the workspace's top-3 headline issue, live in
   `shamir-index` today.
2. **Land the Red test that pins it — first, per Red/Green.** Extend
   `concurrent_rcu_does_not_lose_updates_on_node_zero` to assert every replica equals
   node 0 after the 8×1000 storm (intermittently red today — that is the point), and add
   a `store`-vs-`rcu` interleave test plus a 4-node-mock convergence storm. Closes
   **1.2/2.2** and is the acceptance gate for item 1.

**P1 — soon**

3. **Bound `parse_cpulist` range expansion:** skip any range wider than a sane cap,
   keep the infallible best-effort signature, add `huge_range_is_skipped`. Closes
   **6.1** (+1.8, 3.2, 4.2, 5.8).
4. **Guard `CPU_SET` against `CPU_SETSIZE`:** dynamic `CPU_ALLOC`-sized mask or explicit
   reject/filter of CPU indices ≥ 1024 with a named `AffinityError`; extend the SAFETY
   comment; extract a pure mask-builder for off-Linux tests. Closes **6.3** (+1.4, 3.1).
5. **Stop `probe()`'s blanket error swallow:** `NotFound` → empty node, other kinds
   propagate; have `detect()` degrade to the fallback when the probed topology owns zero
   total CPUs. Closes **6.2** (+3.3).
6. **Probe testability seam + error-path tests, and fix the test layout in the same
   edit:** `from_parts(online, cpulists)` core with platform-independent mapping tests;
   move the inline `linux.rs` tests into `src/tests/linux_tests.rs` behind the
   `target_os = "linux"` cfg. Closes **6.4** (+1.5) and **7.1** (+1.6, 5.6).
7. **`store_node` (and `load_node`) out-of-range → `Result<_, NodeOutOfRange>`**; keep
   the silent clamp only on the `load_local` handoff, documented. Closes **5.2** (+1.3).

**P2 — backlog**

8. **Docs-accuracy sweep (one docs-only commit):** lib.rs scope section, README
   table/roadmap, `src/tests/mod.rs` tier note; delete or implement the phantom
   `numa.yml` references; render the `cpulist.rs` doc example as non-compiled
   illustration; reword the "zero overhead" claim (or land item 10 and keep it).
   Closes **5.3** (+7.2), **1.7** (+7.3), the workflow facet of **5.7**.
9. **`detect_shared()` memoization + degradation observability:** `OnceLock`-backed
   shared topology, migrate the `shamir-index` call sites, `warn!` on Linux-probe
   fallback. Closes **2.3, 6.5**.
10. **`load_local` single-replica short-circuit (or enum topology dispatch) + a small
    read-path bench** comparing bare `ArcSwap::load` vs `NodeReplicated::load_local`.
    Closes **4.1, 4.4**.
11. **API-surface hygiene batch:** `raw_node_id` accessor (or loud slot-vs-kernel-node
    docs), return `Arc<T>` instead of `arc_swap::Guard` at the boundary, one constructor
    convention for the ≥1-node invariant, context/split for
    `AffinityError::Unsupported`, README test command → `./scripts/test.sh -p
    shamir-numa`. Closes **5.4, 5.5, 5.9, 6.6, 5.10**.
12. **Remaining coverage + guard cleanup:** per-platform `detect()` smoke test and a
    `load_node`/`store_node` clamp test (rest of **5.7**); drop the redundant
    `detect()`/`probe()` guards or cite the enforcing invariants (**1.9**).
