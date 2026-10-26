# P1 stage F4 — p1-wal-sealed-replay

## Goal

Sealed-segment replay fails loud on every corruption shape. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F4): A sealed segment is read completely or fails loud for short suffix, oversized length, CRC-bad complete frame and undecodable frame; replay opens sealed paths read-only and never creates files; append rejects payloads that replay cannot decode; the active torn-tail policy is unchanged.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-WAL-5A825F98 — Sealed corrupted-length or incomplete-tail recovery silently discards the remaining records

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment/wal_segment.rs:555; crates/shamir-wal/src/segment_set/segment_set.rs:164; changed by: none
- R0 note (includes the fix site): OBSERVED: Replay exits successfully on a short suffix or frame end beyond the buffer before consulting sealed; absent-sidecar maximum discovery still uses this replay result. INFERRED: a corrupted sealed length can hide the remaining records and understate its maximum. Fix site: reject sealed premature EOF and incomplete headers in crates/shamir-wal/src/segment/wal_segment.rs.

### RV-WAL-6B27BE88 — repair_torn_tail silently amputates the valid suffix on a complete-but-CRC-bad frame, mislabeled as torn tail

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment/wal_segment.rs:402; crates/shamir-wal/src/segment_set/segment_set.rs:205; changed by: none
- R0 note (includes the fix site): OBSERVED: Sidecar-free active startup still runs repair before replay; a complete CRC-bad frame breaks the scan and reaches set_len at that frame's start, deleting any following bytes under a torn-tail warning. WMT2 protects sealed highest segments, not this active case. Fix site: distinguish complete corruption from incomplete tails in crates/shamir-wal/src/segment/wal_segment.rs.

### RV-WAL-22B8D9CA — Sealed CRC loud error through SegmentSet::open without a sidecar

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment_set/segment_set.rs:163; crates/shamir-wal/src/tests/segment_set/segment_set_tests.rs:606; changed by: none
- R0 note (includes the fix site): OBSERVED: Absent-sidecar startup still propagates sealed replay errors, but its fixture uses healthy data; the corrupt-data fixture retains a valid sidecar, and the new sidecar tests do not combine these conditions. Fix site: add the absent-sidecar plus sealed-payload-CRC corruption oracle in crates/shamir-wal/src/tests/segment_set/segment_set_tests.rs.

### RV-WAL-707EFC66 — SegmentSet::replay fronts sealed paths with a create-mode open — defeats documented tolerance and side-effect-creates files on a read path

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wal/src/segment_set/segment_set.rs:488; crates/shamir-wal/src/segment/wal_segment.rs:161; changed by: none
- R0 note (includes the fix site): OBSERVED: Sealed replay still begins with create-enabled append open, before replay's missing-file handling; live drainer gap recovery remains a caller despite the startup-only prose. INFERRED: an unlinked snapshot path can be recreated empty, or an initial denied open can bypass reader handling. Fix site: use a non-creating sealed-read path in crates/shamir-wal/src/segment_set/segment_set.rs.

### RV-WAL-9CB60C5A — A single CRC-valid-but-undecodable frame aborts the entire recovery (version skew == corruption)

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/entry/wal_entry_v2.rs:145; crates/shamir-wal/src/entry/wal_entry_v2.rs:192; changed by: none
- R0 note (includes the fix site): OBSERVED: Pre-HIGH-5 compatibility prose remains, but version-1 decoding still expects positional commit_version before ops; its synthetic legacy fixture includes that newer field. INFERRED: the older empty-ops body cannot satisfy this decoder; general fail-closed rejection is not itself a defect. Fix site: reconcile the actual historical shape and compatibility promise in crates/shamir-wal/src/entry/wal_entry_v2.rs.

### RV-WAL-4470BEF3 — Append path accepts payloads that produce well-formed frames which then hard-fail replay

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/segment/wal_segment.rs:240; crates/shamir-wal/src/segment/wal_segment.rs:595; changed by: none
- R0 note (includes the fix site): OBSERVED: Public raw append frames arbitrary bytes without decoding, while replay requires WalEntryV2 decoding; normal transaction producers encode first, so the surviving issue is the direct-API precondition trap. Fix site: enforce or explicitly document the typed-payload requirement in crates/shamir-wal/src/segment/wal_segment.rs.

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
