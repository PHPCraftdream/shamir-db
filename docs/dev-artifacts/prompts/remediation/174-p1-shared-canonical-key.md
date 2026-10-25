# P0 stage D1-S2 — p1-shared-canonical-key

## Goal

Part of card `RV-FUNCLIB-5FA455F7` (two coexisting equality semantics: `==` variant-strict vs `compare` value-strict; read the WHOLE card in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md`). Staged owner ruling (two rush reviewers + @oh): S0 (`Value::hash` canonicalises -0.0) and S1 (index/scan equality parity through a canonical numeric key; rebuild-required) are MERGED. This stage is **S2 (P1) + S3**:

- **S2:** DISTINCT, GROUP BY and foreign-key matching must use the SAME canonical numeric key as the index and the scan filter (reuse the S1 canonical-key function: if it lives in `shamir-index`, move/expose it to the lowest crate both the engine and the index can depend on - likely `shamir-types` or `shamir-collections` - and have the index keep calling it; one implementation, no copy). Add EXACT Int<->Big equality (an `Int(n)` equals the `Big` with the same integer value and hashes identically; never through f64).
- **S3 (adopted):** equality is exact and hashable; the LOSSY F64<->Dec/Big comparison stays for ORDERING only (not for equality, DISTINCT, GROUP BY or FK). Amend the stale comment at `crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs` ~:179 (verify the line at HEAD) to state this contract.

Binding rules:
- Canonical key for hashing/grouping: integral F64 that fits i64 losslessly keys like the equal Int; -0.0 keys as 0; NaN is never equal to anything (a NaN group key / DISTINCT value must follow the same rule as the filter: NaN != NaN, decide and DOCUMENT deterministically: each NaN is its own distinct value for DISTINCT/GROUP BY but never matches an FK); non-integral floats keep their bit key; Int<->Big by exact value; Dec compared/hashed by exact decimal value where an equal Int exists (document what Dec<->Int/F64 does for equality - exact only).
- Do NOT change `Value::PartialEq` / `Value::hash` public semantics beyond what S0 did; the shared key is a separate function used by the three consumers.
- Persisted data impact: DISTINCT/GROUP BY are query-time; FK matching reads existing data through the shared key - if any persisted key changes, add a CHANGELOG "Changed (index rebuild required)" line, otherwise a plain "Changed" line (alpha policy: no migration code).
- Do NOT touch the wire representation of Dec/Big/Set (that is D2, a later docs/golden-bytes stage).

Tests (names contain `rv_funclib_5fa455f7`): DISTINCT and GROUP BY over a mixed corpus (Int(1), F64(1.0), -0.0/0, Int(2^53+1), Big equal to an Int, NaN) produce the same groups the filter equality implies; FK insert/delete matches Int vs F64(1.0) vs Big of the same value; exact Int<->Big (including values beyond 2^53 and beyond i64 for Big-only) never collapses distinct values; ordering of F64 vs Dec/Big is still lossy-by-design and unchanged. Each test fails with only its own fix reverted.

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
