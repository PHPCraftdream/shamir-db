# P0 stage D1-S0 — p0-value-hash-zero

## Goal

Owner ruling D1-S0 (two reviewers agreed), part of card `### RV-FUNCLIB-5FA455F7` in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md`: `Value::PartialEq` uses IEEE `==` so `+0.0 == -0.0` (`crates/shamir-types/src/types/value.rs:293-298`), but `Hash` feeds raw `to_bits()` and only folds NaN (`value.rs:706-710`) — this violates the Eq/Hash contract (HashSet / funclib `distinct` can keep both zeros nondeterministically).

Acceptance:
- In `Value`'s `Hash`, hash `-0.0` exactly as `+0.0` (NaN folding unchanged). Check every other hand-written Hash over float payloads in shamir-types that is paired with an IEEE-equality `PartialEq` and apply the same rule; do NOT change `PartialEq`, index keys, DISTINCT/GROUP BY keys or any wire format (later stages own those).
- Regression tests (names contain `rv_funclib_5fa455f7`): property-style table test that `a == b ⇒ hash(a) == hash(b)` over ±0.0, several NaN payloads, and the same values nested in List / Set / Map; funclib `distinct` over `[0.0, -0.0]` returns exactly one element.

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
