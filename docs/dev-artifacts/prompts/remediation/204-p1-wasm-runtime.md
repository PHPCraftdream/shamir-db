# P1 stage G5 — p1-wasm-runtime

## Goal

WASM runtime lifecycle: env seeding, globals bound, epoch ticker, registry. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G5): Non-UTF-8 env vars cannot panic seeding; guest `global_set` is bounded; epoch-ticker spawn failure is surfaced; registry replace/rename are atomic and never drop a function; compile child processes are always reaped; host-import trap paths have tests; the async-import global read card is decided with evidence.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-WASM-HOST-A07B9019 — `GlobalVars::seed_env` panics on any non-UTF-8 environment variable

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wasm-host/src/runtime/context.rs:212; crates/shamir-db/src/shamir_db/shamir_db/core.rs:282; changed by: none
- R0 note (includes the fix site): OBSERVED: database initialization still calls seed_env, which iterates std::env::vars before policy filtering. INFERRED from the Unicode iterator contract: an invalid-Unicode entry can still panic even when policy would exclude it. Fix site: handle non-Unicode environment entries explicitly in crates/shamir-wasm-host/src/runtime/context.rs.

### RV-WASM-HOST-51B448DF — Unbounded process-lifetime growth of `GlobalVars` via guest `global_set`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wasm-host/src/wasm/bindings/globals.rs:55; crates/shamir-wasm-host/src/runtime/context.rs:166; changed by: none
- R0 note (includes the fix site): OBSERVED: the live guest import inserts arbitrary distinct non-env keys into the shared globals map without a retained-entry or byte quota; the env write guard and new fuel leases do not cap accumulation across invocations. Fix site: enforce shared retained-storage accounting in crates/shamir-wasm-host/src/runtime/context.rs and report admission failure through the globals binding.

### RV-WASM-HOST-FA126F4A — Epoch-ticker thread spawn failure is swallowed with `.ok()` — wall-clock pre-emption silently disabled, nothing logged

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:121; crates/shamir-wasm-host/src/wasm/wasm_engine.rs:156; changed by: none
- R0 note (includes the fix site): OBSERVED: ticker creation still discards thread-spawn failure with .ok(), and engine construction returns success without logging or propagating it. INFERRED: such failure leaves epoch advancement absent; finite fuel is independent mitigation, not a wall-clock guarantee. Fix site: propagate ticker startup failure from spawn_epoch_ticker through engine construction in crates/shamir-wasm-host/src/wasm/wasm_engine.rs.

### RV-WASM-HOST-C51052DF — Non-atomic remove-then-insert overwrite in `replace` / `put` / `set`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/runtime/registry.rs:50; crates/shamir-wasm-host/src/runtime/context.rs:166; changed by: none
- R0 note (includes the fix site): OBSERVED: replace, BatchContext::put and GlobalVars::set retain separate remove/insert operations and discard insertion results; the existing atomic update helpers are not used by them. INFERRED: readers can observe absence and competing insertion can defeat the intended update. Fix site: replace those setter sequences in crates/shamir-wasm-host/src/runtime/registry.rs and crates/shamir-wasm-host/src/runtime/context.rs with atomic entry updates.

### RV-WASM-HOST-EAE6ABE8 — `FunctionRegistry::rename` rollback can silently drop the function

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/runtime/registry.rs:73; crates/shamir-wasm-host/src/runtime/registry.rs:84; changed by: none
- R0 note (includes the fix site): OBSERVED: rename still removes the source before inserting the target and ignores restoration failure. INFERRED: concurrent registrations occupying both names can make both insertions fail and drop the original function; the preliminary target check does not guard that interval. Fix site: preserve ownership through collision/rollback in FunctionRegistry::rename in crates/shamir-wasm-host/src/runtime/registry.rs.

### RV-WASM-HOST-58C052B0 — Compile child-process teardown is incomplete on every abnormal-exit path (no Job Object; kill-failure waits unbounded; error path never kills)

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/compile.rs:750; crates/shamir-wasm-host/src/compile.rs:765; changed by: none
- R0 note (includes the fix site): OBSERVED: timeout ignores direct-child kill failure before unbounded wait and reader joins, and wait errors join without killing; spawning cargo still establishes no process-tree ownership. Moving compilation to spawn_blocking in #273 does not change teardown. Fix site: implement bounded process-tree teardown for these branches in crates/shamir-wasm-host/src/compile.rs.

### RV-WASM-HOST-91385BE0 — No in-crate test coverage for any host-import trap/error path (db/http/batch/global imports, depth limit, OOB result pointers, missing exports)

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/tests/wasm/wasm_sanitizer_tests.rs:66; crates/shamir-wasm-host/src/tests/wasm/bindings/http_bindings_tests.rs:42; changed by: none
- R0 note (includes the fix site): OBSERVED: current additions cover fuel/cancellation, nested-call trap propagation, successful gated DB calls and HTTP header encoding/valid decoding, not the named db/http/batch/global misuse, depth-rejection, missing-export or OOB-result oracles; sanitizer imports remain uncalled. Fix site: add discriminating guest failure-path oracles under crates/shamir-wasm-host/src/tests/wasm/bindings.

### RV-WASM-HOST-BD9BC412 — Present batch/global reads fail in modules importing any async host function

- R0 verdict: `unverified`, risk `medium`; evidence: crates/shamir-wasm-host/src/wasm/bindings/globals.rs:108; crates/shamir-wasm-host/src/wasm/bindings/batch.rs:67; changed by: none
- R0 note (includes the fix site): OBSERVED: Both present-value getters still synchronously call shamir_alloc, while the linker also exposes async imports; Cargo.lock:4690 still selects Wasmtime 48.0.7, but only 48.0.5 source is locally available. The exact missing premise remains whether 48.0.7 rejects this synchronous reentry for a mixed-import instance. Fix sites: crates/shamir-wasm-host/src/wasm/bindings/globals.rs and batch.rs must use an allocator execution mode proven compatible with the locked runtime.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
