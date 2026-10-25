# P0 stage C2 — p0-client-refs-resume

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Reference detection covers `when` guards, `ForEach` and `ValueCompare` operands; resume verifies the pinned server identity before sending the ticket.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-CLIENT-5212ED16 — `batch_has_refs` misses two ref carriers: `QueryEntry.when` guards and `BatchOp::ForEach`

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-client/src/interner_cache/ops.rs:513; crates/shamir-client/src/interner_cache/ops.rs:422
- Revalidation note: AGREE — OBSERVED: the smart-path reference walk still ignores entry.when and ForEach, permitting opaque Id intermediates despite guards or loop sources consuming earlier results.

### RV-CLIENT-236DAD0A — The reference-walk fix also misses Filter::ValueCompare operands

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-client/src/interner_cache/ops.rs:535; crates/shamir-query-types/src/read/filter/filter_enum.rs:146
- Revalidation note: AGREE — OBSERVED: ValueCompare still has two reference-capable operands but falls through filter_has_refs, including through the existing recursive Cond-condition traversal.

### RV-CLIENT-DE75ADDD — `Client::resume` hands the bearer ticket to an unverified peer; `pinned_hash` is dead bookkeeping on the resume path

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-client/src/client/connect.rs:481; crates/shamir-client/src/client/connect.rs:510
- Revalidation note: AGREE — OBSERVED: resume still sends the bearer ticket through accept-any-certificate TLS without an application identity proof and only copies the supplied pin into the resulting client.

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
