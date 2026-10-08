<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-engine — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The named original mechanisms are mostly removed. Pre-read error swallowing remains; registered tests provide specific oracles, not blanket acceptance. Additional semantic regressions appear in optimized membership paths.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 1 | 8 | 0 | 0 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — StoreChangelog::range_from streams the ENTIRE journal tail before applying limit

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

The loop stops at limit; the registered counting Store test would detect whole-tail consumption.

Evidence: [crates/shamir-engine/src/repo/changelog_store.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/changelog_store.rs#L54); [crates/shamir-engine/src/repo/tests/changelog_store_tests.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/tests/changelog_store_tests.rs#L113).

<a id="review-2"></a>

### Claim 2 — Drainer Phase B silently drops Put/Delete for tables with no MvccStore — and its justification comment is false

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Retained tables resolve to history/raw writes; failures enter failed_tables and block contiguous finalization. The registered missing-map test checks the recovered value.

Evidence: [crates/shamir-engine/src/tx/drainer.rs:599](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L599); [crates/shamir-engine/src/tx/drainer.rs:742](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L742); [crates/shamir-engine/src/tx/tests/drainer_tests.rs:371](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/tests/drainer_tests.rs#L371).

<a id="review-3"></a>

### Claim 3 — .ok() on write-path pre-reads swallows real I/O errors and mis-plans index/counter ops

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

All three .ok conversions remain. A genuine pre-read failure selects no-op deletion or insert-shaped index/counter planning. These are low-level public API witnesses, not established wire-path exploits.

Evidence: [crates/shamir-engine/src/table/table_manager_crud.rs:457](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_crud.rs#L457); [crates/shamir-engine/src/table/table_manager_crud.rs:537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_crud.rs#L537); [crates/shamir-engine/src/table/table_manager_tx_ops.rs:1106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_tx_ops.rs#L1106).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — GroupCommit: a panicking leader strands leader_busy = true forever

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Closure invocation and polling are inside catch_unwind; waiters settle and leadership resets. The registered test additionally requires a successful subsequent flush.

Evidence: [crates/shamir-engine/src/repo/group_commit/group_commit.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/group_commit/group_commit.rs#L137); [crates/shamir-engine/src/repo/group_commit/group_commit.rs:161](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/group_commit/group_commit.rs#L161); [crates/shamir-engine/src/repo/group_commit/tests/panicking_flush_tests.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/group_commit/tests/panicking_flush_tests.rs#L22).

<a id="review-5"></a>

### Claim 5 — rederive_stale_value_ops_post_stage is quadratic in staged ops (O(R×W) per commit)

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Dedup structures are built before the row loop and updated after appended operations, preserving owner/family distinctions.

Evidence: [crates/shamir-engine/src/tx/pre_commit.rs:2062](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/pre_commit.rs#L2062); [crates/shamir-engine/src/tx/pre_commit.rs:2279](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/pre_commit.rs#L2279); [crates/shamir-engine/src/tx/pre_commit.rs:2350](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/pre_commit.rs#L2350).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — Stale doc asserts a footprint-ordering invariant the AsyncIndex path no longer has

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Documentation now agrees with footprint-before-publish source ordering.

Evidence: [crates/shamir-engine/src/tx/finalize.rs:21](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/finalize.rs#L21); [crates/shamir-engine/src/tx/commit.rs:765](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L765); [crates/shamir-engine/src/tx/commit.rs:772](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L772).

<a id="review-7"></a>

### Claim 7 — Dead do-nothing loop and by-construction unwrap()s in SessionPermissions

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

The empty loop is removed and resource extraction is factored; default production excludes the scaffolding. Original enum-protected unwraps were not arbitrary-input panics.

Evidence: [crates/shamir-engine/src/query/auth/session.rs:166](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/session.rs#L166); [crates/shamir-engine/src/query/auth/session.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/session.rs#L231); [crates/shamir-engine/src/query/auth/mod.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/mod.rs#L19).

Grouping/duplicate: [security-crypto.md#7](security-crypto.md#review-7). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Two implementation files embed inline cfg(test) mod tests

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Both relocated ordinary test files are registered; the separate loom model remains feature-coupled.

Evidence: [crates/shamir-engine/src/table/tests/mod.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/tests/mod.rs#L98); [crates/shamir-engine/src/query/read/tests/mod.rs:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/tests/mod.rs#L4).

Grouping/duplicate: [style-claude-md.md#3](style-claude-md.md#review-3). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — RecordCounter::persist dirty-flag race can drop the last persist trigger

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

last_persisted records the value actually written, so a racing increment remains distinguishable. The pause-seam test checks a second durable flush.

Evidence: [crates/shamir-engine/src/table/record_counter.rs:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/record_counter.rs#L180); [crates/shamir-engine/src/table/record_counter.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/record_counter.rs#L187); [crates/shamir-engine/src/table/tests/record_counter_tests.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/tests/record_counter_tests.rs#L374).

Grouping/duplicate: [concurrency-lockfree.md#3](concurrency-lockfree.md#review-3). This is not an additional independent defect.

<a id="review-coverage-verdict-tdd-lens"></a>

### Claim Coverage verdict (TDD lens) — No vacuous tests; the remaining gaps align exactly with the findings

Status: `unverified`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Selected registered assertions are discriminating, but two regression files are orphaned and the new numeric-membership witnesses lack matching oracles. No universal test conclusion follows.

Evidence: [crates/shamir-engine/src/table/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/tests/mod.rs#L1); [crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs:550](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs#L550); [crates/shamir-engine/src/validator/schema/tests/schema_validator_tests.rs:210](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/schema/tests/schema_validator_tests.rs#L210).

## Evidence and recipe corrections

- The missing-map drainer test retains the table and removes only its MVCC map entry. Actual DROP also removes catalog/token registrations; intentionally discarded table contents are not proof of lost retained data.
- The ordinary pre-read fault injector does not cover TableManager::get. Registering the orphan file alone would not make its delete/set fixtures valid.
- The historical counter recipe that rereads cache and stores that reread as last_persisted is unsafe: it can label an unwritten increment persisted. Record the value actually written.
- Original clone-removal findings can remain source-fixed while newly introduced semantic regressions remain open; source-fixed is not a universal correctness label.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-engine -- Correctness & TDD-coverage

## Summary

The crate's commit pipeline, drainer, and write paths are unusually well-defended (issue-numbered regression tests, deterministic pause seams instead of sleeps, self-flagged vacuous tests), and no outright vacuous test was found in the current suite. The genuine defects found are concentrated in edge-of-path error handling and contract seams: `.ok()` on pre-reads conflates I/O failure with "record absent" and mis-plans index/counter ops; the drainer's warm path silently drops data ops for tables without an `MvccStore` (justified by a factually wrong comment, untested); `StoreChangelog::range_from` ignores its `limit` while iterating (unbounded memory); and `GroupCommit` strands leadership forever if the detached leader panics. Two smaller convention violations against CLAUDE.md (stale ordering-invariant doc in `finalize.rs`; two inline `#[cfg(test)] mod tests` blocks) round out the list.

## Findings

### 1. `StoreChangelog::range_from` streams the ENTIRE journal tail before applying `limit`
- **File:** `crates/shamir-engine/src/repo/changelog_store.rs:37-55`
- **Severity:** high
- **Issue:** The `while let Some(chunk) = stream.next().await { pairs.extend(chunk); }` loop consumes every key `>= from_key` into a `Vec<(RecordKey, Bytes)>`, sorts it, and only then truncates to `limit`. The `batch = limit.clamp(1, 1024)` value only sizes stream chunks — it never bounds total accumulation. This is the durable backend for `RepoInstance::read_changelog_from` (follower replication catch-up and late-subscriber resync).
- **Failure scenario:** A repo with millions of committed events whose changelog is tailed with `limit = 10` loads and sorts every remaining event into RAM per call — O(N) memory and O(N log N) CPU where O(limit) was promised. Under a replication loop this is sustained unbounded allocation (OOM / DoS), and it directly violates CLAUDE.md pillar 3 ("avoid hidden O(N) in helpers"; "drive per-op asymptotic cost toward constant").
- **Suggested fix:** Break out of the drain loop as soon as `pairs.len() >= limit + batch` (slack for the defensive in-window sort), or take only the first `limit` pairs from the sorted stream batch-wise. Add a regression test asserting `range_from(from, 1)` on a large seeded journal performs bounded reads (e.g. via a counting Store double).

### 2. Drainer Phase B silently drops Put/Delete for tables with no `MvccStore` — and its justification comment is false
- **File:** `crates/shamir-engine/src/tx/drainer.rs:419-437` (Phase A) and `:504-519` (Phase B)
- **Severity:** high (narrow reachability, but silent permanent data loss plus a load-bearing wrong comment)
- **Issue:** Phase A's op loop explicitly skips `WalOpV2::Put | Delete` for ALL tables ("Data ops: handled below via batch accumulation") — `replay_v2_op` is never called with data ops on the warm path. Phase B then writes history only for tables found in `per_table_mvcc`; for a table with no `MvccStore` it does nothing, with the comment *"data ops were already handled by replay_v2_op in Phase A (which skips Put/Delete for MVCC tables)"* — a description of behavior that does not exist. Cold recovery (`recover_inflight_v2` → `replay_v2_op`, recovery.rs:80-117) DOES write `data_store` for unattached tables, so the cold and warm paths diverge.
- **Failure scenario:** The ack path's `apply_data_batch` writes unattached (non-MVCC) tables via `base.transact` inline; on persistent failure the tx is reported `MaterializationState::Deferred` with the documented contract "recovery / drainer will reconcile" (`tx_outcome.rs`, `materialize.rs`). The drainer then finalizes the entry anyway — `gate.mark_durable(v)`, and `wal.commit(txn_id)` if A5-safe — without ever re-applying the failed table's data ops. If the WAL marker is truncated, the sole durable copy is destroyed: the reconciliation the client was promised cannot happen even after the storage fault heals. Reachability is narrow (unattached/system tables, or a table whose `per_table_mvcc` entry was removed by a concurrent `remove_table` mid-tx), which is exactly why no test catches it: every fixture in `tx/tests/drainer_tests.rs` goes through `repo.get_table(...)`, which attaches an `MvccStore` in `create_table_context`.
- **Suggested fix:** In Phase B, when `read_sync(table_id, ...)` returns `None`, route the batch's ops through `replay_v2_op` (which already implements the non-MVCC `data_store` writes and NotFound-tolerant deletes), or record the table in `failed_tables` so Phase C stops finalization. Correct the Phase B comment. Add a `drain_step` test with a hand-built entry whose `Put` targets a token absent from `per_table_mvcc`, asserting the data lands in `data_store` (or that the entry is NOT marked durable).

### 3. `.ok()` on write-path pre-reads swallows real I/O errors and mis-plans index/counter ops
- **File:** `crates/shamir-engine/src/table/table_manager_crud.rs:431` (`delete_returning_version`), `:511` (`set_returning_version`); `crates/shamir-engine/src/table/table_manager_tx_ops.rs:1075` (`update_tx`)
- **Severity:** medium
- **Issue:** All three sites flatten `Err` to "record absent" via `.ok()`. `get`/`read_one_tx` return `Err(NotFound)` for genuinely absent records but `Err(io)` for real failures, so a transient storage error is indistinguishable from absence. Consequences per site: `delete` returns `(false, 0)` and silently skips the MVCC delete + index cleanup for an existing record; `set`/`update_tx` take the insert branch — `counter_delta = +1` (drift), `plan_insert_ops`/`on_record_created` instead of update ops, so the old value's postings are never removed (dangling postings / stale unique keys). For `update_tx` (the transactional path) the commit-time `rederive_stale_value_ops_post_stage` repair only fires when the repo-busy gate passes (`version_allocation_high_water_mark() > snapshot_version`, pre_commit.rs:1969-1977) — a quiet repo commits the wrong plan. This contradicts CLAUDE.md's error-handling rules (don't swallow; propagate via `?`) and is asymmetric with the sibling `delete_tx`/`read_one_tx_bytes` path, which F-65 (#891) deliberately hardened to fail-closed on exactly this class.
- **Failure scenario:** A transient backend error during the pre-read of `update_tx`/`set` on an existing row → the committed WAL entry carries insert-shaped index ops; the row's previous unique posting is never released at Phase 5c while the data row is overwritten — index/data divergence until doctor `repair()`.
- **Suggested fix:** Replace `.ok()` with a `match` that maps `Err(DbError::NotFound(_))` to `None` and propagates every other error. Add Red tests (per CLAUDE.md Red/Green/Refactor) using the existing fault-injection seam pattern (cf. `TEST_READ_ONE_TX_BYTES_FAILURE`) asserting a read failure aborts the op instead of taking the insert branch.

### 4. `GroupCommit`: a panicking leader strands `leader_busy = true` forever
- **File:** `crates/shamir-engine/src/repo/group_commit/mod.rs:48-117`
- **Severity:** medium
- **Issue:** The detached `leader_loop` task (added to fix the cancellation DoS, audit §2.1) resets `leader_busy = false` only on its normal exit path. If `flush()` panics, the task aborts without unwinding to the reset: the current batch's waiters correctly get `Err("group-commit flush task dropped")` from their dropped oneshots, but `leader_busy` stays `true`, so every subsequent `run()` caller pushes its oneshot and parks forever — all future `synced_flush` calls on that repo hang (durability-flush DoS, the same class the cancellation fix aimed to eliminate).
- **Failure scenario:** Any panic inside the `flush` closure (`repo.flush_buffers()` — e.g. a poisoned lock or an `unwrap` deep in a backend) converts one failed flush into a permanent hang of every later `synced` commit on the repo.
- **Suggested fix:** Catch unwind around `flush().await` (or guard the leadership with an RAII struct whose `Drop` resets `leader_busy` under the state lock), propagating the panic message to the batch as `Err`. Add a test with a panicking flush asserting a subsequent `run()` still completes.

### 5. `rederive_stale_value_ops_post_stage` is quadratic in staged ops (O(R×W) per commit)
- **File:** `crates/shamir-engine/src/tx/pre_commit.rs:2012-2030` (DELETE case) and `:2204-2300` (UPDATE case)
- **Severity:** medium
- **Issue:** The DELETE case rebuilds `staged_removals_by_rid` — a full scan of `tx.index_write_set` — inside the per-staged-op loop; the UPDATE case runs a full `tx.index_write_set` scan per candidate op for dedup. A batch DELETE/UPDATE of R rows with W total staged index ops costs O(R×W) on the commit critical path. This is precisely the hidden O(N²) CLAUDE.md pillar 3 bans, and the same defect class this crate already fixed twice nearby (#1099, #1108 — `released_unique_cache` was made incremental for exactly this reason). The repo-busy gate means the cost fires only under concurrent commit traffic — i.e. under load, where it hurts most.
- **Failure scenario:** A 10k-row batched DELETE on an indexed table with a concurrent committer: ~10k × |index_write_set| scans per commit; latency cliff and CPU saturation exactly when the gate opens.
- **Suggested fix:** Hoist the per-rid removal index and the dedup key-set out of the loops, building each once per `table_token` before iterating staged ops (mirroring `refresh_released_unique_cache`'s incremental shape). Assert via the existing `p1107_stale_value_gate` bench that ns/op does not regress super-linearly with batch size.

### 6. Stale doc asserts a footprint-ordering invariant the AsyncIndex path no longer has
- **File:** `crates/shamir-engine/src/tx/finalize.rs:21-26` vs `crates/shamir-engine/src/tx/commit.rs:731-751`
- **Severity:** low
- **Issue:** `finalize.rs` justifies NOT unifying the AsyncIndex tail by claiming its SSI footprint (`record_commit_writes`) runs AFTER `version_guard.commit()`. The code records the footprint strictly BEFORE publish (commit.rs:744 before :751, with its own F-28/S3-C comment explaining the missed-phantom window this order closes). Divergence axis 1 of the three listed is therefore false. A future refactor trusting this doc either preserves a phantom constraint or — worse — re-orders footprint-after-publish on some path, reintroducing the real window the ordering exists to close.
- **Suggested fix:** Update the doc to the current order (and re-evaluate whether the remaining two axes still justify the duplication). Doc-only, but on a concurrency-critical ordering it should be corrected before it misleads.

### 7. Dead do-nothing loop and by-construction `unwrap()`s in `SessionPermissions`
- **File:** `crates/shamir-engine/src/query/auth/session.rs:162-170` (dead loop), `:238-254` (`op.table_ref().unwrap()`)
- **Severity:** low
- **Issue:** `row_filter`'s first `for` loop has an empty body and a trailing comment "We need a different approach" — a leftover of an abandoned design that reads as if it does something. In the same file, `extract_action_resource` unwraps `op.table_ref()` for all five data-op variants; safe today only because every `BatchOp::Read/Insert/Update/Delete/Set` construction carries a table ref — an invariant not asserted anywhere. Mitigating: the module header documents this whole RBAC path as test-only scaffolding (live access control is Shomer DAC), so production impact is nil.
- **Suggested fix:** Delete the dead loop; replace the unwraps with `debug_assert!` + fallback resource (or return `Resource::Global`-deny) so a future op variant cannot panic the permission check.

### 8. Two implementation files embed inline `#[cfg(test)] mod tests` (documented-layout violation)
- **File:** `crates/shamir-engine/src/table/writer_drain_barrier.rs:410-474`; `crates/shamir-engine/src/query/read/hashable_query_value.rs:250+`
- **Severity:** low
- **Issue:** CLAUDE.md §"Test organisation" rule 5: "Never embed `#[cfg(test)] mod tests { ... }` inline inside an implementation file. Move them to the `tests/` directory." These two files still carry inline test modules (the rest of the crate — ~40 module `tests/` dirs — follows the manifest-only convention meticulously, and CLAUDE.md's imports-exception note says such blocks are "being migrated").
- **Suggested fix:** Move the four `WriterDrainBarrier` tests to `table/tests/writer_drain_barrier_tests.rs` and the `HashableQueryValue` tests to `query/read/tests/hashable_query_value_tests.rs`, registering both in the respective manifest `mod.rs`.

### 9. `RecordCounter::persist` dirty-flag race can drop the last persist trigger
- **File:** `crates/shamir-engine/src/table/record_counter.rs:143-163`
- **Severity:** nit
- **Issue:** Between the persisting task's `cache.load()` (or its `write_through`) and its `dirty.store(false)`, a concurrent `increment` can bump the cache and set `dirty = true`; the persisting task then clears the flag anyway. The new value is left unpersisted with `dirty == false`, so a quiescent table's final increment never flushes (bounded drift, repaired by the next mutation or the doctor's full-scan reconcile; the counter is documented as a metric).
- **Suggested fix:** Clear `dirty` with a compare-exchange conditioned on the persisted value (or re-check `cache != last_persisted` after the store), so a racing increment re-arms the flag.

---

**Coverage verdict (TDD lens).** No vacuous tests were found in the current suite; the discipline is demonstrably strong (issue-keyed regression tests, one-shot pause seams with `reached`/`armed` handshakes, `nextest`-process-isolation rationale, and honest self-corrections where a review found a tautological assertion — the F-84 loom-model fixup and the #1003 vacuous-hook fixup are documented in-code). The gaps that remain align exactly with the findings above: no test drives `drain_step` with a data op against a table lacking an `MvccStore` (Finding 2), no test covers a panicking group-commit leader (Finding 4), and no test asserts that a transient pre-read error on `set`/`update_tx`/`delete` surfaces as an error rather than an insert-shaped plan (Finding 3) — each would be a natural Red test under CLAUDE.md's Red/Green/Refactor protocol.

</details>
