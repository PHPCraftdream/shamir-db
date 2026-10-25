# P0 stage E-collections — p0-collections-contract

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): see the cards

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-COLLECTIONS-91305482 — TMap/TSet docs omit the O(N) order-preserving-removal asymmetry; consumers hit it on hot paths via the alias invisibly

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:734; crates/shamir-tx/src/mvcc_store/history/version_entry.rs:128
- Revalidation note: AGREE — OBSERVED: sorted overlay winners still become a TMap consumed by repeated shift_remove during key-major history traversal, retaining the quadratic entry-shifting worst case and undocumented removal asymmetry.

### RV-COLLECTIONS-D5FF5F41 — No documented serialization/wire contract for TMap-backed protocol fields

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-collections/src/lib.rs:19; crates/shamir-query-types/src/ops/batch/req/batch_request.rs:123
- Revalidation note: AGREE — OBSERVED: the leaf alias still provides no duplicate-key wire contract, and queries deserialize directly into TMap whose locally inspected serde visitor overwrites duplicate keys through insert.

### RV-COLLECTIONS-E214C501 — Collections has no local contract suite; downstream regression protection is real but incomplete

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-collections/src/lib.rs:25; crates/shamir-collections/src/lib.rs:63
- Revalidation note: AGREE — OBSERVED: the collections source tree still contains only lib.rs without test registration or a local contract suite, preserving the scoped coverage gap rather than proving downstream tests absent.

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
