# P0 stage D1-S1 — p0-index-scan-parity

## Goal

Part of card `RV-FUNCLIB-5FA455F7` (two coexisting equality semantics; read the whole card in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md`). D1-S0 (Value::hash -0.0) is already merged. This stage is S1 of the staged ruling by two rush reviewers + @oh: **index probe results must equal full-scan filter results for equality/IN over mixed numeric values**.

Evidence (@oh, verify against HEAD): the scan filter compares F64 with `partial_cmp` and Int<->F64 exactly (`crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs` ~:161-163, `crates/shamir-types/src/record_view/scalar_ref.rs` ~:189-191). The index builds keys from type tags plus `to_bits` (`crates/shamir-index/src/base_index/infra/index_keys.rs` ~:86-93, ~:121-127). So an index probe MISSES: -0.0 vs 0.0; Int(1) vs F64(1.0); and it can MATCH NaN to NaN while the filter never does.

Binding rules:
- One canonical numeric key used on BOTH the write path and the probe path of regular, unique and functional indexes: an F64 that is integral and fits losslessly in i64 is keyed exactly like the equal Int; -0.0 is keyed as 0; NaN is NEVER matched by an equality/IN probe (same as the filter) and is not a unique conflict with another NaN; non-integral floats keep their bit key. Mixed Int/F64 comparisons beyond 2^53 must stay exact (an Int that is not exactly representable never equals a float).
- Unique-conflict detection uses the same canonical key: inserting Int(1) after F64(1.0) is a unique conflict.
- Affected persisted index keys change for float/int-valued keys: existing indexes must be dropped and re-created; add a CHANGELOG "Changed (index rebuild required)" entry (alpha policy: no in-place upgrade; do NOT write migration code).
- Do NOT touch DISTINCT/GROUP BY/FK/filters (later stage D1-S2) and do NOT change `Value::PartialEq`.

Tests (names contain `rv_funclib_5fa455f7`): property-style table test that for a mixed corpus (Int, F64 integral/non-integral, +0.0/-0.0, NaN, i64::MAX/MIN, 2^53+1, nested not required) `eq` and `in` probes through the index return exactly what the scan filter returns; unique index rejects Int(1) after F64(1.0) and does not conflict NaN with NaN; -0.0 and 0 probes find each other. Each test fails with only its own fix reverted.

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
