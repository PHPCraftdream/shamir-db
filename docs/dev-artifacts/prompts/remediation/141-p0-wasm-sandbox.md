# P0 stage B3 — p0-wasm-sandbox

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): The forbidden-macro scan works on tokens, not text; fuel values above `i64::MAX` are clamped; the aggregate budget is reserved before a nested call and debited on cancellation; the Argon2 semaphore never loses a wakeup; the aggregate-fuel test fails against per-Store fuel.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-WASM-HOST-5E65E165 — Forbidden-macro scan bypassed by whitespace/comment between macro name and `!`

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-wasm-host/src/compile.rs:365; crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs:214
- Revalidation note: AGREE — OBSERVED: comment stripping leaves spaces but forbidden-macro recognition still requires an immediately adjacent exclamation mark, and source-backed validator registration still reaches this compiler pipeline without another macro guard.

### RV-WASM-HOST-2E9CF487 — `fuel > i64::MAX` immediately errors AND makes the wall-clock/epoch test vacuous

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-wasm-host/src/wasm/wasm_function.rs:473; crates/shamir-wasm-host/src/tests/wasm/wasm_tests.rs:246
- Revalidation note: AGREE — OBSERVED: u64::MAX still seeds AtomicI64 with -1 and fails before Store creation, while the deadline test accepts any error with only an upper elapsed-time bound, so epoch removal remains undetected by that oracle.

### RV-WASM-HOST-6F709AC2 — Aggregate fuel budget is enforced retroactively: in-flight chain can draw (depth+1) × fuel; doc overclaims the bound

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-wasm-host/src/wasm/wasm_function.rs:475; crates/shamir-wasm-host/src/wasm/wasm_function.rs:624
- Revalidation note: AGREE — OBSERVED: nested calls inherit the same counter without charging the suspended parent's consumption, receive independent Store grants from its undebited balance and debit only after returning, so the aggregate bound remains unenforced during the chain.

### RV-WASM-HOST-A6416C6A — `wasm_aggregate_fuel_exhausted_across_nested_calls` cannot distinguish aggregate from per-Store fuel — vacuous regression test

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-wasm-host/src/tests/wasm/wasm_tests.rs:140; crates/shamir-wasm-host/src/tests/wasm/wasm_tests.rs:313
- Revalidation note: AGREE — OBSERVED: the WAT still unconditionally recurses through a trapping host import without a successful base case, and expect_err cannot distinguish aggregate exhaustion from per-Store fuel, depth or epoch failure.

### RV-WASM-HOST-8AFCE04E — Aggregate fuel-budget debit is skipped when the call future is cancelled (cancelled task permanently leaks budget)

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-wasm-host/src/wasm/wasm_function.rs:588; crates/shamir-wasm-host/src/wasm/wasm_function.rs:624
- Revalidation note: AGREE — OBSERVED: the top-level timeout drops the nested call future, whose shared-budget debit remains trailing code rather than a drop guard, leaving consumed child fuel uncharged within that invocation rather than leaking capacity across fresh contexts.

### RV-WASM-HOST-D2732518 — The delegated Argon2 concurrency semaphore can lose its final wakeup

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-funclib/src/functions/crypto/core.rs:141; crates/shamir-funclib/src/functions/crypto/core.rs:146
- Revalidation note: AGREE — OBSERVED: the delegated Argon2 path still uses a semaphore whose release increments and notifies without acquiring the wait mutex, allowing the final notification between the waiter's failed predicate check and Condvar registration to be lost.

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
