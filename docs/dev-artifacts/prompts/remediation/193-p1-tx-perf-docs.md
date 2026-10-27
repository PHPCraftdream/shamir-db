# P1 stage F8 — p1-tx-perf-docs

## Goal

Tx gate hot-path scans, typed errors, stale docs. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F8): `min_alive` and conflict recording are not linear on the commit path; the posting-key layout has one source; errors are typed (thiserror); separator invariant enforced; the listed stale docs are corrected.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TX-3095C5C9 — min_alive: full-map iteration on the write tail and GC paths

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/gate/repo_tx_gate.rs:825; crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:167; changed by: none
- R0 note (includes the fix site): OBSERVED: absent an in-flight opener, min_alive still traverses every distinct registered snapshot version, and scan-path write-tail vacuum still calls it; new failed-write fencing does not change this mechanism. Fix site: crates/shamir-tx/src/gate/repo_tx_gate.rs needs a maintained minimum structure or equivalent bounded lookup; reader-version deduplication limits the actual population.

### RV-TX-06ED4CDB — record_conflicts: linear interval scan over already-sorted keys inside commit critical section

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/gate/repo_tx_gate/progress.rs:453; crates/shamir-tx/src/gate/repo_tx_gate/progress.rs:528; changed by: none
- R0 note (includes the fix site): OBSERVED: footprint construction sorts posting vectors, but nested commit-window/dependency validation still calls iter().any over each vector under the documented Serializable commit-lock contract. Fix site: crates/shamir-tx/src/gate/repo_tx_gate/progress.rs needs bounded interval search intersected with the requested index prefix; latency impact is unmeasured.

### RV-TX-52FB938F — SORTED_TAG posting-key layout duplicated across crates with an illusory test pin

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-tx/src/gate/predicate_set.rs:159; crates/shamir-tx/src/tests/gate_tests/predicate_set_tests.rs:195; changed by: none
- R0 note (includes the fix site): OBSERVED: SORTED_TAG and posting-layout assumptions remain local duplicates of the index crate, and the named cross-crate pin still only compares the tx constant with literal 0x80; current layouts agree and engine manager-bound round trips provide some drift detection. Fix site: crates/shamir-tx/src/tests/gate_tests/predicate_set_tests.rs needs a genuine cross-crate contract oracle.

### RV-TX-FF6AB063 — thiserror declared but never used — String errors including a public trait

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-tx/Cargo.toml:23; crates/shamir-tx/src/changefeed/feed/core.rs:29; changed by: none
- R0 note (includes the fix site): OBSERVED: thiserror remains declared without source usage, and public ChangelogStore methods still return String errors. This remains typed-error maintenance debt, not demonstrated runtime corruption. Fix site: crates/shamir-tx/src/changefeed/feed/core.rs is the public error boundary requiring coordinated caller and implementor migration.

### RV-TX-1480F306 — Separator invariant is doc-only, probability wrong, and vacuum scan uses the wrong cur_v for prefix-matched keys

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-tx/src/version/version_codec.rs:20; crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:186; changed by: none
- R0 note (includes the fix site): OBSERVED: unsupported separator/probability prose remains, and scan vacuum still discards decoded original keys before applying the requested key's cur_v and retention guards; batched deletion does not add namespace filtering. Fix sites: crates/shamir-tx/src/version/version_codec.rs needs accurate grammar documentation and crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs needs exact-original-key selection.

### RV-TX-33897BD6 — Stale cannot early-return comment on validate_read_set iter_sync

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-tx/src/tx/tx_context.rs:828; crates/shamir-tx/src/tx/tx_context.rs:834; changed by: none
- R0 note (includes the fix site): OBSERVED: the comment still describes a scan visitor that cannot early-return, while the actual iter_sync callback returns false once a conflict is recorded; intervening vector-ticket fields do not alter validation. Fix site: crates/shamir-tx/src/tx/tx_context.rs must describe the current short-circuit visitor.

### RV-TX-95E562E9 — Stale doc: predicate_conflicts claims non-tx writes never reach last_committed

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-tx/src/gate/repo_tx_gate/progress.rs:70; crates/shamir-tx/src/mvcc_store/store.rs:628; changed by: none
- R0 note (includes the fix site): OBSERVED: the pending-footprint rationale still implies non-tx writes remain outside last_committed because they do not call publish_committed, while their successful guard.commit advances that same floor through the completion watermark. Fix site: crates/shamir-tx/src/gate/repo_tx_gate/progress.rs must describe the actual publication ordering rather than the obsolete method-based rationale.

### RV-TX-2437D088 — apply_committed_ops doc contradicts error-path ordering

- R0 verdict: `partially-fixed`, risk `nit`; evidence: crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:438; crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:447; changed by: none
- R0 note (includes the fix site): OBSERVED: outer prose correctly says visible-first and history errors leave overlay/cell ahead, but the inner comment still says both halves are synchronous and carry no durability window immediately before an awaited history write. Fix site: crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs must remove that contradictory inner assurance.

### RV-TX-10BB4C96 — Stale and self-contradictory doc comments

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-tx/src/lib.rs:10; crates/shamir-tx/src/mvcc_store/history/version_entry.rs:31; changed by: none
- R0 note (includes the fix site): OBSERVED: Stage 2 is still labelled both in progress and complete, and VersionEntry still links to record_ts despite no current definition; intervening changes only add exports and unrelated implementation changes. Fix sites: crates/shamir-tx/src/lib.rs and crates/shamir-tx/src/mvcc_store/history/version_entry.rs retain the stale documentation.

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
