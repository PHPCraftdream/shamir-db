# P0 stage A5 — p0-index-durability

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Restart never reuses a delta index below the absorption watermark; replay and compaction failures fail closed (no snapshot or swap of an incomplete graph); `SortedIndexManager::load` propagates store errors.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-INDEX-2F3CBCA3 — Restart reseeds next_delta_idx below the persisted absorption watermark after pruning

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-index/src/vector/vector_backend/backend.rs:704; crates/shamir-index/src/vector/snapshot/load.rs:564
- Revalidation note: AGREE — OBSERVED: restore still seeds next_delta_idx from surviving chunks alone with scan errors defaulted to zero, while replay skips indices below the persisted absorption boundary, preserving the prune/reopen/append/reopen loss mechanism.

### RV-INDEX-4651AA00 — Vector delta-replay failure is warned away, then permanently baked in by the next background snapshot

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-index/src/vector/vector_backend/backend.rs:682; crates/shamir-index/src/vector/snapshot/load.rs:636
- Revalidation note: AGREE — OBSERVED: replay errors still produce only a warning before restore succeeds, and generation flip can subsequently prune absorbed delta chunks without a recovery-failure barrier protecting omitted mutations.

### RV-INDEX-9F62C8DF — Compaction double-write errors silently discarded; an incomplete graph is then swapped in as live

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-index/src/vector/vector_backend/backend.rs:511; crates/shamir-index/src/vector/vector_backend/maintenance.rs:252
- Revalidation note: AGREE — OBSERVED: compaction-target vector-write errors remain discarded and target publication has no latched-error check or failed-update reconciliation; the existing delete reconciliation does not close failed upserts.

### RV-INDEX-86978429 — `SortedIndexManager::load` swallows ALL store errors, silently loading zero sorted definitions

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-index/src/base_index/sorted_index_manager/inner.rs:638; crates/shamir-index/src/base_index/sorted_index_manager/inner.rs:624
- Revalidation note: AGREE — OBSERVED: load still maps every definition-store get error to successful empty initialization, while later persist_defs serializes and writes the resulting registry without preserving the unread definitions.

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
