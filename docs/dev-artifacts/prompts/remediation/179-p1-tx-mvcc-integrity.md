# P1 stage F7 — p1-tx-mvcc-integrity

## Goal

MVCC drain durability, GC, locks, ts-index, SSI reservation. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F7): A single-table forced drain never marks a multi-table version durable; GC deletes without materialising whole history; a reentrant read never downgrades an Exclusive lock; the ts index keeps its largest-version contract; cache pruning never evicts a cell with a live SSI reservation; index rebuild and vacuum propagate scan errors.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TX-E5E0CC50 — Single-table forced drain falsely marks a multi-table transaction's shared version durable

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-tx/src/mvcc_store/drain.rs:138; crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:130; changed by: none
- R0 note (includes the fix site): OBSERVED: forced drain still marks each single-table version globally durable, and background replay starts strictly above the shared durable watermark. Neither the per-table drain lock nor sparse completion tracking accounts for another table sharing V. Fix site: crates/shamir-tx/src/mvcc_store/drain.rs must stop treating table-local completion as repository-wide version completion.

### RV-TX-46CC0BD3 — gc_below and purge_below_ts materialise history before deleting

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:341; crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:437; changed by: none
- R0 note (includes the fix site): OBSERVED: gc_below still accumulates all below-threshold versions and purge_below_ts all decoded versions into unbounded per-key vectors before deletion; the new batching affects vacuum_key only. Fix site: crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs needs bounded-memory global reclamation; high remains scoped to sufficiently large histories, not a measured RSS threshold.

### RV-TX-709D5F0D — Reentrant pessimistic read silently downgrades an Exclusive lock before commit

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:135; crates/shamir-engine/src/table/table_manager/accessors/table_manager_streaming.rs:645; changed by: none
- R0 note (includes the fix site): OBSERVED: same-owner Shared reacquisition remains compatible and unconditionally overwrites Exclusive mode; production transactional reads acquire Shared before consulting staged writes, so write-then-read still reaches the downgrade. Fix site: crates/shamir-tx/src/mvcc_store/mvcc_locks.rs must retain the strongest held mode; this establishes lost exclusion, not dirty reads or durable corruption.

### RV-TX-B40374FD — Timestamp index violates its largest-version contract when commit timestamps decrease

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/mvcc_store/store.rs:358; crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:98; changed by: none
- R0 note (includes the fix site): OBSERVED: the documented largest-version query still takes the first timestamp-major index entry, without enforcing monotonic timestamps. The supplied v1:2000, v2:1000, target:2500 witness selects v1 rather than the specified v2. Fix site: crates/shamir-tx/src/mvcc_store/store.rs must implement version-max selection under the timestamp bound.

### RV-TX-ACFF61D3 — prune_version_cache can evict a cell holding a live SSI reservation, breaking exactly one committer wins

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:563; crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:363; changed by: none
- R0 note (includes the fix site): OBSERVED: pruning retains cells solely by version, while a successful fresh reservation inserts version zero with nonzero reserved_by; a positive floor therefore removes the reservation. Serializable engine commit locking still limits the simultaneous-committer claim to the primitive scope. Fix site: crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs must preserve live reservations during pruning.

### RV-TX-E59FB9AC — ts_index_rebuild marks the index ready even when the history scan errored

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/mvcc_store/store.rs:382; crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:102; changed by: none
- R0 note (includes the fix site): OBSERVED: rebuild continues after scan errors and unconditionally publishes ready; subsequent queries use that index without a production scan fallback or rebuild retry. Fix site: crates/shamir-tx/src/mvcc_store/store.rs must avoid publishing successful readiness after an incomplete scan and expose the failure appropriately.

### RV-TX-FB689AD7 — vacuum_key scan path silently swallows prefix-scan errors

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:179; crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:204; changed by: none
- R0 note (includes the fix site): OBSERVED: per-key vacuum still replaces failed prefix-scan batches with empty batches; batching the later removals did not change that behavior. This hides incomplete reclamation, but omission alone does not establish deletion of a required anchor under a correct floor. Fix site: crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs must explicitly report or handle failed scan batches.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
