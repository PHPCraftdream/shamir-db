# P1 stage F6 — p1-wal-errors-hygiene

## Goal

Typed WAL errors, group-commit error context, docs/layout. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F6): WAL errors are a thiserror enum that keeps `io::ErrorKind`; group-commit waiters receive the underlying error; corruption is not `DbError::Internal`; the retired KV-marker API is removed or marked; `has_truncatable` is O(1) per tick; inline tests move to `tests/`; stale docs fixed; the historical bench numbers are re-measured or withdrawn.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-WAL-C55C2D47 — Group-commit layer discards the underlying error — waiters get a context-free generic Err

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:320; crates/shamir-wal/src/segment/wal_ticket.rs:24; changed by: none
- R0 note (includes the fix site): OBSERVED: physical outcome categories remain available, but append and sync errors are still reduced through is_ok and tickets carry no underlying cause; #274 added a fault seam, not cause propagation. Fix site: preserve sink failure causes through crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs and the ticket error representation.

### RV-WAL-5839FF0D — No thiserror error enum — stringly-typed errors, io::ErrorKind destroyed at the wrap boundary

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/segment/wal_segment.rs:166; crates/shamir-wal/src/entry/wal_entry_v2.rs:278; changed by: none
- R0 note (includes the fix site): OBSERVED: File-open errors still become Storage strings and codec failures become Internal strings, losing typed causes despite the shared thiserror DbError already providing Io; a dedicated WAL enum is not itself required. Fix sites: preserve causes at the wrapping boundaries in crates/shamir-wal/src/segment/wal_segment.rs and crates/shamir-wal/src/entry/wal_entry_v2.rs.

### RV-WAL-645DBB85 — Corrupt on-disk data decoded as DbError::Internal — misclassifies corruption as a code bug

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/entry/wal_entry_v2.rs:265; crates/shamir-wal/src/entry/wal_entry_v2.rs:303; changed by: none
- R0 note (includes the fix site): OBSERVED: Short input, bad magic, malformed versioned bodies and unsupported versions still return Internal, whose shared error definition denotes an internal logic error. Fix site: classify persisted-data decode failures separately from programmer failures in crates/shamir-wal/src/entry/wal_entry_v2.rs.

### RV-WAL-79167DD7 — Retired F5c KV-marker wire protocol still exported as public API, with docs describing it as live

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/lib.rs:60; crates/shamir-wal/src/entry/wal_entry_v2.rs:14; changed by: none
- R0 note (includes the fix site): OBSERVED: WalActiveKey and magic-sniffing helpers remain public with live info-store recovery prose, while code references remain their own tests and production uses segmented files. Fix sites: retire or accurately mark the legacy surface in crates/shamir-wal/src/lib.rs and crates/shamir-wal/src/entry/wal_entry_v2.rs.

### RV-WAL-B8E9884C — has_truncatable on the Mem sink is an O(frames) scan run on every drainer tick

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/segment/wal_sink.rs:399; crates/shamir-engine/src/tx/drain/drainer/truncate.rs:105; changed by: none
- R0 note (includes the fix site): OBSERVED: The drainer still calls has_truncatable, whose Mem branch holds the frames mutex while any scans retained frames; when none qualifies, work scales with the entire backlog. No measured CPU severity is established. Fix site: maintain a constant-time eligibility summary in crates/shamir-wal/src/segment/wal_sink.rs.

### RV-WAL-2859EC6A — Inline #[cfg(test)] mod tests in an implementation file

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/entry/segment_meta.rs:199; changed by: none
- R0 note (includes the fix site): OBSERVED: The implementation still contains an inline cfg(test) module, now with six tests rather than five; WMT2 remediation extended it rather than moving it to the mandated tests directory. Fix site: relocate the inline tests from crates/shamir-wal/src/entry/segment_meta.rs into the existing test hierarchy.

### RV-WAL-705D095F — segment_set.rs module doc claims it is unwired scaffold

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wal/src/segment_set/segment_set.rs:15; crates/shamir-engine/src/repo/instance/repo_instance/transactions.rs:192; changed by: none
- R0 note (includes the fix site): OBSERVED: The module still says it is unwired additive scaffolding, while production repository initialization constructs SegmentSet and installs WalSink::File. Fix site: replace the obsolete wiring paragraph in crates/shamir-wal/src/segment_set/segment_set.rs.

### RV-WAL-B8C4B31A — Historical 4.4× scaling, 63× fsync dominance, and lock-cost measurements

- R0 verdict: `unverified`, risk `—`; evidence: crates/shamir-wal/benches/segment_set_lock.rs:131; crates/shamir-wal/src/segment/wal_segment.rs:234; changed by: none
- R0 note (includes the fix site): OBSERVED: Sustained timing still includes payload production, append framing, blocking dispatch and file writes rather than isolated mutex acquisition. Missing premise: reproducible measurements and methodology establishing the historical 4.4× and 63× ratios and isolated lock cost. Fix site: separate these workloads and measurement claims in crates/shamir-wal/benches/segment_set_lock.rs before collecting CI benchmark evidence.

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
