# P0 stage D2 — Wire representation of Dec / Big / Set

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): see the cards

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-TYPES-F28FAFBF — Wire format cannot represent Dec / Big / Set -- types silently degrade to Str / List on every encode

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/record_view/scalar_ref.rs:192; crates/shamir-types/src/codecs/README.md:430
- Revalidation note: AGREE: intentional Str/List serialization remains, stored-string scalar comparisons still lack typed Dec/Big RHS arms, and the documentation still claims all-type MessagePack round trips.

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

## OWNER RULING (binding): documented v1 projection, NOT a protocol change
Document `Dec`/`Big` -> `Str` and `Set` -> `List` as the **v1 wire/storage projection** (a documented, intentional degradation; no protocol change, no version gate in this stage). Deliverables:
1. **Docs.** Correct every place that claims all-type MessagePack round trips (e.g. `crates/shamir-types/src/codecs/README.md` ~:430 and any guide/spec page that mentions Dec/Big/Set encoding): state precisely what is written on encode (`Dec`/`Big` as their decimal string, `Set` as a list), what comes back on decode (`Str`/`List`), and the consequences that are NOW defined by D1-S2: a stored `Big`/`Dec` groups/compares as a string (DISTINCT/GROUP BY over stored bytes), the in-memory `NumericKey` classes apply only to type-preserving paths, equality rule "equal iff equal canonical NumericKey and not NaN". Add a short "future typed encoding" note (what a v2 would need: an explicit type tag + version gate). Mention it in `CHANGELOG.md` under an appropriate heading (no version bump).
2. **Golden byte tests** in the owning crate's `tests/` dir (manifest `mod.rs` only): pin the exact MessagePack bytes produced for `Dec` (e.g. 1.50, a negative, a large-scale one), `Big` (small, > i64, negative) and `Set` (with a stable order or an explicitly documented order) — write the expected bytes as literal byte arrays derived by reading the encoder code (you cannot run anything), AND the decode result type (`Str`/`List`) so the projection is pinned in both directions. Test names contain `f28fafbf`.
3. **Typed-RHS handling with D1-S2:** check the stored-string scalar comparison paths mentioned in the card's revalidation note (`scalar_ref.rs` around the `Str` vs `Dec`/`Big` RHS arms). D1-S2 is merged: verify with file:line what a stored string compared against a typed `Dec`/`Big` right-hand side does today. If there is a missing typed arm that makes a stored `Big`/`Dec` string unequal to the equal typed value in filters/FK, implement it in `scalar_ref_numeric_eq` (parse the stored string to the exact value and compare by the canonical key) with tests; if it is already handled, say so with evidence and add a pinning test.
Do not change the wire encoding itself. Keep every file <=1000 lines and folders <=7 entries.
