<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-tx — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The principal cell-publication, reservation-pruning, rebuild, and snapshot-minimum mechanisms remain. Ordering-precondition hardening is optional, and several registered oracles cover narrower seams than the overview implies.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 8 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — finalize_reservation is not max-monotonic — the ack-path publish can regress a cell below a newer committed version

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Plain Snapshot writers need not hold commit_lock. Publishing key K at v11 and then v10 overwrites its cell with 10; exact-version current reads can return the older value. Existing monotonic tests invoke publish_cell or seed_version, not this publisher. An oracle must invoke apply_committed_visible in reverse version order and assert both cell version and returned bytes.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:664](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L664); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:503](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L503); [crates/shamir-engine/src/tx/commit.rs:921](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L921); [crates/shamir-tx/src/tests/mvcc_store_tests/publish_monotonic_tests.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/publish_monotonic_tests.rs#L47).

<a id="review-2"></a>

### Claim 2 — prune_version_cache can evict a cell holding a live SSI reservation, breaking exactly one committer wins

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A fresh reserved cell has version zero; pruning at a positive minimum removes it without checking reserved_by, permitting another primitive-level claim. Serializable engine commit_lock excludes the report's two-simultaneous-Serializable-committer witness. Required oracle: claim, prune, then verify another claimant still loses and the original owner remains recorded.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:642](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L642); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:533](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L533); [crates/shamir-engine/src/tx/pre_commit.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/pre_commit.rs#L140); [crates/shamir-engine/src/tx/commit.rs:921](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L921).

<a id="review-3"></a>

### Claim 3 — ts_index_rebuild marks the index ready even when the history scan errored

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

An error batch is skipped and ready is unconditionally published. A later healthy query never reconstructs omitted entries and can return an older version or None. The scan oracle is cfg(test), not a production fallback. A discriminating test needs a failing first rebuild followed by healthy storage and an independently specified expected version.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L408); [crates/shamir-tx/src/mvcc_store/mod.rs:428](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L428); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L91); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:101](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L101).

<a id="review-4"></a>

### Claim 4 — RepoTxGate::publish_committed is an unconditional store that can regress last_committed

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The plain store can lower a floor independently advanced by VersionGuard even if its caller holds commit_lock. Inspected in-tree callers are tests, so this is latent public-API risk rather than a current commit-path failure.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:586](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_tx_gate.rs#L586); [crates/shamir-tx/src/version_guard.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/version_guard.rs#L96).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — write_committed_batch_to_history trusts caller-ascending pass for the floor advance

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

pass.last is valid under the explicit ascending-order precondition, and the production drainer constructs an ascending version range. The proposed unsorted witness violates that contract. Assertion/max-fold hardening is optional, not an established defect.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L285); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:394](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L394); [crates/shamir-engine/src/tx/drainer.rs:347](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L347).

<a id="review-6"></a>

### Claim 6 — min_alive check-then-scan-then-read is not atomic — narrow TOCTOU against a just-registering snapshot

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

After an empty traversal, an opener can finish at floor 5 while this thread is preempted; writers can advance to 8 before its fallback load. Returning 8 misses the live floor 5. A scan vacuum can then retain anchor 7 and delete version 5. A final opening-counter check still sees zero. Oracle: completed registration between traversal and fallback, multiple subsequent versions, then exact snapshot-value preservation.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:457](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_tx_gate.rs#L457); [crates/shamir-tx/src/repo_tx_gate.rs:655](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_tx_gate.rs#L655); [crates/shamir-tx/src/repo_tx_gate.rs:666](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_tx_gate.rs#L666); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:193](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L193).

<a id="review-7"></a>

### Claim 7 — release_locks never removes empty KeyLock entries — unbounded locks-map growth

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Each distinct pessimistically acquired key leaves a registry allocation after release; no reclamation path exists. Removal must exclude every outstanding old-lock Arc, including requesters not yet waiting, or lock identity splits.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L75); [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:225](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L225).

Grouping/duplicate: [concurrency-lockfree.md#2](concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Stale doc: predicate_conflicts claims non-tx writes never reach last_committed

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Direct writes do not call the named plain-store method, but guard.commit does advance the same reader floor. The pending-footprint rationale is stale; the observed bound itself is not disproved by this comment.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:793](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_tx_gate.rs#L793); [crates/shamir-tx/src/mvcc_store/mod.rs:818](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L818); [crates/shamir-tx/src/version_guard.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/version_guard.rs#L96).

<a id="review-9"></a>

### Claim 9 — Result&lt;_, String&gt; error types on library APIs

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Remap, staging transforms, retention, and the changelog trait retain string errors. This is error-model maintenance debt, not independently evidence of incorrect runtime behavior.

Evidence: [crates/shamir-tx/src/tx_context.rs:942](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tx_context.rs#L942); [crates/shamir-tx/src/staging_store.rs:329](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/staging_store.rs#L329); [crates/shamir-tx/src/changefeed.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L154).

Grouping/duplicate: [error-handling-lifecycle.md#4](error-handling-lifecycle.md#review-4). This is not an additional independent defect.

<a id="review-coverage-notes"></a>

### Claim Coverage notes — A10, D2, codec, timestamp-index and vectored-read regression guarantees

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Suites are registered. A10's held barrier catches multi-generation deletion; D2 requires a nonmissing valid payload, not equality to the requested snapshot; timestamp comparisons use monotonic timestamps; vectored tests include explicit expected values but also shared per-key oracles. None proves exhaustive scheduling or execution success.

Evidence: [crates/shamir-tx/src/tests/mvcc_store_tests/mod.rs:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/mod.rs#L4); [crates/shamir-tx/src/tests/mvcc_store_tests/a10_toctou_tests.rs:423](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/a10_toctou_tests.rs#L423); [crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs#L77); [crates/shamir-tx/src/tests/mvcc_store_tests/ts_index_tests.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/ts_index_tests.rs#L66); [crates/shamir-tx/src/tests/mvcc_store_tests/get_at_many_tests.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/get_at_many_tests.rs#L119).

## Evidence and recipe corrections

- Claim 5 is precondition hardening, not an open defect: valid callers already supply ascending passes.
- Claim 6's scheduling window is not bounded to a few instructions in elapsed time: thread preemption permits multiple writer cycles. Its concrete snapshot-loss witness warrants Medium rather than Low.
- A max guard on finalization must not indiscriminately clear another owner's reservation; ownership and version monotonicity are separate obligations.
- The timestamp-index versus scan comparison does not cover decreasing timestamps; its fixed array is monotonic.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tx -- Correctness & TDD-coverage
## Summary
The crate shows generally strong TDD discipline (A10 TOCTOU keystone tests, D2
ordering proof-by-test, A2 regression guards, proptest oracles), but the A2
max-monotonic fix was applied to `publish_cell`/`seed_version` only and NOT to
`finalize_reservation` — the function the live ack-path (`apply_committed_visible`)
actually uses to publish cell versions — leaving a stale-read / masked-SSI-conflict
window under out-of-order commits, with no test covering that interleaving. Two
further invariant gaps (GC evicting a cell that holds a live SSI reservation;
ts-index rebuild marking itself ready after swallowed scan errors) plus several
low-severity edge cases round out the findings.

## Findings

### 1. `finalize_reservation` is not max-monotonic — the ack-path publish can regress a cell below a newer committed version
- **File:** `crates/shamir-tx/src/mvcc_store/mod.rs:664-679` (impl; unconditional
  `cell.version = version` at :668); live call path
  `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:450-511`
  (`apply_committed_visible` → `finalize_reservation` at :503).
- **Severity:** high
- **Issue:** `publish_cell` (:545-560) and `seed_version`
  (`mvcc_history.rs:262-277`) both carry the A2 "strictly-greater-than" guard
  precisely because a slower writer/drain can apply an OLDER version after a
  NEWER one already landed. `finalize_reservation` sets `cell.version`
  unconditionally, yet it is the publisher used on EVERY commit's ack-path
  (`apply_committed_visible`, engine `commit_phases.rs:603`) — including plain
  `Snapshot` txs, which per `shamir-engine/src/tx/commit.rs:903-911` run Phase 5a
  WITHOUT `commit_lock` (no CAS/RI-barrier/footprint tokens → no lock). Two
  concurrent commits touching the same key can therefore finalize out of order.
- **Failure scenario:** tx A allocates version 100, tx B version 101 for key K;
  B's Phase-5a lands first (`cell.version = 101`), then A's late Phase-5a runs
  `finalize_reservation(K, 100)` → cell regresses to 100. A subsequent
  `get_current`/`get_current_bytes` takes the direct path (cur_v=100 ≤ floor=101)
  and returns A's value though B committed later — a stale "current" read — and
  `version_of`/`try_reserve` stale-write checks are fed a regressed version
  (masked SSI write-write conflicts), exactly the anomaly class A2 named. The
  window self-heals only when the drainer's `write_committed_to_history` →
  guarded `publish_cell` re-publishes 101.
- **TDD gap (Red phase never run against the live publisher):** the A2 suite
  (`tests/mvcc_store_tests/publish_monotonic_tests.rs`) covers `publish_cell`
  and `seed_version` only; every `apply_committed_visible` /
  `finalize_reservation` test applies versions in ascending order on a single
  task. The comment at `publish_monotonic_tests.rs:46` even asserts Phase 5a
  goes "finalize_reservation -> publish_cell" — it does not; finalize is called
  directly, which is why the guard was missed.
- **Suggested fix:** in `finalize_reservation`'s Occupied branch, write
  `cell.version` only when `version > cell.version` (the `reserved_by = 0`
  clearing stays unconditional), and add an out-of-order ack test
  (apply v11 then v10 for the same key; assert cell stays at 11) mirroring the
  A2 tests — plus a two-task `apply_committed_visible` race test on
  `multi_thread`.

### 2. `prune_version_cache` can evict a cell holding a live SSI reservation, breaking "exactly one committer wins"
- **File:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:531-534`; reservation
  insert at `mod.rs:641-649` (`try_reserve` Vacant → `RecordCell { version: 0,
  reserved_by: txn_id }`).
- **Severity:** medium
- **Issue:** the retain predicate `c.version >= min_alive` ignores
  `c.reserved_by`. A freshly-claimed key's cell has `version: 0`, and
  `min_alive()` (no snapshots ⇒ `last_committed()`) is > 0 on any live repo, so
  a `gc_below`/`gc()` tick that runs between a Serializable committer's
  `try_reserve` (pre_commit) and its `finalize_reservation` (Phase 5a — a window
  spanning the WAL fsync) erases the claim.
- **Failure scenario:** tx A claims fresh key K; GC tick evicts the
  `{version: 0, reserved_by: A}` cell; tx B's `try_reserve(K)` now sees Vacant
  and WINS; both committers proceed to WAL/publish → double-commit / lost
  update under Serializable, defeating the S2 serialization point that
  `concurrent_try_reserve_exactly_one_wins` proves atomically but nothing
  re-proves across a concurrent prune.
- **Suggested fix:** `self.cells.retain_sync(|_k, c| c.version >= min_alive ||
  c.reserved_by != 0);` plus a test that claims a fresh key, runs
  `gc_below(last_committed)`, and asserts a second `try_reserve` still loses.

### 3. `ts_index_rebuild` marks the index ready even when the history scan errored
- **File:** `crates/shamir-tx/src/mvcc_store/mod.rs:398-429` (`Err(_) => continue`
  at :406-409; unconditional `ts_index_ready.store(true)` at :428).
- **Severity:** medium
- **Issue:** a transient storage error during the one-time rebuild is swallowed
  per batch, but the ready flag is still set — `version_at_or_before_ts`
  (`mvcc_history.rs:87-97`) will never retry, so every subsequent as-of-by-ts
  query silently resolves against a permanently incomplete index (returns an
  older version than the true answer). With a durable backend an I/O hiccup at
  first query poisons the index for the process lifetime.
- **Suggested fix:** track whether any batch errored; only store `ready = true`
  on a clean pass (leave false to retry on next query). Add a test with an
  error-injecting store (the `PausableStore`/fault-injection pattern already
  exists in `tests/mvcc_store_tests/test_stores.rs`).

### 4. `RepoTxGate::publish_committed` is an unconditional store that can regress `last_committed`
- **File:** `crates/shamir-tx/src/repo_tx_gate.rs:586-589`.
- **Severity:** low
- **Issue:** the doc claims safety from `commit_lock` "strict monotonicity", but
  non-tx writes and lock-free Snapshot commits advance the floor concurrently
  via `publish_committed_max`/`VersionGuard::fetch_max` WITHOUT that lock. A
  stale `publish_committed(v)` racing a newer advance temporarily lowers the
  reader-visible floor (committed versions become invisible until the next
  advance). No production callers today (grep: tests only), so this is a
  public-API footgun rather than a live bug.
- **Suggested fix:** delegate to `publish_committed_max` (identical on the
  serialized path, safe everywhere), or make it `pub(crate)`/test-only with a
  doc warning.

### 5. `write_committed_batch_to_history` trusts caller-ascending `pass` for the floor advance
- **File:** `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:295-299` (contract:
  "MUST be in ascending commit_version order") and :393-396
  (`pass.last()` assumed to be the max for `publish_committed_max`).
- **Severity:** low
- **Issue:** the ordering contract is unenforced. With an unsorted `pass`,
  `pass.last()` may be below the true max; `publish_committed_max`'s CAS-max
  prevents regression but under-advances the floor for one drain round (brief
  visibility lag, no corruption). No test feeds an unsorted pass.
- **Suggested fix:** `debug_assert!(pass.windows(2).all(|w| w[0].0 <= w[1].0))`
  and/or fold to the true max (`pass.iter().map(|(v,_)| *v).max()`), which is
  O(n) on an off-hot-path batch builder.

### 6. `min_alive()` check-then-scan-then-read is not atomic — narrow TOCTOU against a just-registering snapshot
- **File:** `crates/shamir-tx/src/repo_tx_gate.rs:651-670`; consumer
  `mvcc_gc.rs:156` (scan-path `min_alive`) and :193-201 (anchor computed from
  it).
- **Severity:** low
- **Issue:** the A10 in-flight barrier is checked once at entry. A reader whose
  whole `register_snapshot` (barrier inc → floor read → register → barrier dec)
  lands between the `active_snapshots_opening` check, the `iter_sync` scan, and
  the `last_committed()` fallback is missed by the scan while `last_committed`
  may already have advanced past its floor; `min_alive()` then returns a value
  ABOVE the live snapshot's true floor, and the scan-path vacuum can compute its
  single anchor against that too-high floor — in principle reclaiming the
  version the just-registered snapshot resolves (reader sees the key vanish as
  `None`). The window is a few instructions wide, so this is theoretical, but it
  is exactly the A10 class the barrier was built to close.
- **Suggested fix:** double-check — if `snapshots_opening()` is true after the
  scan, return 0 (conservative) — mirroring the barrier semantics the fast path
  already enforces.

### 7. `release_locks` never removes empty `KeyLock` entries — unbounded locks-map growth
- **File:** `crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:225-239`.
- **Severity:** low (documented as intentional; memory growth, not correctness)
- **Issue:** every distinct key ever locked by a `Pessimistic` tx leaves a
  permanent `Arc<KeyLock>` (Mutex + Notify) in `self.locks`. For a long-lived
  repo with a high-cardinality locked-key population this is an unbounded leak;
  `locks_len()` exists but only tests assert it.
- **Suggested fix:** drop the entry when `holders` becomes empty (remove under
  the same per-key state lock, tolerating a racing re-inserter via `entry`).

### 8. Stale doc: `predicate_conflicts` claims non-tx writes never reach `last_committed`
- **File:** `crates/shamir-tx/src/repo_tx_gate.rs:793-798`.
- **Severity:** nit
- **Issue:** the comment justifies the `last_committed` upper bound with "non-tx
  writes ... do NOT call `publish_committed`", but since the VersionGuard
  cutover `set_versioned`/`delete_versioned` advance the floor via
  `guard.commit()` → watermark fetch_max. Behaviour remains conservative-correct
  (committed non-tx footprints belong in the window); only the rationale is
  wrong — a future maintainer could "fix" the bound based on it.
- **Suggested fix:** update the comment to describe the actual post-cutover
  floor semantics.

### 9. `Result<_, String>` error types on library APIs
- **File:** `crates/shamir-tx/src/tx_context.rs:913-916` (`apply_id_remap`),
  `crates/shamir-tx/src/staging_store.rs:249-252` (`rewrite_set_bytes`),
  `crates/shamir-tx/src/changefeed.rs:154-157` (`ChangelogStore`).
- **Severity:** nit
- **Issue:** CLAUDE.md mandates `thiserror` for library error enums; these
  public library surfaces propagate `String` errors, forcing callers to
  re-wrap.
- **Suggested fix:** introduce a small `thiserror` enum (e.g.
  `RemapError::Decode`) at the next signature-touching change.

## Coverage notes (positive evidence, for balance)
- A10 barrier/anchor-deferral suite (`a10_toctou_tests.rs`) is exemplary
  Red/Green — including the multi-generation-stall keystone that fails against
  the weaker fix, and barrier-aware `gc_below` protection.
- `overlay_ordering_tests.rs` turns the D2 "version ⇒ value" invariant into a
  genuine parallel proof-by-test with a timeout that names the failure mode.
- `version_codec_tests.rs` proptests the separator invariant;
  `ts_index_tests.rs` cross-checks the index against the scan oracle;
  `get_at_many`/`get_current_many` cover direct/fallback/cold/tombstone mixes
  against per-key oracles; `repo_tx_gate_tests.rs` covers the commit-window
  bounds and prune floors.
- The single notable TDD gap is finding #1's interleaving — the one place where
  the regression suite's own comment mis-describes the production call path.

</details>
