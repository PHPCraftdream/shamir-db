# P0 stage E-bench — p0-bench-utils

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): see the cards

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-BENCH-UTILS-E1F3D036 — `peak_mem`'s global allocator is silently active for every bench/example/test binary of shamir-engine and shamir-index, contradicting the documented "off by default" contract

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-engine/Cargo.toml:114; crates/shamir-bench-utils/src/peak_mem.rs:39
- Revalidation note: AGREE — OBSERVED: engine and index dev-dependencies still enable peak_mem, so linked fixture-only consumers inherit the global allocator independently of setup calls.

### RV-BENCH-UTILS-E6177532 — The tracking allocator always moves reallocations, changing benchmark work rather than merely adding counters

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-bench-utils/src/peak_mem.rs:39; Cargo.lock:2443
- Revalidation note: AGREE — OBSERVED: HEAD still selects peak_alloc 0.3.0 globally, and its locally inspected realloc allocates, copies and frees rather than delegating to System.realloc.

### RV-BENCH-UTILS-126E5E6F — Reproducibility key excludes any generator/dataset-format version

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/vector_data.rs:158; crates/shamir-engine/examples/vector_report.rs:332
- Revalidation note: OBSERVED: the reproducibility contract and report header still identify parameters without a generator revision or source SHA.

### RV-BENCH-UTILS-325F3AC6 — Box-Muller scale is unpinned — a transcription bug in `next_gaussian` would pass the entire suite

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/vector_data.rs:234; crates/shamir-bench-utils/src/vector_data.rs:326
- Revalidation note: OBSERVED: tests still compare symmetric regeneration and impose only an upper clustering-distance bound, leaving Gaussian scale without a discriminating reference or lower variance bound.

### RV-BENCH-UTILS-A7605FF1 — The cross-crate LCG "lineage" contract is prose-only: ~13 hand-maintained mirrors, zero enforcement, and the stated justification for the duplication is stale

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-index/src/vector/tests/quant/sq8_tests.rs:14; crates/shamir-index/src/vector/tests/quant/sq8_tests.rs:47
- Revalidation note: OBSERVED: SQ8 retains its independent Box-Muller implementation and stale no-dev-dependency justification, with no canonical comparison in that generator's tests.

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
