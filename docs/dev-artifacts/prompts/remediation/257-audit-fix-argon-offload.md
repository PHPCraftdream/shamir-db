# 257 — audit-fix-argon-offload (P1 audit follow-up: H11 RV-FUNCLIB-8B0C96FB, argon2 off the runtime workers)

Source: post-merge audit of PR #330 (P1 H11) + @oh design decision (2026-10-29). PR #330 offloaded argon2 to `spawn_blocking`/`block_in_place` at ONE of four dispatch sites and only on a multi-thread runtime. The card acceptance is "argon2 waits off the runtime workers". Read first: `CLAUDE.md`, `crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs` (~25-51 `call_scalar_offloaded`, call at ~418, residual-risk doc), its tests `semantics/tests/resolve_offload_tests.rs` (:44, :53, :62), `crates/shamir-funclib/src/functions/crypto/core.rs` (~107-115 residual-risk doc, `A2_PEAK_IN_FLIGHT` accessor), `ScalarResolver`.

## Decision
Option (b): the acceptance is amended to "argon2 waits off the runtime workers ON MULTI-THREAD runtimes; on current-thread runtimes it runs inline and this is reported loudly". Making it work on current-thread is not safely possible (`block_in_place` panics there, `Handle::block_on` panics inside a runtime, a helper thread + channel still blocks the only worker, returning an error would silently change query results because `resolve.rs` ~418 swallows errors via `.ok()`, async call chain = every per-row `FilterNode::matches`). No non-test code builds a current-thread runtime.

## The real gap to close
Three more sites call the scalar synchronously on a worker even on multi-thread:
- `crates/shamir-engine/src/table/write/write_helpers.rs` ~314-316 (computed values on writes)
- `crates/shamir-engine-query/src/query/read/aggregation/function_inputs.rs` ~208-210
- `crates/shamir-engine-validator/src/validator/schema/rules/field_rule.rs` ~477

## Change
1. Move the helper into funclib as `ScalarResolver::call_offloaded(&self, name, args) -> ScalarResult`. Keep the NAME-based selection (`crypto/argon2id` or `argon2id`) - existing tests rely on a user scalar shadowing the builtin; do NOT add a flag to `FnEntry`. It returns `ScalarResult` (not `Option`) so error-propagating sites keep their errors. Mechanism: the existing `block_in_place` + `spawn_blocking` on a multi-thread runtime, unchanged. State the trade-off in the final report: `shamir-funclib` gains a `tokio` dependency with features `["rt"]` only (already shipped in the binary, no size cost).
2. All four sites call `call_offloaded`; `call_scalar_offloaded` in `resolve.rs` becomes a thin wrapper or is deleted.
3. Loud guard: when a runtime handle exists and its flavor is NOT MultiThread (current-thread or any other): `log::warn!` once per process (`AtomicBool`) naming the stall, and increment a counter `ARGON2_INLINE_ON_RUNTIME` exposed next to the existing `A2_PEAK_IN_FLIGHT` accessor. No runtime at all (plain sync code): run inline, no warning.
4. Rewrite the residual-risk docs at `crypto/core.rs` ~107-115 and `resolve.rs` ~25-29 to state the amended contract.

## Tests
- Keep `resolve_offload_tests.rs:44` and `:62`. Keep `:53` but rename it to state it documents the limitation, and make it also assert the counter went up and the warn flag is set.
- New multi-thread thread-identity probe tests (shadow `crypto/argon2id` with a scalar that records the thread; assert it is not the caller thread) for the write computed-value path, the aggregate-input path and the validator scalar-check path.
- An error result still propagates through `call_offloaded` on the write path.

## Rules
Crates: `shamir-funclib`, `shamir-engine`, `shamir-engine-query`, `shamir-engine-validator` (only the named sites + tests). Only the `tokio` `rt` feature as new dependency edge for funclib. No `#[allow]`, layout limits, imports at top. Do not touch the batch preflight or compile.rs (another executor works on `compile.rs`).

## Local gates (only these)
`cargo fmt -p shamir-funclib -p shamir-engine -p shamir-engine-query -p shamir-engine-validator -- --check`; `cargo clippy -p shamir-funclib --all-targets -- -D warnings`; `cargo check -p shamir-engine -p shamir-engine-query -p shamir-engine-validator --all-targets`; `cargo check -p shamir-db --all-targets`; `python -I scripts/ci/check_layout.py`; after the Cargo.toml change run `cargo metadata --locked --format-version 1` (online) and report the Cargo.lock diff (it must change by the funclib dependency line only). Real exit codes.
