# P1 stage F9 — p1-index-vector

## Goal

Vector index: watermark vs promotions, config persistence, leaks, dims. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F9): The snapshot absorption watermark counts only promoted deltas; persisted `VectorConfig.backend` and SQ8 opt-in survive reopen; quantized load and generation flip leave no orphan chunks; staged vectors validate dimension; rebuild failures are logged/propagated; `drop_all` removes everything and reports per-key errors; SQ8 fit does not stall one upsert.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-INDEX-216A1B0E — Snapshot absorption watermark counts durable delta appends before their graph promotions have completed

- R0 verdict: `fixed` — CLOSED, change nothing for it. OBSERVED: Snapshots now sample the contiguous applied prefix before dumping and use that exact boundary for manifest absorption and pruning; the engine fires acknowledgements only after successful graph promotion. A delayed or failed promotion therefore no longer advances the prune boundary merely by appending its delta.

### RV-INDEX-8DB6B1EC — Quantized snapshot load permanently leaks complete temporary dump files

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/vector/snapshot/load.rs:311; crates/shamir-index/src/vector/snapshot/load.rs:324; changed by: none
- R0 note (includes the fix site): OBSERVED: Quantized load writes complete graph/data files, leaks their TempDir, then invokes the fallible loader; neither success nor loader failure retains a cleanup owner. Fix site: crates/shamir-index/src/vector/snapshot/load.rs must separate required loader lifetime ownership from temporary-file cleanup.

### RV-INDEX-FAB4E4E5 — Persisted `VectorConfig.backend` is ignored on the reopen/rebuild path

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/backends/build_backend.rs:52; crates/shamir-index/src/vector/hnsw_adapter/fit.rs:283; changed by: none
- R0 note (includes the fix site): OBSERVED: Reopen constructs default HNSW without inspecting cfg.backend, absent/unusable snapshots still rebuild that adapter, and compaction still hardcodes graph tuning. Successful snapshot restoration does not close these fallback paths. Fix sites: crates/shamir-index/src/backends/build_backend.rs and crates/shamir-index/src/vector/hnsw_adapter/fit.rs, honoring backend selection and retaining live tuning.

### RV-INDEX-E69B5ACD — SQ8 quantization opt-in has no durable carrier and is lost on most restarts

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/model/kind.rs:185; crates/shamir-index/src/vector/hnsw_adapter/adapter.rs:347; changed by: none
- R0 note (includes the fix site): OBSERVED: Descriptor serde still skips quantization intent, ordinary snapshot reconstruction sets None, and fallback construction also omits it; fitted SQ8 snapshots remain the counterexample that restores intent. Fix site: crates/shamir-index/src/model/kind.rs, introducing a compatible durable intent carrier and threading it through restoration.

### RV-INDEX-07424EF0 — `flip_generation` never prunes the old generation's `qgraph`/`qdata` chunks

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/vector/vector_backend/maintenance.rs:72; crates/shamir-index/src/vector/snapshot/load.rs:624; changed by: none
- R0 note (includes the fix site): OBSERVED: Snapshot maintenance retains only old graph/data counts, and flip_generation removes graph/data/sidecar but no qgraph/qdata keys; the applied-prefix remediation does not add quantized-generation cleanup. Fix sites: crates/shamir-index/src/vector/vector_backend/maintenance.rs and crates/shamir-index/src/vector/snapshot/load.rs must carry and prune the old quantized sections.

### RV-INDEX-AB9403D5 — Staged vectors bypass dim validation on the in-tx merge paths (debug panic / silent truncation)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/vector/vector_backend/backend.rs:475; crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:681; changed by: none
- R0 note (includes the fix site): OBSERVED: staged_vector returns unchecked extracted lengths, engine staging accepts them, and ordinary HNSW/BruteForce merges score them without validation; the separate score_staged_candidates guard is not used by these merges. INFERRED: Unequal lengths reach asserting/truncating distance kernels. Fix site: crates/shamir-index/src/vector/vector_backend/backend.rs needs dimension-checked staging, with guards also at adapter staged-merge boundaries.

### RV-INDEX-DA21C946 — `try_fit_and_rebuild` failures silently dropped behind comments that falsely claim logging

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:199; crates/shamir-index/src/vector/hnsw_adapter/search.rs:476; changed by: none
- R0 note (includes the fix site): OBSERVED: Single upsert, batch upsert and backfill still discard the fit Result; the fit implementation returns errors without logging, including catch-up errors after publication. Fix sites: crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs and crates/shamir-index/src/vector/hnsw_adapter/fit.rs, handling failures with publication-phase-aware diagnostics.

### RV-INDEX-AA2CD737 — Default VectorAdapter batch violates its dimension-atomicity contract

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-index/src/vector/quant/adapter.rs:107; crates/shamir-index/src/vector/quant/brute_force.rs:235; changed by: none
- R0 note (includes the fix site): OBSERVED: Default upsert_batch promises upfront dimension atomicity but awaits each upsert sequentially; BruteForce inherits it and enqueues a valid first row before rejecting a malformed second row. HNSW's override protects only that adapter. Fix site: crates/shamir-index/src/vector/quant/adapter.rs must validate all dimensions before invoking any upsert.

### RV-INDEX-7BB73DA5 — SQ8 fit runs inline on the threshold-crossing write — O(N) stall on one upsert

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:199; crates/shamir-index/src/vector/hnsw_adapter/search.rs:191; changed by: none
- R0 note (includes the fix site): OBSERVED: The threshold-crossing upsert still awaits try_fit_and_rebuild, including training, graph construction and catch-up; spawn_blocking protects runtime workers during major CPU phases but does not remove caller latency. Fix site: crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs needs separately owned background fit scheduling coordinated with publication.

### RV-INDEX-0B756318 — index2 `drop_all` sweeps swallow per-key errors, and `VectorBackend::drop_all` leaks the entire `__vec_snap__` keyspace

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-index/src/backends/fts_ranked_backend.rs:409; crates/shamir-index/src/vector/vector_backend/backend.rs:841; changed by: none
- R0 note (includes the fix site): OBSERVED: Ranked FTS, plain FTS and functional drop loops still discard removal errors, vector drop remains a no-op, and recovery sweeps only the binary index-id prefix rather than the string snapshot namespace. Fix sites: crates/shamir-index/src/backends/fts_ranked_backend.rs and sibling drop implementations must propagate failures, while crates/shamir-index/src/vector/vector_backend/backend.rs needs snapshot-namespace cleanup.

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
