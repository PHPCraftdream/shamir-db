# P1 stage H11 — p1-funclib-agg

## Goal

Funclib aggregates: overflow, O(N) distinct, streaming stddev. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H11): Decimal arithmetic in aggregates cannot panic; `age` uses checked arithmetic; `count_distinct`/`DistinctWrapper` are hash-based; `stddev`/`variance` stream; F64->i64 range check exact; argon2 waits off the runtime workers.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-FUNCLIB-0CEB1AB2 — `rust_decimal` arithmetic panics on overflow in agg and array reductions

- R0 verdict: `unverified`, risk `medium`; evidence: crates/shamir-funclib/src/functions/numeric/agg/core.rs:514; Cargo.lock:2968; changed by: none
- R0 note (includes the fix site): OBSERVED: Decimal variance and array reductions retain unchecked arithmetic; HEAD still locks rust_decimal 1.43.0, while the inspected local registry contains only 1.40.0 arithmetic source and no 1.43.0 archive. Missing premise: exact locked-version operator overflow behavior. Fix site: crates/shamir-funclib/src/functions/numeric/agg/core.rs, together with arrays/core.rs, is the checked-arithmetic remediation point if that premise is confirmed.

### RV-FUNCLIB-D45CF639 — `datetime/age` performs unchecked `i64` subtraction — inconsistent with the file's own checked discipline

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/functions/datetime/core.rs:78; crates/shamir-funclib/src/registry.rs:208; changed by: none
- R0 note (includes the fix site): OBSERVED: arg_i64 accepts Int(i64::MIN) unchanged and age directly subtracts it from current epoch milliseconds, without calling the timestamp-range validator. INFERRED: A positive current epoch overflows this subtraction. Fix site: crates/shamir-funclib/src/functions/datetime/core.rs must use checked subtraction and return a defined error.

### RV-FUNCLIB-A1B1A620 — `count_distinct` aggregator is O(N·C) -- the exact legacy pattern `arrays::distinct` was fixed to remove

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/functions/numeric/agg/core.rs:202; crates/shamir-engine/src/query/read/aggregation/function_inputs.rs:107; changed by: none
- R0 note (includes the fix site): OBSERVED: AggregateFn still reaches the builtin factory, and count_distinct scans every previous representative before appending a unique value; unique inputs retain N(N−1)/2 comparisons. Fix site: crates/shamir-funclib/src/functions/numeric/agg/core.rs needs indexed deduplication preserving the chosen equality contract.

### RV-FUNCLIB-B30DC9AC — `DistinctWrapper` dedup is O(N·C) per wrapped aggregate

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/functions/numeric/agg/core.rs:245; crates/shamir-engine/src/query/read/aggregation/group_projection.rs:120; changed by: none
- R0 note (includes the fix site): OBSERVED: DISTINCT AggregateFn dispatch still installs DistinctWrapper, whose unbounded seen Vec is linearly searched for every non-null input; no cardinality guard closes the quadratic case. Fix site: crates/shamir-funclib/src/functions/numeric/agg/core.rs needs indexed deduplication preserving representative and inner-input order.

### RV-FUNCLIB-676F82A9 — `stddev`/`variance` buffer the entire column though an O(1)-state algorithm exists

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/functions/numeric/agg/core.rs:463; crates/shamir-funclib/src/functions/numeric/agg/core.rs:499; changed by: none
- R0 note (includes the fix site): OBSERVED: registered stddev and variance aggregators still append every non-null Decimal and retain the vectors until two-pass finalization; no constant-state replacement or per-vector cap is present. Fix site: crates/shamir-funclib/src/functions/numeric/agg/core.rs needs a constant-state algorithm with an explicit rounding, accuracy and overflow contract.

### RV-FUNCLIB-1EF53AB4 — F64→i64 range check is off by one ulp: silently saturates instead of `out_of_range`

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-funclib/src/registry.rs:218; crates/shamir-funclib/src/functions/value/cast/core.rs:121; changed by: none
- R0 note (includes the fix site): OBSERVED: both guards retain the inclusive i64::MAX-as-f64 upper bound followed by an integer cast. INFERRED from IEEE representation and Rust cast semantics: exact 2^63 is admitted and saturates, rather than returning the respective conversion error. Fix sites: crates/shamir-funclib/src/registry.rs and crates/shamir-funclib/src/functions/value/cast/core.rs need an exclusive upper bound.

### RV-FUNCLIB-8B0C96FB — `argon2id` blocking semaphore acquire runs inline on async runtime workers (documented residual risk — tracked, listed for completeness)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs:373; crates/shamir-funclib/src/functions/crypto/core.rs:271; changed by: none
- R0 note (includes the fix site): OBSERVED: Expression evaluation synchronously dispatches the scalar, which acquires a blocking Condvar-backed permit and performs the KDF inline; #271 repairs wakeup synchronization, not offloading. Fix site: crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs needs an offloaded expression-call path for blocking scalars.

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
