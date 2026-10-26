# P1 stage F5 — p1-wal-segment-lifecycle

## Goal

Segment names, reopen watermark, rotate-after-poison, coverage. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F5): Only canonical segment names are accepted; reopen restores the cross-segment commit maximum; a failed next-segment open after poison is reported and recoverable; the mid-flight append-failure rotate/retry path and the startup PermissionDenied branch have discriminating tests.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-WAL-AE1CA19C — Segment-name parser accepts non-canonical names and can silently shadow WAL data

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment_set/segment_set.rs:80; crates/shamir-wal/src/segment_set/segment_set.rs:140; changed by: none
- R0 note (includes the fix site): OBSERVED: Discovery still accepts arbitrary numeric stems, retains duplicate sequence numbers and reconstructs canonical paths instead of preserving discovered names. INFERRED: aliases such as 5.wal can be ignored or make the canonical path both sealed and active; WMT2 does not canonicalize discovery. Fix site: validate canonical names and reject sequence aliases in crates/shamir-wal/src/segment_set/segment_set.rs.

### RV-WAL-48749F97 — Reopen resets the active commit maximum rather than restoring the documented cross-segment watermark

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/segment/wal_segment.rs:189; crates/shamir-wal/src/segment_set/segment_set.rs:649; changed by: none
- R0 note (includes the fix site): OBSERVED: A reopened sidecar-free active segment still starts with maximum zero, and repair updates only byte length; the aggregate maximum includes that zero without reconstructing retained active versions. WMT2 highest-segment sealing does not cover an ordinary unsealed active segment. Fix site: restore its retained commit maximum during open in crates/shamir-wal/src/segment_set/segment_set.rs.

### RV-WAL-8077DA84 — rotate_after_poison when opening the next segment fails

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment_set/segment_set.rs:345; crates/shamir-wal/src/tests/segment_set/sidecar_seal_tests.rs:79; changed by: none
- R0 note (includes the fix site): OBSERVED: Poison rotation still opens the replacement before publishing state and propagates failure with the poisoned active unchanged; the new replacement-open failure test exercises healthy seal_and_rotate, not rotate_after_poison. The missing poison-specific failure oracle remains. Fix site: add replacement-open failure and subsequent recovery assertions under crates/shamir-wal/src/tests/segment_set/.

### RV-WAL-99F291CE — §1.3 file-sink mid-flight append failure → rotate → retry-once path is untested

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment/wal_segment.rs:247; crates/shamir-wal/src/tests/wal_segment/wal_segment_poison_tests.rs:59; changed by: none
- R0 note (includes the fix site): OBSERVED: File fixtures still inject torn bytes manually or mark poison; new fault seams cover memory append refusal and seal fsync failure, not a partial file write through rollback, rotation and retry. Fix site: introduce a discriminating partial-write failure seam at write_all in crates/shamir-wal/src/segment/wal_segment.rs and exercise the complete path.

### RV-WAL-0375857F — §2.4 startup PermissionDenied hard-fail branch has zero coverage

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-wal/src/tests/wal_segment/wal_segment_tests.rs:271; crates/shamir-wal/src/segment/wal_segment.rs:497; changed by: none
- R0 note (includes the fix site): OBSERVED: Persistent-denial coverage distinguishes ordinary startup replay from tolerant replay, but sealed startup replay still has only healthy-file dispatch coverage; the denial fixture also skips privileged Unix runs. Fix site: extend crates/shamir-wal/src/tests/wal_segment/wal_segment_tests.rs with a denial-discriminating sealed-startup oracle.

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
