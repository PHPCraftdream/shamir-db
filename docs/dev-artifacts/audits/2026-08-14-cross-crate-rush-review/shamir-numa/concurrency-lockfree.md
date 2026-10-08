<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-numa — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The unversioned mirror defect is independently source-proven. Per-cell atomicity, immutable reverse-map reads and a fixture-only Mutex are supported, but whole-operation lock freedom is not.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `rcu`/`store` mirror phase can overwrite newer replicas with a stale value — non-zero nodes diverge from node 0 indefinitely

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

With two replicas, A publishes and captures X1; B publishes and mirrors X2; A writes X1 last to replica 1. Canonical X2 survives while replica 1 remains X1 after quiescence. store/store and mixed writers admit the same ordering. No convergence pass or publication-bound version exists.

Evidence: [crates/shamir-numa/src/node_replicated.rs:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L82); [crates/shamir-numa/src/node_replicated.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L98); [crates/shamir-numa/src/node_replicated.rs:103](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L103); [crates/shamir-numa/src/node_replicated.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L105).

<a id="review-2"></a>

### Claim 2 — Concurrency tests never assert mirror convergence — finding 1 is invisible to CI

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The registered concurrent test inspects only canonical node 0. Its increment oracle is meaningful but remains unchanged if every concurrent mirror is stale; sequential mirror tests do not force that counterexample.

Evidence: [crates/shamir-numa/src/tests/mod.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/mod.rs#L12); [crates/shamir-numa/src/tests/node_replicated_tests.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/node_replicated_tests.rs#L96); [crates/shamir-numa/src/tests/node_replicated_tests.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/node_replicated_tests.rs#L116).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `detect()` re-probes `/sys` on every call — no memoized variant, and consumers already call it per-instance construction

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Linux detect performs discovery every time. IndexInfo construction, deserialization and cloning invoke it; async SortedIndexManager::new invokes it synchronously. This proves repeated blocking construction work, not per-record query latency or a measured slowdown.

Evidence: [crates/shamir-numa/src/detect.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/detect.rs#L25); [crates/shamir-numa/src/linux.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L168); [crates/shamir-index/src/base_index/index_info.rs:142](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_info.rs#L142); [crates/shamir-index/src/base_index/index_info.rs:340](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_info.rs#L340); [crates/shamir-index/src/base_index/sorted_index_manager.rs:296](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/sorted_index_manager.rs#L296).

## Evidence and recipe corrections

- Exact arc-swap 1.9.1 published src/lib.rs:622 supplies CAS retries, not a lock around f. Arbitrary callbacks, allocators and destructors can nevertheless block.
- An epoch increment after a successful canonical CAS is unsafe as an ordering recipe: an earlier CAS winner can obtain a later epoch after another writer finishes. Publish version and value together.
- A re-read/retry mirror approach must cover store as well as rcu, preserve intentional store_node divergence, and state quiescence/progress assumptions.
- Inspected sorted create/drop and regular online registration/final-publication phases hold per-table DDL admission. The online scan releases it, so do not generalize whole-body serialization; the inspected definition publications themselves remain guarded.
- Sharing Topology does not share registry contents. SortedIndexManager clones already share their registry; IndexInfo clones intentionally snapshot into independent registries.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-numa — Concurrency & lock-free invariants

## Summary

The crate is otherwise a faithful implementation of CLAUDE.md's five pillars: `NodeReplicated<T>` is fully lock-free (one cache-line-padded `ArcSwap<T>` per NUMA node, CAS-based `rcu`), the only `Mutex` in the crate is `MockTopology::pin_log` — a sanctioned test fixture with the required inline contention-model comment (`mock.rs:35-36`) — the CPU→node reverse index uses `TFxMap` (Fx-hash pillar), there is no `async` and therefore no lock-across-`.await` risk, and there are no `scc::*` calls (hence no O(N) `len()` exposure). The one substantive defect is in `NodeReplicated`'s mirror phase, which can leave non-zero-node replicas **permanently stale** under concurrent writers — contradicting both the struct's documented consistency model and the in-code invariant comment — and the current concurrency tests cannot catch it.

## Findings

### 1. `rcu`/`store` mirror phase can overwrite newer replicas with a stale value — non-zero nodes diverge from node 0 indefinitely

- **File:line:** `crates/shamir-numa/src/node_replicated.rs:96-108` (mirror loop at 99–107; `store` at 82–87 has the same unsynchronized multi-replica publish shape)
- **Severity:** high
- **Issue:** `rcu` linearises only on the node-0 cell. The mirror phase re-reads node 0 via `load_full()` (line 103) and then blindly `store`s that snapshot to every other replica — with no ordering or versioning that ties a thread's mirror pass to the node-0 value that is latest *at mirror time*. The in-code comment (lines 100–102) claims "Reading it back … keeps the mirror consistent with the value that actually won the CAS", but that holds only when there is a single concurrent writer. Two writers can interleave so that the *loser's* mirrors land last. The same applies to `rcu` racing a plain `store` (arbitrary per-replica interleaving, including a mix of two publications across replicas), since `store` never consults node 0.
- **Failure scenario:** On a multi-node Linux host (thread T1, T2 on a `NodeReplicated` with N > 1 replicas):
  1. T1's `rcu` CAS wins on node 0 → node 0 = X1; T1's `load_full()` reads X1.
  2. T2's `rcu` CAS wins on node 0 → node 0 = X2; T2 mirrors X2 to nodes 1..N.
  3. T1 now mirrors its earlier X1 to nodes 1..N.
  Final state: node 0 = X2, nodes 1..N = **X1**. Nothing re-synchronises: readers only `load()` their own node's cell, so the divergence persists **until the next successful `rcu`/`store`** — potentially forever on an idle registry. This is not the "a few nanoseconds" transient the struct doc (lines 22–30) promises; it directly violates the "mirrors the winning value" invariant. Live consumers make concurrent writers plausible: `shamir-index` calls `.rcu(...)` from many distinct code paths (`sorted_index_manager.rs:508, 633, 844, 954, 1145, 1628`; `index_info.rs:252, 270`) plus a plain `store` (`sorted_index_manager.rs:2784`), so a stale index-definition list on a non-zero node is a silent correctness hazard (e.g. a query on node 1 consults a dropped/renamed index definition). Mitigations: single-node topologies (Windows/CI/dev) never execute the mirror loop, and node 0 is always correct.
- **Suggested fix:** Give the publish a linearisable version, e.g. keep an `AtomicU64` epoch advanced by the node-0 CAS winner and have the mirror phase install `(epoch, Arc<T>)` only if the target's epoch is older (CAS retry). A lighter variant that restores the "eventually equals node 0" invariant without a version: after each mirror pass, re-read node 0; if it changed during the pass, redo the pass with the fresh value and only exit when a full pass completes with node 0 stable (the last writer to finish mirroring then always converges all replicas to the final node-0 value). At minimum, correct the doc-comment and in-code invariant claims to describe the actual (weaker) guarantee.

### 2. Concurrency tests never assert mirror convergence — finding 1 is invisible to CI

- **File:line:** `crates/shamir-numa/src/tests/node_replicated_tests.rs:95-117` (`concurrent_rcu_does_not_lose_updates_on_node_zero`)
- **Severity:** medium
- **Issue:** The only multi-threaded writer test asserts the final value of **node 0 only** (line 116). Node 0 is exactly the cell the CAS loop protects, so the test passes even when the mirror phase leaves nodes 1..N stale — the exact failure mode of finding 1 is unreachable by the current suite. There is also no test interleaving `store` with `rcu`, and no convergence check on a >2-replica mock.
- **Failure scenario:** A regression (or the existing behaviour itself) leaves node 1's replica at `7997` instead of `8000` after the 8×1000 increment storm; all tests stay green.
- **Suggested fix:** After the joins, assert every replica equals node 0 (`for n in 0..replicas { assert_eq!(**r.load_node(NodeId(n)), threads * per) }`) — note this assertion is intermittent *today*, which is the point; land it together with the fix from finding 1. Add a `store`-vs-`rcu` interleaving test and a 4-node-mock convergence storm.

### 3. `detect()` re-probes `/sys` on every call — no memoized variant, and consumers already call it per-instance construction

- **File:line:** `crates/shamir-numa/src/detect.rs:24-43` (Linux arm at 24–31 does 1 + `num_nodes` blocking `std::fs::read_to_string` calls per invocation)
- **Severity:** low
- **Issue:** `detect()` is framed as a bootstrap helper, but the API offers no cached/shared form and no doc note that the result must be reused. It is already being called per instance construction in `shamir-index` (`base_index/index_info.rs:142, 152, 167, 204, 340` — including the `Deserialize` impl, i.e. per hydrated record — and `base_index/sorted_index_manager.rs:299-302`). On Linux each call repeats O(nodes) blocking file reads — synchronous I/O on (potentially) an async-runtime thread, and repeated non-constant per-instance work, contrary to pillars 2 and 3; the topology it returns is also a fresh object each time, so replicas are never shared across those instances.
- **Failure scenario:** A table hydration path deserialises `IndexInfo`s in a loop; on a 4-node host each record costs ~5 sysfs file reads on the calling (async) thread, multiplied across every hydrated record — hidden per-op I/O that no benchmark attributes to `detect()`.
- **Suggested fix:** Add a `OnceLock<Arc<dyn Topology>>`-backed `detect_shared()` (or make `detect()` itself cache) plus a doc note that probing is bootstrap-grade blocking I/O and the returned `Arc` must be reused; migrate the `shamir-index` call sites to the cached form (a sibling reviewer should weigh the consumer side).

## Verified compliant (no action)

- **Pillar 1/5 (lock-free, scc/dashmap/ArcSwap):** per-node `ArcSwap` cells + `CachePadded` (`repr(align(128))`, matches the crossbeam choice) — the sanctioned RCU shape; `LinuxTopology::current_node` is `sched_getcpu` + Fx-map lookup, O(1) and allocation-free on the read hot path (`linux.rs:105-117`). Note arc-swap's `rcu` (verified against arc-swap 1.9 source; semver-compatible with the pinned `"1.7"`) is a load + `compare_and_swap` retry loop — the closure is **not** run under an internal lock, so the "lock-free" claim holds for arbitrary `f`.
- **Sanctioned Mutex:** `MockTopology::pin_log` (`mock.rs:37`) is a test fixture with the required inline contention-model comment — fits the CLAUDE.md test-fixture category.
- **Pillar 4 (Fx hash):** `cpu_to_node: TFxMap` (`linux.rs:40`); immutable after probe, so lock-free concurrent reads are sound.
- **Pillar 3:** no `scc::*::len()` anywhere; `num_replicas()` is a `Box` slice `len()` (O(1)); `load_local` documented and implemented O(1).
- **Async/pinning:** no locks held across `.await` (no async in the crate); all `/sys` I/O is confined to one-shot probe code (but see finding 3 for the consumer-driven leak).

</details>
