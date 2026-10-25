# P0 stage B5 — p0-parser-limits

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): `is_json` has a depth cap; `repeat`/`pad_*`/`random_bytes` enforce a byte cap; msgpack preallocation is capped by the remaining input; planner walks are iterative or depth-bounded; regexes are cached without a global mutex.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-FUNCLIB-84DD4DA9 — `validate/is_json` hand-rolled parser recurses without a depth cap — stack-overflow abort from a ~20 KB query string

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-funclib/src/functions/text/validate/core.rs:123; crates/shamir-funclib/src/functions/text/validate/core.rs:178
- Revalidation note: AGREE — OBSERVED: is_json's parser still mutually recurses through value/object/array without depth state or a nesting guard on the evaluated string; the numerical crash threshold is not established.

### RV-FUNCLIB-CDACB2E5 — Unbounded-allocation scalar paths: `strings/repeat`, `strings/pad_left`/`pad_right`, `gen/random_bytes`

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-funclib/src/functions/text/strings.rs:237; crates/shamir-funclib/src/functions/numeric/gen/core.rs:75
- Revalidation note: AGREE — OBSERVED: repeat, padding and random_bytes still reject only negative sizes before unchecked usize conversion and output allocation, with no output-byte ceiling guarding amplification.

### RV-TYPES-BB78489C — Unbounded preallocation from attacker-controlled msgpack headers -- allocator abort, unlike the capped tree visitor

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-types/src/codecs/interned/messagepack.rs:305; crates/shamir-types/src/codecs/interned/messagepack.rs:581
- Revalidation note: AGREE: custom container decoding and storage merge still allocate directly from declared counts before validating bodies, and the capped generic Value visitor does not guard these separate entry points.

### RV-QUERY-TYPES-8257ACB4 — Unbounded recursive walks over already-parsed attacker trees

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-types/src/ops/batch/plan/planner.rs:253; crates/shamir-engine/src/query/batch/exec/batch_execute.rs:193
- Revalidation note: AGREE — OBSERVED: W-series caps now bound engine query counts, but public planner DFS still precedes dependency-depth rejection and recursive value/filter extraction still precedes engine filter-depth validation.

### RV-FUNCLIB-64320F3C — Global `std::sync::Mutex` regex cache on the hot filter path — regex compiled while holding the lock

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/functions/text/strings.rs:423; crates/shamir-funclib/src/functions/text/strings.rs:427
- Revalidation note: AGREE — OBSERVED: every regex-cache access still acquires one process-global mutex, misses compile under its guard, and the next miss at 256 entries clears the entire cache.

### RV-FUNCLIB-F1354271 — `validate/matches` recompiles the regex on every call

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-funclib/src/functions/text/validate/core.rs:342; crates/shamir-engine/src/query/filter/evaluation/semantics/resolve.rs:372
- Revalidation note: AGREE — OBSERVED: matches still invokes Regex::new inside its scalar closure and filter evaluation dispatches that closure per evaluation without a compiled-pattern reuse boundary.

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
