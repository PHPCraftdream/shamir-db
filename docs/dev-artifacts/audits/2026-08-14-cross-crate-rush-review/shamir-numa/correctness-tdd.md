<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-numa — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The canonical CAS has a meaningful registered oracle; the crate's concurrent mirror guarantee does not. Portable sequential tests are useful, while Linux discovery coverage depends on host shape.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 7 | 0 | 0 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `NodeReplicated::rcu` mirror loop can permanently strand non-node-0 replicas on a stale value under concurrent writers

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A captures X1, B commits and mirrors X2, then A mirrors X1 and finishes. No repair remains scheduled. Force this production-method interleaving and assert every replica after both writers finish; ordinary engine DDL admission limits production reachability.

Evidence: [crates/shamir-numa/src/node_replicated.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L98); [crates/shamir-numa/src/node_replicated.rs:103](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L103); [crates/shamir-numa/src/node_replicated.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L105); [crates/shamir-engine/src/table/table_manager.rs:1322](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager.rs#L1322).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Concurrency test asserts node 0 only -- the mirror half of `rcu` has zero multi-threaded coverage

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The eight-thread test joins writers and asserts only node 0. Sequential all-replica assertions catch missing mirroring, not reordered mirrors. A deterministic delayed-mirror oracle must inspect every replica and mixed publication methods.

Evidence: [crates/shamir-numa/src/lib.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/lib.rs#L66); [crates/shamir-numa/src/tests/mod.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/mod.rs#L12); [crates/shamir-numa/src/tests/node_replicated_tests.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/node_replicated_tests.rs#L116).

<a id="review-3"></a>

### Claim 3 — Out-of-range `store_node` / `load_node` silently redirect to node 0

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The redirection is expressly documented for both methods and implemented accordingly. No rejection guarantee, built-in topology replacement or production store_node caller establishes a defect. Fallible alternatives are optional API redesign.

Evidence: [crates/shamir-numa/src/node_replicated.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L75); [crates/shamir-numa/src/node_replicated.rs:114](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L114); [crates/shamir-numa/src/node_replicated.rs:121](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/node_replicated.rs#L121).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — `LinuxTopology::pin_current_thread_to_node` uses a fixed 1024-CPU `cpu_set_t`; CPUs &gt;= 1024 pin wrongly or fail

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A discovered CPU ID 1024 reaches checked-index panic in libc 0.2.186 published linux_l4re_shared.rs:1531, not truncation or corruption. The public Result contract is violated; production pinning was not found. A boundary oracle must reject without invoking the syscall.

Evidence: [crates/shamir-numa/src/linux.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L143); [Cargo.lock:1923](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1923); [crates/shamir-numa/src/topology.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/topology.rs#L35).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — `LinuxTopology::probe` decision logic has no unit tests and no injection seam

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

probe reads fixed sysfs paths. Host tests do not discriminate sparse-ID mapping, missing files, non-NotFound errors or reverse-map misses. An injected production discovery path is needed; copied parsing logic would not test this mechanism.

Evidence: [crates/shamir-numa/src/linux.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L54); [crates/shamir-numa/src/linux.rs:74](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L74); [crates/shamir-numa/src/linux.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L179).

Grouping/duplicate: [error-handling-lifecycle.md#4](error-handling-lifecycle.md#review-4). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — Inline `#[cfg(test)] mod tests` inside `linux.rs` violates the documented test layout

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The inline module violates the explicit layout rule but is registered under Linux cfg. Relocation must preserve that cfg and production imports; placement does not establish runtime failure.

Evidence: [crates/shamir-numa/src/linux.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L179); [crates/shamir-numa/src/lib.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/lib.rs#L47); [CLAUDE.md:594](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L594).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — Dead doctest in `cpulist.rs` -- assertions never compiled or run

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Nonexecution follows explicit manifest policy, not a defect. Registered tests assert empty input and mixed inclusive ranges, providing counter-evidence to the historical claim that no behavioral mechanism catches drift.

Evidence: [crates/shamir-numa/Cargo.toml:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/Cargo.toml#L10); [crates/shamir-numa/src/tests/mod.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/mod.rs#L9); [crates/shamir-numa/src/tests/cpulist_tests.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/cpulist_tests.rs#L25); [crates/shamir-numa/src/tests/cpulist_tests.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/cpulist_tests.rs#L36).

<a id="review-8"></a>

### Claim 8 — `parse_cpulist` allocates unboundedly on a large range token

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Valid ascending ranges are expanded before deduplication without span or aggregate budgets. Large external tooling input causes excessive work/allocation; current service callers use trusted sysfs. No exact OOM threshold is established.

Evidence: [crates/shamir-numa/src/cpulist.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L38); [crates/shamir-numa/src/cpulist.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L40); [crates/shamir-numa/src/linux.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L57).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — Redundant guards in `detect()` / `probe()`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

probe rejects an empty node list and parse_cpulist already sorts output. The extra positive-count check and sort are present but harmless; their removal is cleanup, not correctness repair.

Evidence: [crates/shamir-numa/src/detect.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/detect.rs#L26); [crates/shamir-numa/src/linux.rs:62](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L62); [crates/shamir-numa/src/linux.rs:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L70); [crates/shamir-numa/src/cpulist.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L51).

## Evidence and recipe corrections

- The overview and correction bullets still call exact CPU_SET behavior unverified despite the parent refinement. Independently inspected checksummed libc 0.2.186 establishes bounds panic.
- The existing load_local out-of-range test cannot detect deletion of NodeReplicated's clamp: MockTopology already returns 0, and both replicas initially contain the same value. Use a non-clamping topology and distinguishable replicas.
- An every-replica storm is schedule-dependent, not guaranteed Red. Force the stale-mirror schedule through the production publication seam.
- A generic writer Mutex is not automatically sanctioned merely because current consumers perform DDL. It changes progress/reentrancy semantics and requires unwind-safe release and a demonstrated contention model.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-numa -- Correctness & TDD-coverage

## Summary

The platform-independent core is genuinely well-tested: `parse_cpulist`, the fallback/mock topologies, and `CachePadded` have real behavioural coverage in the prescribed `src/tests/` layout, and the `MockTopology` thread-local isolation discipline in the tests is sound. The flagship primitive, however, has a real correctness gap: `NodeReplicated::rcu`'s mirror step loses under concurrent writers, permanently stranding non-node-0 replicas on a stale value -- directly contradicting the doc's "a few nanoseconds ... eventual consistency" claim -- and the one concurrency test asserts only the node-0 half that arc-swap already guarantees, so no test can fail on this race (vacuous vs the documented replication contract). Secondary gaps: `LinuxTopology::probe`'s decision logic is untestable as written (hard-coded sysfs paths, host-only tests), a fixed-size `cpu_set_t` silently mis-pins on >1024-CPU hosts, and two convention violations (inline `mod tests` in `linux.rs`, a dead doctest in `cpulist.rs` given `doctest = false`). Note that `shamir-index` (`index_info.rs`, `sorted_index_manager.rs`) already consumes `NodeReplicated`, so the `rcu` mirror race is live code, not scaffolding.

## Findings

### 1. `NodeReplicated::rcu` mirror loop can permanently strand non-node-0 replicas on a stale value under concurrent writers
- **File:line:** `crates/shamir-numa/src/node_replicated.rs:96-108` (mirror at 103-106); contradicts the consistency-model doc at lines 22-30.
- **Severity:** high
- **Issue:** The node-0 CAS loop is lost-update-safe (correct), but the mirror step reads `replicas[0].load_full()` *after* the commit and stores it to nodes 1..n with no ordering against other writers. With two concurrent `rcu` callers A and B (a mode the doc explicitly supports: "concurrent `rcu` callers retry instead of clobbering each other"): A commits node 0 = v_old, loads `published` = v_old; B commits node 0 = v_new and mirrors v_new to node 1; A then stores its stale v_old over node 1. Nothing re-mirrors -- node 1 stays at v_old while node 0 holds v_new until the *next* `rcu`/`store`, and if that was the last update, forever. The doc claims the divergence window is "a few nanoseconds" and therefore "eventual consistency"; under concurrency the window is unbounded and convergence is not guaranteed. The same stale-store interleave applies between `store` and `rcu` mirror runs.
- **Failure scenario:** On a real NUMA host, a DDL thread adds an index definition via `rcu`; concurrent DDL activity interleaves mirrors; a quiet node's readers then load the *old* index-definition list indefinitely -- the final published config never reaches that replica.
- **Suggested fix:** Make the mirror monotonic: stamp published values with a monotonic epoch and have the mirror only overwrite lower epochs; or re-read node 0 after the mirror loop and retry while it differs; or single-flight the whole rcu+mirror through one lock (the write path is cold -- a sanctioned low-frequency `std::sync::Mutex` with an inline contention-model comment fits CLAUDE.md). At minimum, correct the doc to state replicas may lag arbitrarily under concurrent writers.

### 2. Concurrency test asserts node 0 only -- the mirror half of `rcu` has zero multi-threaded coverage
- **File:line:** `crates/shamir-numa/src/tests/node_replicated_tests.rs:96-117`
- **Severity:** medium
- **Issue:** `concurrent_rcu_does_not_lose_updates_on_node_zero` verifies exactly the guarantee arc-swap already provides (node-0 CAS) and nothing about the crate's own addition (the mirror to nodes 1..n). The test's name promises "does not lose updates", but replication freshness under contention -- the entire point of `NodeReplicated` -- is unasserted, so finding #1 cannot fail any test today. This is a coverage hole against CLAUDE.md's Red/Green/Refactor discipline: the Red test for the mirror race was never written.
- **Failure scenario:** The mirror race (finding #1) ships with a green suite.
- **Suggested fix:** Extend the test: after joining the 8x1000 `rcu` threads, assert `load_node(NodeId(n)) == threads * per` for **every** n, not just node 0. That assertion is the Red test for finding #1 (it will fail under concurrency today).

### 3. Out-of-range `store_node` / `load_node` silently redirect to node 0
- **File:line:** `crates/shamir-numa/src/node_replicated.rs:77-79, 116-118` (clamp in `replica()` at 121-128)
- **Severity:** medium
- **Issue:** `replica()` clamps an out-of-range `NodeId` to slot 0. For reads this is a benign best-effort degrade; for the write path (`store_node`) it silently overwrites a *different* node's replica with no error. A caller with a stale `NodeId` (e.g. derived from a stale `num_nodes()` snapshot or a typo) lands its write on node 0, corrupting whatever staged per-node config node 0 held. The behaviour is documented in the doc-comments, but a wrong-target silent write still clashes with CLAUDE.md's error-handling rule ("Return `Result<T, E>`") for an operation that can fail semantically.
- **Failure scenario:** `NodeReplicated<Vec<Def>>` staged via `store_node`; topology shrinks (topology object swapped); caller still holds `NodeId(3)` -> `store_node(NodeId(3), defs)` overwrites node 0's replica instead of erroring.
- **Suggested fix:** Return `Result<(), AffinityError::NodeOutOfRange>` from `store_node` (and consider `load_node` panicking in debug builds on OOB). Keep the silent clamp only on the read path, where a degrade is legitimate.

### 4. `LinuxTopology::pin_current_thread_to_node` uses a fixed 1024-CPU `cpu_set_t`; CPUs >= 1024 pin wrongly or fail
- **File:line:** `crates/shamir-numa/src/linux.rs:139-150`
- **Severity:** medium
- **Issue:** `libc::cpu_set_t` is 128 bytes = 1024 bits. `libc::CPU_SET(cpu.0, ...)` silently ignores indices >= 1024. On hosts with more than 1024 logical CPUs (real: 4-socket x 192-core x SMT2 = 1536), a node whose CPUs are all >= 1024 yields an all-zero mask and `sched_setaffinity` fails with EINVAL (surfaced as `AffinityError::Syscall` -- loud but spurious); a node *straddling* 1024 yields a partial mask and the call returns `Ok(())` while the thread can still migrate onto the node's high CPUs -- a silent no-op pin violating the documented post-condition. The SAFETY comment covers memory validity but not the mask-capacity edge.
- **Failure scenario:** Pin to node 1 owning CPUs 512-1535 returns Ok; the thread still lands on CPU 1400; "node-local" reads are not node-local, with no diagnostic.
- **Suggested fix:** Size the mask from the highest known CPU (max over `cores_on_node` + 1) and pass a dynamically sized buffer to `sched_setaffinity`; or reject `cpu.0 >= 1024` explicitly with a dedicated `AffinityError` instead of silently dropping bits.

### 5. `LinuxTopology::probe` decision logic has no unit tests and no injection seam
- **File:line:** `crates/shamir-numa/src/linux.rs:52-93`
- **Severity:** low
- **Issue:** The only tests are host-dependent (`probe_on_real_linux_host_succeeds`, `current_node_is_in_range`, linux.rs:179-200) which skip entirely on non-Linux and fail-or-pass depending on what the CI box happens to look like. The non-trivial logic -- dense-slot mapping of potentially non-contiguous online node ids (e.g. sysfs `online` = `0,2`), missing-cpulist -> empty-node best-effort, and the CPU->node reverse map with its node-0 degrade -- is never exercised by a deterministic Red test. Hard-coded `/sys` paths make such tests impossible as written.
- **Failure scenario:** A regression in slot mapping (e.g. raw node id used as slot index) would pass every test unless the test host happens to have a hole in its node numbering.
- **Suggested fix:** Extract a pure core, e.g. `fn from_parts(node_ids: &[usize], per_node_cpulists: &[&str]) -> Self`, and keep `probe()` as a thin I/O shell; unit-test the core in `src/tests/` (edge cases: non-contiguous ids, duplicate cpulists, empty cpulists).

### 6. Inline `#[cfg(test)] mod tests` inside `linux.rs` violates the documented test layout
- **File:line:** `crates/shamir-numa/src/linux.rs:179-200`; convention: `CLAUDE.md` "📁 Test organisation" rules 3-5 ("Never embed `#[cfg(test)] mod tests { ... }` inline ... Move them to the `tests/` directory")
- **Severity:** low
- **Issue:** Every other module routes its tests through `src/tests/` (wired via `lib.rs:66-67`), but `linux.rs` carries an embedded test module. The crate's own sibling pattern already supports cfg-gated entries in the manifest.
- **Failure scenario:** None at runtime -- convention drift only, but it is the exact pattern CLAUDE.md bans.
- **Suggested fix:** Move to `src/tests/linux_tests.rs` with `#[cfg(target_os = "linux")] pub mod linux_tests;` in `src/tests/mod.rs`.

### 7. Dead doctest in `cpulist.rs` -- assertions never compiled or run
- **File:line:** `crates/shamir-numa/src/cpulist.rs:24-28`; policy: `crates/shamir-numa/Cargo.toml:9-13` (`doctest = false`, "Doctests are banned project-wide ... behavioural coverage lives in the `tests/` modules")
- **Severity:** low
- **Issue:** The doc-example's two `assert_eq!`s are never executed (and never even compiled) because doctests are disabled, giving the illusion of verified coverage. It also duplicates `mixed_ranges_and_indices` and `empty_is_empty` in `cpulist_tests.rs`, which do run.
- **Failure scenario:** If the parser ever drifts from the example, nothing fails.
- **Suggested fix:** Render the example as non-compiled illustrative text (the convention the Cargo.toml comment prescribes) or delete it -- the behavioural tests already exist.

### 8. `parse_cpulist` allocates unboundedly on a large range token
- **File:line:** `crates/shamir-numa/src/cpulist.rs:36-43`
- **Severity:** low
- **Issue:** A token like `0-4294967295` parses fine and `extend(lo..=hi)` materialises billions of `usize`s before the sort/dedup -> OOM. `/sys` is trusted, but the function is `pub` explicitly "useful standalone for tooling that inspects `/proc` / `/sys` cpu masks", i.e. it is exposed to non-sysfs inputs.
- **Failure scenario:** A corrupt/hand-edited cpulist string fed by tooling takes the process down.
- **Suggested fix:** Skip ranges wider than a sane cap (e.g. > `1 << 20`) or verify the span against the host's CPU count before materialising.

### 9. Redundant guards in `detect()` / `probe()`
- **File:line:** `crates/shamir-numa/src/detect.rs:26`; `crates/shamir-numa/src/linux.rs:69-70`
- **Severity:** nit
- **Issue:** `topo.num_nodes() > 0` in `detect()` is dead -- `probe()` already returns `Err(Unsupported)` when the node list is empty (linux.rs:62-64). `node_ids_sorted.sort_unstable()` is a no-op -- `parse_cpulist` already returns sorted, de-duplicated output. Harmless, but they imply an invariant the callees already enforce and can mislead a reader into thinking those states are reachable.
- **Failure scenario:** None.
- **Suggested fix:** Drop both, or replace with a one-line comment citing the enforcing invariant upstream.

</details>
