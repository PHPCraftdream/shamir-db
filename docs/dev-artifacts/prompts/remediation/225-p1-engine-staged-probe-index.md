# P1 stage F14b — p1-engine-staged-probe-index

## Goal

Split from F14. Fix card RV-ENGINE-0DB9257B (read the whole `### RV-ENGINE-0DB9257B` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md`): staged-overlay probes clone and linearly scan the whole tx write-set per validated record, so repeated unsuccessful probes over a growing write-set are quadratic.

This worktree is based on branch fix/p1-engine-batch (PR #349, P1 F14, not yet merged). Its changes are a given: build on them, do not revert or rework them.

### RV-ENGINE-0DB9257B — Staged-overlay probe clones and linearly scans the whole tx write-set per validated record

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-engine/src/validator/registry/validator_db.rs:381; crates/shamir-engine/src/validator/registry/validator_db.rs:501; changed by: none
- R0 note (includes the fix site): OBSERVED: both staged probes still iterate the whole staging map until a match; iter_ops avoids eager vector materialization but yields cloned owned handles. INFERRED structural cost remains quadratic for repeated unsuccessful probes over a growing write-set. Fix site: crates/shamir-engine/src/validator/registry/validator_db.rs needs indexed or amortized staged-field probing.

## Binding design (design review; implement as written, deviate only with file:line evidence)

1. `shamir-types` `scalar_ref.rs`: add `ProbeKey` helpers — a hashable, owned key derived from a scalar field value, equal exactly when the existing equality used by the probes says equal.
2. `shamir-tx/src/tx/support/staged_field_index.rs` (new file): a lazily-built `StagedFieldIndex` (per table, per probed field path → `ProbeKey` → record ids), built on the first probe and then maintained incrementally by `StagingStore` `set` / `set_many` / `remove`; cleared (rebuilt lazily) on `rewrite_set_bytes`. Use Fx hashers. Single-tx ownership: no `std::sync::Mutex` on the hot path.
3. The four engine probes — `validator/registry/validator_db.rs` (around :365-388 and :486-514), `fk_restrict.rs` (around :494-507), `fk_on_update/scan.rs` (around :337-350) — call `probe_field` and then re-check each candidate with the existing `record_field_matches_by_id`, so semantics stay exact (the index only narrows candidates).
4. Regression `rv_engine_0db9257b_staged_probe_is_not_linear`: a `#[cfg(test)]` decode counter; 200 staged inserts validated in one tx must cost at most 400 record decodes (the linear scan costs O(n^2)). Plus a correctness test: probe results equal the old linear scan, including after `remove` and `rewrite_set_bytes`.

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
