<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tx — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The ack-path regression, reservation eviction, incomplete timestamp rebuild, and min_alive race remain. Several claimed consequences and proposed fixes need qualification; registered tests do not close these gaps.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — finalize_reservation is not max-monotonic — the ack-path publish can regress a cell below a newer committed version

Status: `confirmed-open`. Current risk: `high`.

Occupied cells are overwritten unconditionally. Plain Snapshot commits can publish without commit_lock; direct reads use the regressed exact version. Existing monotonic tests exercise different publishers.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:668](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L668); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:503](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L503); [crates/shamir-engine/src/tx/commit.rs:921](../../../../../crates/shamir-engine/src/tx/commit.rs#L921); [crates/shamir-tx/src/tests/mvcc_store_tests/publish_monotonic_tests.rs:47](../../../../../crates/shamir-tx/src/tests/mvcc_store_tests/publish_monotonic_tests.rs#L47).

<a id="review-2"></a>

### Claim 2 — prune_version_cache can evict a cell holding a live SSI reservation, breaking exactly one committer wins

Status: `confirmed-open`. Current risk: `medium`.

A reserved fresh cell has version zero and retain_sync ignores reserved_by. The reservation primitive is violated, but the alleged simultaneous production Serializable commits are excluded by commit_lock.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:642](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L642); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:533](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L533); [crates/shamir-engine/src/tx/pre_commit.rs:140](../../../../../crates/shamir-engine/src/tx/pre_commit.rs#L140); [crates/shamir-engine/src/tx/commit.rs:921](../../../../../crates/shamir-engine/src/tx/commit.rs#L921).

<a id="review-3"></a>

### Claim 3 — ts_index_rebuild marks the index ready even when the history scan errored

Status: `confirmed-open`. Current risk: `medium`.

Errored batches are skipped and ready is always set. Subsequent queries use the incomplete index without rebuilding or a production scan fallback.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:408](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L408); [crates/shamir-tx/src/mvcc_store/mod.rs:428](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L428); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:91](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L91).

<a id="review-4"></a>

### Claim 4 — RepoTxGate::publish_committed is an unconditional store that can regress last_committed

Status: `confirmed-open`. Current risk: `low`.

The public method still performs a plain store while independent publishers advance the same atomic monotonically. Repository search found only test callers, so this is latent API risk.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:586](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L586); [crates/shamir-tx/src/version_guard.rs:96](../../../../../crates/shamir-tx/src/version_guard.rs#L96); [crates/shamir-tx/src/tests/repo_tx_gate_tests.rs:33](../../../../../crates/shamir-tx/src/tests/repo_tx_gate_tests.rs#L33).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — write_committed_batch_to_history trusts caller-ascending pass for the floor advance

Status: `confirmed-open`. Current risk: `low`.

The maximum is taken from pass.last without validation. The production drainer consumes an ordered version range, so the under-advance requires violating the public method's documented precondition.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:285](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L285); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:394](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L394); [crates/shamir-engine/src/tx/drainer.rs:347](../../../../../crates/shamir-engine/src/tx/drainer.rs#L347).

<a id="review-6"></a>

### Claim 6 — min_alive check-then-scan-then-read is not atomic — narrow TOCTOU against a just-registering snapshot

Status: `confirmed-open`. Current risk: `low`.

An opener can complete after the map traversal and before the fallback floor read; its retained snapshot can be older than the returned minimum. A counter-only post-check cannot detect completed openers.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:655](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L655); [crates/shamir-tx/src/repo_tx_gate.rs:659](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L659); [crates/shamir-tx/src/repo_tx_gate.rs:666](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L666); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:193](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L193).

<a id="review-7"></a>

### Claim 7 — release_locks never removes empty KeyLock entries — unbounded locks-map growth

Status: `confirmed-open`. Current risk: `medium`.

Distinct pessimistically acquired keys retain their Arc<KeyLock> after holders are removed. No eviction exists; any removal must account for all outstanding requester clones, not merely parked waiters.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:75](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L75); [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:223](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L223); [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:232](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L232).

Grouping/duplicate: `concurrency-lockfree.md#2`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Stale doc: predicate_conflicts claims non-tx writes never reach last_committed

Status: `confirmed-open`. Current risk: `nit`.

The rationale still treats non-tx versions as unpublished despite guard.commit advancing last_committed. The exact named plain-store method is indeed unused by those writes; the floor rationale is stale.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:793](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L793); [crates/shamir-tx/src/mvcc_store/mod.rs:818](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L818); [crates/shamir-tx/src/version_guard.rs:96](../../../../../crates/shamir-tx/src/version_guard.rs#L96).

<a id="review-9"></a>

### Claim 9 — Result<_, String> error types on library APIs

Status: `confirmed-open`. Current risk: `low`.

The public remap, staging transformation, and changelog trait boundaries retain String errors. This is error-model/maintainability debt, not independently a runtime correctness failure.

Evidence: [crates/shamir-tx/src/tx_context.rs:942](../../../../../crates/shamir-tx/src/tx_context.rs#L942); [crates/shamir-tx/src/staging_store.rs:329](../../../../../crates/shamir-tx/src/staging_store.rs#L329); [crates/shamir-tx/src/changefeed.rs:154](../../../../../crates/shamir-tx/src/changefeed.rs#L154).

Grouping/duplicate: `error-handling-lifecycle.md#4`. This row is not another independent defect.

<a id="review-coverage-notes"></a>

### Claim Coverage notes — A10, D2, codec, timestamp-index and vectored-read regression guarantees

Status: `not-applicable`. Current risk: —.

The cited suites are registered and contain meaningful assertions. A10's held-barrier test detects multi-generation deletion; D2 detects missing values but is not exhaustive scheduling or exact snapshot-value proof.

Evidence: [crates/shamir-tx/src/lib.rs:33](../../../../../crates/shamir-tx/src/lib.rs#L33); [crates/shamir-tx/src/tests/mvcc_store_tests/mod.rs:4](../../../../../crates/shamir-tx/src/tests/mvcc_store_tests/mod.rs#L4); [crates/shamir-tx/src/tests/mvcc_store_tests/a10_toctou_tests.rs:423](../../../../../crates/shamir-tx/src/tests/mvcc_store_tests/a10_toctou_tests.rs#L423); [crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs:77](../../../../../crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs#L77); [crates/shamir-tx/src/tests/mvcc_store_tests/ts_index_tests.rs:107](../../../../../crates/shamir-tx/src/tests/mvcc_store_tests/ts_index_tests.rs#L107).

## Corrections and qualified non-findings

- Finding 2 proves loss of the reservation, not the stated two-Serializable-committer production race: both live commit paths retain commit_lock through publication.
- Finding 6's suggested post-scan snapshots_opening check is insufficient for an opener that has already completed; require a captured conservative floor or registration-generation validation.
- The timestamp-index scan oracle is cfg(test) only; there is no production fallback even when rebuilding leaves the index empty.
- Claim-then-GC coverage and out-of-order finalize coverage are still absent from the registered tests.

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
