# P0 stage A6 — p0-index-correctness

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Dec/Big/Bin values hash distinctly in the functional backend; tokenizers case-fold non-ASCII words.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-INDEX-56A505F0 — FunctionalBackend hash collapses every Dec/Big/Bin value to an identical posting hash

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-index/src/backends/functional_backend.rs:173; crates/shamir-engine/src/table/read/dispatch/vector.rs:795
- Revalidation note: AGREE — OBSERVED: unsupported computed leaves still contribute only tag 255, functional lookup accepts matching posting hashes without value verification, and candidate collection supplies no residual predicate to distinguish the collisions.

### RV-INDEX-A570106D — Whitespace/Full tokenizers never case-fold words whose uppercase letters are all non-ASCII (Russian/Greek FTS broken)

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-index/src/fts/tokenizer.rs:55; crates/shamir-index/src/fts/tokenizer.rs:277
- Revalidation note: AGREE — OBSERVED: Whitespace and Full still select borrowed tokens using an ASCII-byte uppercase test, so words with only non-ASCII uppercase letters bypass lowercasing despite the Unicode tokenizer's separate correct helper.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
