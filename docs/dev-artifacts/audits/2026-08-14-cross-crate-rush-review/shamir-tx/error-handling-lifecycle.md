<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-tx — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Failed direct writes mask prior values, journal failures remain ambiguous, and bad timestamp rebuilds remain cached. Refuted panic scenarios remain refuted. Partial-scan omissions alone are conservative under a correct floor, contrary to a current parent refinement.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 9 | 0 | 0 | 2 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Failed history.transact leaves RecordCell advanced with no rollback — prior version masked on point reads

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

With committed k::5 present, a failed write at 6 bumps the cell first; guard Drop marks 6 Aborted and advances the floor but does not restore the cell. Exact k::6 lookup returns None although k::5 remains. Direct engine CRUD callers exist. Oracle must start with an existing value and assert exact point/stream preservation after failure, including a later-operation batch fault.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:785](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L785); [crates/shamir-tx/src/mvcc_store/mod.rs:799](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L799); [crates/shamir-tx/src/mvcc_store/mod.rs:1291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L1291); [crates/shamir-tx/src/version_guard.rs:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/version_guard.rs#L107); [crates/shamir-engine/src/table/table_manager_crud.rs:218](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_crud.rs#L218).

<a id="review-2"></a>

### Claim 2 — vacuum_key scan path silently swallows prefix-scan errors

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

unwrap_or_default silently omits failed scan batches while siblings propagate errors, causing hidden incomplete reclamation. However, omission alone cannot delete an observed required snapshot anchor under a correct floor: observed versions &gt;= floor are protected; an observed true below-floor maximum remains the observed maximum; an omitted anchor is not a deletion candidate. Other floor/namespace bugs are separate.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:174](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L174); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:193](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L193); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:214](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L214); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:236](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L236).

<a id="review-3"></a>

### Claim 3 — Changefeed journal gap detection has an undetected hole on persist failures

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Failed serialization or put only warns; the writer has no gap-marker handle. Closed sends are ignored. A later successful higher-version put advances max persisted past the missing event. A failing-put then successful-put oracle must assert an explicit loss/error signal, not just maximum progress.

Evidence: [crates/shamir-tx/src/changefeed.rs:336](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L336); [crates/shamir-tx/src/changefeed.rs:557](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L557); [crates/shamir-tx/src/changefeed.rs:639](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L639); [crates/shamir-tx/src/changefeed.rs:653](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L653).

<a id="review-4"></a>

### Claim 4 — thiserror declared but never used — String errors including a public trait

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The dependency is declared without use and the identified boundaries use String. Typed library errors are maintenance debt; converting the public trait requires coordinated implementor/caller changes, not merely a drop-in derive.

Evidence: [crates/shamir-tx/Cargo.toml:23](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/Cargo.toml#L23); [crates/shamir-tx/src/changefeed.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L154); [crates/shamir-tx/src/mvcc_store/retention.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/retention.rs#L60); [crates/shamir-tx/src/tx_context.rs:942](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tx_context.rs#L942).

<a id="review-5"></a>

### Claim 5 — Error-path tests stop at two functions on fresh keys — batch-abort and drain error claims untested

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Four registered fresh-key fault tests exist, including ts_atomicity; fail_set rejects the first Set in a sequential mock. They do not prove prior-value restoration, late-operation rollback, batch watermark obligations, vectored fault propagation, or drain-error guard release. The deferral test only proves primitive no-write/Ok behavior, not engine acceptance safety.

Evidence: [crates/shamir-tx/src/tests/mvcc_store_tests/error_tests.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/error_tests.rs#L43); [crates/shamir-tx/src/tests/mvcc_store_tests/ts_atomicity_tests.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/ts_atomicity_tests.rs#L237); [crates/shamir-tx/src/tests/mvcc_store_tests/test_stores.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/test_stores.rs#L92); [crates/shamir-tx/src/tests/mvcc_store_tests/write_committed_batch_tests.rs:363](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/tests/mvcc_store_tests/write_committed_batch_tests.rs#L363).

<a id="review-6"></a>

### Claim 6 — ts_index_rebuild swallows stream errors and unconditionally marks ready

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Error batches are skipped and ready prevents retry; production lookup has no scan fallback. A healthy subsequent read can remain incomplete for the process lifetime.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L408); [crates/shamir-tx/src/mvcc_store/mod.rs:428](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L428); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L95).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — LayeredInterner::touch_sync panics on a fallible signature its sibling propagates

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The current callee never returns Err. Different caller handling is prospective resilience, not a current input-conditioned panic mechanism.

Evidence: [crates/shamir-tx/src/layered_interner.rs:84](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/layered_interner.rs#L84); [crates/shamir-types/src/core/interner/interner.rs:146](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L146); [crates/shamir-types/src/core/interner/interner.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L176).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — StagedRow::as_inner panics on decode failure though construction is unvalidated

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The panic method has no callers and no public construction/access path exposes a StagedRow. Current byte reads and remapping bypass it.

Evidence: [crates/shamir-tx/src/staging_store.rs:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/staging_store.rs#L32); [crates/shamir-tx/src/staging_store.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/staging_store.rs#L81); [crates/shamir-tx/src/staging_store.rs:165](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/staging_store.rs#L165).

Grouping/duplicate: [security-crypto.md#4](security-crypto.md#review-4). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — apply_committed_ops doc contradicts error-path ordering

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Outer prose promises history-first/no visible failed state; implementation publishes visible state before awaiting history. This combined method also has a production replication caller, contrary to the broader test/direct-only assurance.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L413); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:427](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L427); [crates/shamir-engine/src/tx/apply_replicated.rs:338](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/apply_replicated.rs#L338).

<a id="review-10"></a>

### Claim 10 — lookup_ts swallows history.get errors as unknown age with no log

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

All get errors map to None without diagnostics. Age-dependent reclaim conservatively keeps unknown-age entries, but history reporting cannot distinguish absent timestamps from storage failure.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:1634](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L1634); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:232](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L232); [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:212](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L212).

<a id="review-11"></a>

### Claim 11 — RepoChangefeed::new panics outside a Tokio runtime; writer task is detached

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

new calls spawn and drops its handle. Exact Tokio 1.49.0 spawn requires runtime context; current engine initialization is runtime-hosted. This is a constructor/lifecycle documentation issue, not an established production panic. Source: https://docs.rs/crate/tokio/1.49.0/source/src/task/spawn.rs

Evidence: [crates/shamir-tx/src/changefeed.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/changefeed.rs#L249); [crates/shamir-engine/src/repo/repo_instance.rs:1204](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L1204); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

<a id="review-summary-raii-guarantees"></a>

### Claim Summary: RAII guarantees — VersionGuard, CellReservationGuard and SnapshotGuard discharge synchronous cleanup obligations

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Their Drop operations are present and synchronous. Those narrow obligations do not ensure failed-write rollback, complete persistence, retained reservations, or semantic undo after a WAL append.

Evidence: [crates/shamir-tx/src/version_guard.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/version_guard.rs#L100); [crates/shamir-tx/src/cell_reservation_guard.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/cell_reservation_guard.rs#L102); [crates/shamir-tx/src/repo_tx_gate.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_tx_gate.rs#L234); [crates/shamir-tx/src/repo_wal_manager.rs:62](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_wal_manager.rs#L62).

## Evidence and recipe corrections

- The current refinement suggesting scan omissions alone can delete a required observed anchor is refuted under exact key isolation and a correct snapshot floor. The true maximum is either observed and retained, or omitted and not deleted.
- That conditional conservatism does not excuse silent scan errors or repair the independent min_alive and prefix-isolation defects.
- Failed-cell masking is not necessarily permanent: a later higher global floor plus cache pruning can enable cold recovery.
- Unconditional restoration can overwrite a newer writer; post-log publication can reopen the original snapshot race. Partial backend application must be modeled.
- apply_committed_ops is used by production apply_replicated, where the caller explicitly has no local WAL entry to offer the drainer. Its failure state cannot universally be described as automatically healed by background drain.
- The existing deferral oracle does not check the downstream durable watermark, overlay retention, or WAL cleanup.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tx -- Error handling & resource lifecycle

## Summary

Error-path discipline in this crate is genuinely strong in places — the `VersionGuard`/`CellReservationGuard`/`SnapshotGuard` RAII design makes abort-marking hold "by construction", and fault-injection tests exist for the single-key non-tx write paths. However, the bump-first write paths (`set_versioned`/`set_versioned_many`/`delete_versioned`) advance the in-memory `RecordCell` *before* the durable `history.transact` and perform **no compensating rollback when that transact fails**, which masks the key's prior version on every subsequent point read. Beyond that, `thiserror` is declared in `Cargo.toml` but never used (six APIs return `Result<_, String>`, including the public `ChangelogStore` trait), several GC/recovery paths silently swallow storage errors in ways inconsistent with their siblings, the changefeed's gap-detection invariant has an undetected hole on journal persist failures, and error-path test coverage stops at two functions on fresh keys. Note: `RepoTxGate::pending_commits.lock().unwrap()` (repo_tx_gate.rs:753/761) is **not** flagged — CLAUDE.md explicitly sanctions it as dead scaffolding with zero live callers.

## Findings

### 1. Failed `history.transact` leaves `RecordCell` advanced with no rollback — prior version permanently masked on point reads
**File:** `crates/shamir-tx/src/mvcc_store/mod.rs:766-832` (`set_versioned`: `publish_cell` at :785, `?` at :799); same shape at `:853-938` (`set_versioned_many`, publish loop :885, `?` :898) and `:1035-1086` (`delete_versioned`, publish :1051, `?` :1063)
**Severity:** high

**Issue:** All three non-tx write paths execute `publish_cell(key, new_v)` *before* the single durable `history.transact(...)` (deliberate — the MVCC-2 snapshot invariant requires publish-before-log). On transact failure the `?` propagates immediately; the `VersionGuard` drop correctly marks the version `Aborted` and advances the watermark, but the cell map is left at `new_v` with **no compensating restore** (verified: no rollback/revert/compensate exists anywhere in `mvcc_store/`). `publish_cell`'s A2 max-monotonic guard means nothing short of a successful rewrite of the key ever corrects it, and `prune_version_cache` only evicts cells with `version < min_alive` — a cell at the aborted (now watermark-past) version is never evicted.

**Failure scenario:**
- *Failed SET:* key `k` has committed version 5. `set_versioned(k, v6)` hits a disk-full/IO error in `transact`. Caller gets `Err`. Every later `get_current`/`get_current_bytes` on `k`: `cur_v = 6`, floor ≥ 6, so no `get_at` fallback; overlay miss; `history.get(k::6)` → `NotFound` → **`Ok(None)` — the record reads as deleted** even though version 5 is intact in the log. Meanwhile `current_stream` (which group-bys the *log*, not the cell) still emits it — point read and stream read disagree indefinitely.
- *Failed DELETE:* `delete_versioned(k)` bumps the cell to the tombstone version but the tombstone never lands. Point reads return `None` — **the delete appears to have succeeded** in-process, then the record resurrects after restart (recovery reads the log). A durability illusion.

The doc comments say "cancel-safe: NO … caller must retry or WAL-replay to converge", but this is the *error* path, not cancellation: non-tx writes have no WAL entry to replay, and a caller that surfaces the `Err` (the normal engine behavior) never retries — the divergence is permanent until the same key is rewritten.

**Suggested fix:** On `transact` failure, restore the cell before propagating: capture `old_v` per key (already captured for vacuum on `set_versioned`/`delete_versioned`), then on the error branch run an explicit `restore_cell(key, old_v)` that unconditionally sets `cell.version = old_v` (a plain `entry_sync` write — *not* `publish_cell`, whose max-monotonic guard would refuse the regression). For `set_versioned_many`, roll back all batch keys to their captured `old_versions`. Add a test with a pre-existing key (see Finding 5). Alternatively, defer the `publish_cell` loop to after transact success on the batch paths (the publish is already synchronous there), keeping the pre-log publish only where the MVCC-2 invariant demonstrably requires it.

### 2. `vacuum_key` scan path silently swallows prefix-scan stream errors — inconsistent with its siblings
**File:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:173-181` (`batch.unwrap_or_default()` at :174)
**Severity:** medium

**Issue:** The retention-aware vacuum's phase-1 scan treats every stream error as an empty batch (`for (phys_key, _val) in batch.unwrap_or_default()`). The two sibling GC paths — `gc_below` (:312) and `purge_below_ts` (:408) — both propagate with `batch?`. Analysis shows the truncation errs toward over-retention rather than data loss (deletions only target *collected* entries, and a shorter list lowers each entry's reclaim rank), so this is not a correctness hole — but under a persistent read error, vacuum silently stops reclaiming anything with **zero log output**, and the history log grows unboundedly while `gc_below` on the same store correctly reports errors. The deletions themselves being best-effort (`let _ = remove_no_flag`, documented) is fine; the *decision-input* scan being silently lossy is not.

**Failure scenario:** A store backend returns intermittent `DbError::Storage` from `scan_prefix_stream` (e.g. corrupted batch, transient IO). Every write still succeeds, but the vacuum scan collects a partial/empty entry list each time and quietly deletes nothing. No warning is logged; disk fills; the only symptom is history growth. An operator correlating with `gc_below` errors would see inconsistent behavior between two GC entry points.

**Suggested fix:** Either propagate (return early, making `vacuum_key` return `DbResult<()>` — its only callers are write paths that already return `DbResult`), or at minimum `log::warn!` on each errored batch and skip the reclaim pass for that key (a partial entry list must not drive deletions at all). Match `gc_below`/`purge_below_ts`.

### 3. Changefeed journal gap detection has an undetected hole on persist failures
**File:** `crates/shamir-tx/src/changefeed.rs:632-655` (`persist_one`); related: `journal_send` `Closed` branch :336-339
**Severity:** medium

**Issue:** CF-1's `first_gap_version` is updated **only** on `TrySendError::Full` (channel overflow, :310-334). When the background writer's `store.put()` fails (`persist_one`, :645-651), the event is dropped with a `log::warn!` and *no* gap marker — and the next successful persist advances `last_persisted_version` via `fetch_max` **past the hole**, so the CF-2 watermark also cannot expose it. `read_from` (:414-421) then returns `gap_at: None` over a journal that is silently missing a version — directly violating the module's own stated contract at :415: "Conservative over-signal is acceptable; silent omission is not." The `TrySendError::Closed` branch is even quieter: no counter, no gap marker, no log (a panicked writer task closes the channel and every subsequent commit's journal event vanishes with zero observability; CF-2 only detects this if a consumer is actively comparing watermarks).

**Failure scenario:** A replication consumer resumes from `read_from(v)` after a transient store failure dropped version `v`'s journal write. `gap_at` is `None`, events on both sides of the hole are returned, and the consumer trusts an unbroken history — missing exactly one committed transaction with no signal to trigger the documented full-snapshot resync.

**Suggested fix:** In `persist_one`'s error branch, run the same min-CAS loop on `first_gap_version` that `journal_send` uses for `Full`. For `Closed`, bump `journal_dropped` (or a dedicated counter) so a dead writer is at least countable.

### 4. `thiserror` declared but never used — six APIs return `Result<_, String>`, including a public trait
**File:** `crates/shamir-tx/Cargo.toml:23` (dependency); `changefeed.rs:154` & `:157` (`ChangelogStore::put`/`range_from`), `changefeed.rs:542` (`serialize_event`), `staging_store.rs:249` (`rewrite_set_bytes`), `tx_context.rs:913-916` (`apply_id_remap`), `mvcc_store/mod.rs:500` (`set_retention`), `mvcc_store/retention.rs:60` (`Retention::validate`)
**Severity:** medium

**Issue:** CLAUDE.md's error-handling rule is explicit: "`thiserror` for library error enums (with `#[from]` where natural)". `thiserror` is a declared dependency of this crate but a repo-wide grep finds zero uses — the crate defines no error enum of its own and instead threads `String` through six APIs. `ChangelogStore` is the worst offender: a **public trait** whose `Result<(), String>` / `Result<Vec<Bytes>, String>` shape forces stringly-typed errors onto every implementor (engine-side production store included), making error-kind matching impossible and pushing `format!`-based error construction into callers (`serialize_event`, `apply_id_remap`'s `.map_err(|e| format!("remap: {e}"))`). The others are validation/config seams where a small `thiserror` enum (`RetentionError`, `RemapError`) would let callers distinguish decode failures from policy violations.

**Suggested fix:** Introduce `#[derive(thiserror::Error)]` enums for the crate's own failure kinds and convert the six sites, starting with the public `ChangelogStore` trait (its `put`/`range_from` failures are already only ever logged — a small `ChangelogStoreError` with `#[from]` for the underlying storage error is a drop-in).

### 5. Error-path tests stop at two functions on fresh keys — the documented batch-abort and drain error claims are untested
**File:** `crates/shamir-tx/src/tests/mvcc_store_tests/error_tests.rs` (entire file); fault double at `test_stores.rs:10-108`
**Severity:** medium

**Issue:** The crate has an excellent fault-injection double (`FailingStore` with `fail_get`/`fail_remove`/`fail_set`) but it is exercised by exactly three tests, all on a **fresh key** against `set_versioned`/`delete_versioned`. Untested error paths, each carrying a documented behavioral claim that only a test can pin:
- `set_versioned_many` / `set_versioned_many_append_only` transact failure — the guard-vector comment (mod.rs:869-873) claims "every guard drops un-committed and marks its version Aborted, so the contiguous watermark advances past the whole failed batch instead of wedging at the first version". No test asserts the watermark advances or that overlay stays empty.
- No pre-existing-key failure test — which is precisely why Finding 1 (prior-version masking) is invisible to the suite: `set_versioned_propagates_archive_read_error` asserts `get_current == None` on a key that *never existed*, where `None` is the correct answer for the wrong reason.
- `write_committed_to_history` / `write_committed_batch_to_history` / `drain_to_history` `?` propagation, and the `drain_exclusive` backoff-returns-`Ok` contract (#1032) — only the deferral is tested (`write_committed_batch_tests.rs:363`), not the error path.
- Batched reads: `get_at_many`/`get_current_many` propagate `get_many` errors with `?` mid-assembly — untested (note: `FailingStore` doesn't override `get_many`, so injection through the default per-key `get` works but is never exercised).
- `vacuum_key` scan-error behavior (Finding 2) and journal persist-failure gap behavior (Finding 3) — untested.

**Suggested fix:** Extend `error_tests.rs`: (1) failed `set_versioned` on a key with a committed prior version, asserting the prior value is still readable (will currently fail — Finding 1); (2) failed `set_versioned_many`, asserting `gate.last_committed()` advances past the batch, overlay stays empty, and `durable_watermark() <= last_committed()`; (3) failed `drain_to_history` mid-version, asserting the error propagates and `drain_exclusive` is released; (4) `get_at_many` with `fail_get` armed, asserting propagation.

### 6. `ts_index_rebuild` swallows all stream errors and unconditionally marks the index ready
**File:** `crates/shamir-tx/src/mvcc_store/mod.rs:398-429` (`Err(_) => continue` at :408; `ts_index_ready.store(true, ...)` at :428)
**Severity:** low

**Issue:** The lazy rebuild treats every errored batch as skippable, then sets `ts_index_ready = true` regardless of how much was dropped. A partial rebuild is never retried (the flag gates future rebuilds forever), and the documented fallback ("falls back to the full history scan only if the index is empty after rebuild", mvcc_history.rs:80-81) does not fire for a *partially* populated index — `version_at_or_before_ts` then silently resolves as-of-ts queries to stale versions with no log line. Best-effort is acceptable for this index; permanently caching a known-bad state is not.

**Suggested fix:** Count dropped batches; if any errored, leave `ts_index_ready = false` (retry on next query) and `log::warn!` once. Alternatively track a `ts_index_degraded: AtomicBool` surfaced next to `ts_index_len()`.

### 7. `LayeredInterner::touch_sync` panics on a fallible signature that `commit_interner_overlay` propagates
**File:** `crates/shamir-tx/src/layered_interner.rs:82-86` vs `:259-263`
**Severity:** low

**Issue:** `Interner::touch_ind` returns `Result<TouchInd, &'static str>` (shamir-types interner.rs:138). The `Direct`-mode branch handles it with `.expect("Interner::touch_ind is infallible for valid input")` — on the **non-tx hot path** — while the same operation ten lines below in `commit_interner_overlay` is treated as a real, mappable error (`.map_err(|e| DbError::Codec(e.to_string()))?`). Today `touch_ind` happens to never return `Err` (all its body paths are `Ok`), so the expect holds; but the two call sites disagree about the contract, and any future `touch_ind` change that returns an error (its signature explicitly reserves that) converts the non-tx write path into a panic while the commit path degrades gracefully. CLAUDE.md permits `expect` only for genuine invariant violations; "I checked the other call site and it disagrees" means this isn't one.

**Suggested fix:** Make `touch_sync` return the `Result` (or map the error to a sentinel id / propagate as `DbError::Codec`) so both paths share one contract; if the infallibility claim is real, that belongs in a test pinning `touch_ind`'s Ok-for-all-inputs property, not an `expect` on the hot path.

### 8. `StagedRow::as_inner` panics on msgpack decode failure though nothing validates at construction
**File:** `crates/shamir-tx/src/staging_store.rs:46-49`
**Severity:** low

**Issue:** `StagedRow`'s invariant ("always holds valid msgpack") is a caller contract — `StagingStore::set`/`set_many` accept arbitrary `Bytes` with no validation, so a caller staging malformed bytes (raw put path, corrupt upstream payload) defers the failure to `as_inner`, which panics. The inconsistency: the commit-time remap of the *same bytes* (`remap_inner_value_bytes`, id_remap.rs:77-78) treats decode failure as a recoverable `Err` that aborts the tx. A malformed staged row should be an abortable error at read-your-own-write time too, not a process-killing panic (this is a server library; a panic takes down the runtime shared by every other session).

**Suggested fix:** Change `as_inner` to `Result<Cow<'_, InnerValue>, rmp_serde...Error>` (or add `try_as_inner`), or validate on `set` and return an error at staging time. Callers are few (read-your-own-write lens and commit remap).

### 9. `apply_committed_ops` doc contradicts the code's error-path ordering
**File:** `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:413-431`
**Severity:** low

**Issue:** The function doc states "Ordering: history FIRST (durable landing), then visible (overlay + cell) — matching the pre-split contract where a failed history `transact` (`?`) left no reader-visible state." The code does the **opposite**: `apply_committed_visible(&ops, commit_version)` runs first (:427), then `write_committed_to_history(...).await?` (:428). The inner comment (:417-426) correctly explains the intentional swap (pending_ts must be stamped before the drain half consumes it) and admits "a history error propagates via `?` (the cell/overlay are then ahead …)" — so on this path a failed history write *does* leave reader-visible state. The stale outer paragraph is exactly what a future maintainer auditing error-path state will read first, and it describes the opposite guarantee.

**Suggested fix:** Rewrite the doc's ordering paragraph to match the code: visible-first (ts stamp) → history `?` → on error the cell/overlay are intentionally ahead, mirroring the production ack-path until the drainer catches up.

### 10. `lookup_ts` swallows all `history.get` errors as "unknown age" with no log
**File:** `crates/shamir-tx/src/mvcc_store/mod.rs:1624-1636`
**Severity:** nit

**Issue:** `Err(_) => None` is the conservative direction (unknown-ts versions are KEPT by vacuum/purge), so it is safe — but a persistent storage error silently disables the entire age-retention axis and, in `history_of`, silently degrades every `ts_millis` to `None`. A single `log::debug!`/`warn!` (rate-limited) on the error arm would make "age retention quietly stopped working" diagnosable.

### 11. `RepoChangefeed::new` panics outside a tokio runtime; writer task is detached
**File:** `crates/shamir-tx/src/changefeed.rs:249-255`
**Severity:** nit

**Issue:** `tokio::spawn(journal_writer_loop(...))` panics if called off-runtime (e.g. from a `new()` during engine construction before the runtime exists), and the returned `JoinHandle` is discarded so a writer panic is observable only via the CF-2 watermark stall. Engine-side call sites are runtime-hosted today; a `#[track_caller]`-documented contract or a builder that takes the runtime's handle would remove the foot-gun.

</details>
