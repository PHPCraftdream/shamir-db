# P0 stage E-funclib — p0-funclib-resolver

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): see the cards

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-FUNCLIB-18A4B82E — `UserScalarLayer::register` discards scc's `Err` — documented "(or replace)" silently never replaces

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/scalar_resolver.rs:39; Cargo.lock:3120
- Revalidation note: AGREE — OBSERVED: register still discards insert_sync's occupied-key error, and the locally available pinned scc 3.8.4 implementation explicitly returns that error without updating the existing value.

### RV-FUNCLIB-4936C6ED — `scalar_resolver` is the only real module with no tests and no `tests/` dir

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/scalar_resolver.rs:135; crates/shamir-funclib/src/tests/mod.rs:1
- Revalidation note: OBSERVED: resolver-specific test registration and tests of replacement, shadowing, arity and identity remain absent in funclib, although the title's directory-uniqueness claim remains inaccurate.

### RV-FUNCLIB-C8BC39BC — `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes with no locking"

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/scalar_resolver.rs:25; crates/shamir-funclib/src/scalar_resolver.rs:46
- Revalidation note: OBSERVED: lookup still uses get_sync despite the no-locking documentation, and the locally inspected pinned scc 3.8.4 implementation retains an exclusive writer-bucket guard while cloning.

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
