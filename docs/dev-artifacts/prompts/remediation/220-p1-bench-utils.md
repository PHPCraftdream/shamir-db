# P1 stage I1 — p1-bench-utils

## Goal

Bench utils: peak_mem ownership, docs, layout. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row I1): `peak_mem` has an owned measurement window and tests; docs describe the current harness and reproducibility key; generator allocation-in-loop removed; tests moved to `tests/`.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-BENCH-UTILS-C5422902 — `peak_mem`'s measurement window is unowned: concurrent or aborted use silently corrupts the one process-global peak watermark

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/peak_mem.rs:132; crates/shamir-bench-utils/src/peak_mem.rs:160; changed by: none
- R0 note (includes the fix site): OBSERVED: Both measurement helpers reset the same unowned global watermark; the replacement reset still loads CURRENT then stores PEAK, and nested calls can erase an outer peak. Fix site: crates/shamir-bench-utils/src/peak_mem.rs, measurement-window ownership and reset ordering or an explicit unsupported-use contract.

### RV-BENCH-UTILS-AF0F6362 — `ClusteredDataset` doc claims "(k, σ) parameters are recoverable from the artefact alone" — σ never is, and k is clamped

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/vector_data.rs:116; crates/shamir-bench-utils/src/vector_data.rs:184; changed by: none
- R0 note (includes the fix site): OBSERVED: ClusteredDataset stores only vectors and centroids, yet still claims sigma recovery; positive-n generation also discards requested k through clamping. Fix site: crates/shamir-bench-utils/src/vector_data.rs, the artifact contract or retained generation metadata.

### RV-BENCH-UTILS-77EB8D3E — peak_mem has zero test coverage, unpinned dependency semantics, and dead public API

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-bench-utils/src/peak_mem/tests/allocator_tests.rs:15; crates/shamir-bench-utils/src/peak_mem.rs:160; changed by: 66ea63d0 (#287)
- R0 note (includes the fix site): OBSERVED: The external peak_alloc dependency is removed and registered tests assert real alloc/realloc live and peak counters, refuting zero coverage; measure and measure_async still lack behavioral oracles and workspace callers. Fix site: crates/shamir-bench-utils/src/peak_mem/tests/allocator_tests.rs for measurement-window and reset semantics, with API ownership decided in peak_mem.rs.

### RV-BENCH-UTILS-2CAF36EA — `clustered_vectors` Panics section omits the `dim == 0` assert; `sigma` domain undocumented

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/vector_data.rs:168; crates/shamir-bench-utils/src/vector_data.rs:176; changed by: none
- R0 note (includes the fix site): OBSERVED: Panics documents only zero clusters, but zero dimension also asserts; sigma remains unrestricted and undocumented beyond standard-deviation terminology. Fix site: crates/shamir-bench-utils/src/vector_data.rs, the clustered_vectors contract and input guards.

### RV-BENCH-UTILS-2F1D995C — Fixture generator allocates one heap `Vec` per point (allocation-in-loop, n+1 allocations per call)

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/vector_data.rs:204; crates/shamir-engine/benches/vector_search/vector_search.rs:121; changed by: none
- R0 note (includes the fix site): OBSERVED: Each positive-dimensional point receives its own Vec allocation, and the adapter batch clones each row; no performance magnitude is established. Fix site: crates/shamir-bench-utils/src/vector_data.rs, the dataset representation and generation loop, accounting for consumer ownership requirements.

### RV-BENCH-UTILS-67097ED5 — `peak_mem.rs` docs still teach Criterion as the module's usage pattern — the harness the workspace removed and banned

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-bench-utils/src/peak_mem.rs:11; crates/shamir-bench-utils/src/lib.rs:3; changed by: none
- R0 note (includes the fix site): OBSERVED: The revised allocator example still teaches Criterion iter_custom, while the crate explicitly requires bench-scale-tool. Fix site: crates/shamir-bench-utils/src/peak_mem.rs, its primary usage example.

### RV-BENCH-UTILS-CF87C480 — Entire test module embedded inline in `vector_data.rs`, violating the mandatory `tests/` layout

- R0 verdict: `fixed` — CLOSED, change nothing for it. OBSERVED: vector_data now declares an external tests module whose manifest registers generation_tests and gaussian_tests; the inline test body is removed.

### RV-BENCH-UTILS-4E7A9919 — Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-bench-utils/src/vector_data.rs:29; crates/shamir-engine/benches/vector_search/vector_search.rs:19; changed by: none
- R0 note (includes the fix site): OBSERVED: Adding GENERATOR_REVISION did not supply n and dim in the module's reproducibility key; the consumer still calls its five-value tuple a triple. Fix sites: crates/shamir-bench-utils/src/vector_data.rs and crates/shamir-engine/benches/vector_search/vector_search.rs, their reproducibility guidance.

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
