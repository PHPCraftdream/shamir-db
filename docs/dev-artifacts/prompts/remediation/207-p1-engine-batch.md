# P1 stage F14 — p1-engine-batch

## Goal

Engine batch validation depth, regex caps, overlay probe, unverified proofs. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F14): Executable value conditions go through the normal batch depth and pattern collection; Regex/Like patterns are size-capped and invalid patterns are errors; the staged-overlay probe is not a per-record linear scan; the two unverified proofs (SeqCst/loom, unwrap guards) are either demonstrated or the cards closed with evidence.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-ENGINE-C1A53A7A — Executable value conditions bypass normal batch depth and pattern collection

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-engine/src/query/batch/exec/batch_validate.rs:145; crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs:395; changed by: none
- R0 note (includes the fix site): OBSERVED: filter collection still excludes ForEach.over, bind values and SELECT arguments; ForEach resolves over before iteration limits and Cond reaches unchecked cached compilation. Numeric equality and builder-side document depth checks do not guard this engine input path. Fix site: crates/shamir-engine/src/query/batch/exec/batch_validate.rs must traverse every executable FilterValue surface.

### RV-ENGINE-0DB9257B — Staged-overlay probe clones and linearly scans the whole tx write-set per validated record

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-engine/src/validator/registry/validator_db.rs:381; crates/shamir-engine/src/validator/registry/validator_db.rs:501; changed by: none
- R0 note (includes the fix site): OBSERVED: both staged probes still iterate the whole staging map until a match; iter_ops avoids eager vector materialization but yields cloned owned handles. INFERRED structural cost remains quadratic for repeated unsuccessful probes over a growing write-set. Fix site: crates/shamir-engine/src/validator/registry/validator_db.rs needs indexed or amortized staged-field probing.

### RV-ENGINE-80890C84 — Missing error-path tests for the specific gaps above

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-engine/src/query/batch/tests/fk/actions/fk_indexed_action_read_error_tests.rs:625; crates/shamir-engine/src/table/tests/read/query/mod.rs:5; changed by: none
- R0 note (includes the fix site): OBSERVED: pre-read and verify-panic modules remain registered, but the FK corruption oracle still injects only truncated non-map byte 0xc4; its changes only adapt Doc::set calls. Fix site: crates/shamir-engine/src/query/batch/tests/fk/actions/fk_indexed_action_read_error_tests.rs needs a malformed-map-body oracle distinct from both-decoder rejection.

### RV-ENGINE-2A18B647 — Regex/Like patterns have no size cap and invalid patterns silently compile to False

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-engine/src/query/batch/exec/batch_validate.rs:145; crates/shamir-engine/src/query/filter/evaluation/planning/compile.rs:104; changed by: none
- R0 note (includes the fix site): OBSERVED: collected filters receive the 64-KiB/pattern-validity guard, but executable values omitted by collection still reach unchecked Regex construction and invalid-pattern False folding. Fix site: crates/shamir-engine/src/query/batch/exec/batch_validate.rs must include executable value-condition surfaces, with checked compilation covering direct callers.

### RV-ENGINE-898ECB5E — SeqCst proof and loom coverage

- R0 verdict: `unverified`, risk `—`; evidence: crates/shamir-engine/src/table/support/guards/writer_drain_barrier.rs:448; Cargo.lock:1993; changed by: none
- R0 note (includes the fix site): OBSERVED: Loom remains 0.7.2 and the copied model explicitly uses compensating SC fences that mask weakening surrounding access orderings. Missing premise: an independent weak-memory oracle sensitive to production-order weakening, not evidence that production is unsafe. Fix site: crates/shamir-engine/src/table/support/guards/writer_drain_barrier.rs needs that discriminating model/proof coverage.

### RV-ENGINE-B789E7AC — Every remaining production unwrap/expect is structurally guarded

- R0 verdict: `unverified`, risk `—`; evidence: crates/shamir-engine/src/query/auth/session.rs:184; crates/shamir-engine/src/table/read/dispatch/collect.rs:106; changed by: none
- R0 note (includes the fix site): OBSERVED: singleton extraction is guarded by len==1 and top-k unwraps by is_some checks, supporting these particular sites only. Missing premise: an exhaustive current production unwrap/expect inventory with a guard proof for every entry. Fix scope starts at crates/shamir-engine/src/query/auth/session.rs and crates/shamir-engine/src/table/read/dispatch/collect.rs, extending the inventory across production engine sources.

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
