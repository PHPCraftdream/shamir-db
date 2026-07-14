Task: G5 (task #530) — two independent vector/storage perf findings
deferred from task #496 (audit `docs/dev-artifacts/audits/2026-07-06-perf-hot-paths.md`,
findings 2.3 and 4.1/4.2). Former #515 (MemBuffer merge-overlay) +
former #516 (SQ8 fused rescore + weighted-SIMD kernels).

Read the audit doc sections directly before starting (`## 2.` for finding
2.3, `## 4.` for findings 4.1/4.2) — this brief summarizes but line
numbers may have drifted since the audit was written; confirm against
current code.

## Part A — MemBuffer merge-overlay instead of drain-before-scan (former #515)

### Current behavior (confirmed)

`crates/shamir-storage/src/storage_membuffer.rs`'s four scan methods
(`iter_stream` ~line 540, `scan_prefix_stream` ~line 570,
`iter_range_stream` ~line 595, `iter_range_stream_reverse`) ALL do the
same thing first: loop calling `drain_once` until it returns `Ok(0)`
(dirty buffer fully flushed to `inner`), THEN stream from `inner` alone.
Since `MemBufferStore` wraps EVERY store behind `BoxRepo::MemBuffer`
(`crates/shamir-engine/src/repo/repo_types.rs:57-60`), under mixed
read/write load, EVERY full-scan / FTS-token prefix-scan / sorted-index
range-scan forces a write-flush first — read-triggered write
amplification, and it defeats the 500ms fsync-batching interval every
time a scan happens to run.

### Fix

Don't drain. Instead, snapshot the dirty overlay (the SMALL side) and
MERGE it on top of the `inner` stream (the LARGE side) — the exact
pattern `crates/shamir-tx/src/mvcc_store/mod.rs::current_stream`
(currently ~line 1116) already uses for MVCC's overlay-vs-history merge:
materialize the overlay into a small in-memory map at stream-open time
(`self.overlay.snapshot_le(floor)`-equivalent — for MemBuffer this would
be a snapshot of the current dirty map), then during the `inner` stream
walk, for each key check the overlay map first (an overlay entry, if
present, wins — including `Slot::Tombstone` entries, which must EXCLUDE
that key from the result even though `inner` might still have stale data
for it), falling through to `inner`'s value when the overlay has nothing
for that key. After the `inner` stream is exhausted, any overlay entries
whose keys were NEVER visited by the `inner` walk (i.e., keys that exist
ONLY in the dirty overlay, not yet flushed to `inner` at all) must still
be yielded — read `current_stream`'s "DrainOverlay" phase
(`StreamingGroupByState::DrainOverlay`, referenced in the same function)
for how it handles this "overlay-only tail" case.

**This needs to work correctly for ALL FOUR scan shapes**, which differ
in ordering guarantees:
- `iter_stream`: no ordering guarantee needed beyond "some order" — the
  simplest case.
- `scan_prefix_stream`: must still only yield keys under the given
  prefix — both from `inner` AND from the overlay-only tail (filter the
  overlay snapshot to the prefix range before merging).
- `iter_range_stream` / `iter_range_stream_reverse`: must preserve the
  requested key ORDER (ascending / descending) across the merge — this
  is the hardest part. The overlay-map and the `inner` stream must be
  walked in a genuine sorted merge (like a merge-sort merge step), not
  "inner then overlay-tail appended" (which would break ordering for
  range scans specifically, unlike the prefix/full-scan cases where order
  doesn't matter as much — confirm what ordering guarantee callers of
  each method actually rely on before deciding how much precision is
  needed here).

### Scope-down guidance

If the sorted-merge requirement for `iter_range_stream`/`_reverse`
specifically turns out to be substantially more invasive than the other
two methods (e.g. requires restructuring `inner`'s stream cursor
interface), it's acceptable to land the fix for `iter_stream` and
`scan_prefix_stream` first (both order-insensitive, lower risk) and
defer the range-stream variants as a follow-up with documented reasoning
— per this campaign's established pattern, don't force a risky fix for
marginal extra coverage. State clearly in your report which methods got
the merge-overlay treatment and which (if any) still drain, and why.

### TDD

1. A test proving a scan (each of the 4 methods, or however many you
   fix) returns CORRECT results (including entries only in the dirty
   overlay, and correctly EXCLUDING tombstoned keys) WITHOUT forcing a
   drain — instrument/verify no flush happened (e.g. check dirty state
   after the scan still has entries, or count `drain_once` invocations).
2. Existing MemBuffer tests must stay green — scan correctness/ordering
   must not regress.

### Performance verification (MANDATORY)

The audit itself notes NO bench covers "scan under writes"
(`membuffer_pump` only measures insert/get). Add a bench (or a bench
variant) exercising a scan running CONCURRENTLY with (or immediately
after, without settling) ongoing writes, comparing p99 latency (or just
wall-time honestly, whichever this repo's bench harness supports)
before/after. Follow the `CARGO_TARGET_DIR=<isolated dir>`
(POSIX-style path if on bash/Windows) convention.

## Part B — SQ8 fused rescore + weighted-SIMD kernels (former #516)

### Current behavior (confirmed)

`crates/shamir-index/src/vector/quantized_dist.rs::rescore_f32` (currently
~line 258) calls `params.dequantize(codes)` — a FRESH `Vec<f32>`
allocation (dim × 4 bytes) PER CANDIDATE — then computes dot-products
against that dequantized vector, including re-computing `dot(query,
query)` (the query's own norm) on EVERY candidate even though it's
identical every time within one search. With an overscan of `16k+64`
candidates (per the audit, confirm current value in
`hnsw_adapter.rs`), this is hundreds of allocations per search.

`sq8.rs`'s `approx_dot`/`approx_l2_sq` (called on every HNSW-traversal
edge — roughly `ef × M` times per search) already had its
`scales_sq`/`min_scale` precompute landed in task #496 — confirm this by
reading the current code before assuming more precompute work is needed
here. What's NOT yet done (per the audit finding 4.2) is replacing the
scalar per-dimension loop with SIMD weighted-dot/weighted-L2 kernels,
using the existing SIMD kernel patterns already in `crates/shamir-index/src/vector/simd.rs`
(`dot_product_avx2`/`avx512`/`neon` — read these as the template for
correct feature-detection/fallback structure).

### Fix

1. **Fused rescore (do this part — low-risk, no unsafe code needed):**
   precompute once per query (NOT per candidate): `qm = dot(query,
   mins)`, `qs[i] = query[i] * scales[i]` for all `i`, and `q_norm =
   dot(query, query)`. Then for each candidate's u8 codes, compute
   `dot(query, dequantized_candidate) = qm + Σ qs[i] * codes[i]` in a
   SINGLE pass over the u8 codes (u8→f32 convert + multiply-accumulate),
   with ZERO per-candidate heap allocation. Apply the same decomposition
   for L2. This alone is a substantial, safe win per the audit's own
   estimate (rescore ×2-4, minus the per-candidate dequant allocation
   entirely) — land this even if the SIMD kernel part below proves too
   risky to do safely in this pass.
2. **SIMD weighted kernels (do this if it can be done safely and
   correctly — see scope-down guidance):** write
   `weighted_dot_product`/`weighted_l2_squared`-style SIMD kernels
   (following `simd.rs`'s existing AVX2/AVX512/NEON pattern +
   scalar-fallback structure) that take the precomputed `qs`/`qm` (or
   equivalent) and the u8 code slice directly, avoiding both the
   allocation AND the scalar per-dim loop in `approx_dot`/`approx_l2_sq`.

### Scope-down guidance (IMPORTANT — SIMD intrinsics carry real correctness risk)

This repo's own `unsafe`/SIMD code (`simd.rs`) presumably already has
careful feature-detection and safety-comment discipline — follow that
EXACT pattern, do not introduce a new style. If, after investigation,
writing correct new SIMD kernels for the weighted-dot/L2 case would
require significant new unsafe code you're not fully confident in (e.g.
because the weighted variant needs different lane-shuffling than the
existing kernels' template), it is ACCEPTABLE to land ONLY the fused
rescore (item 1 — no new unsafe code, portable, still a real win) and
defer the SIMD weighted-kernel part (item 2) as its own follow-up task
with documented reasoning, rather than risk a subtly incorrect SIMD
kernel. State clearly in your report which parts landed.

### TDD

1. Existing quantization/rescore correctness tests must stay green — the
   fused rescore must produce numerically-equivalent (within float
   tolerance) results to the current dequant-then-dot approach. Add a
   test asserting this equivalence explicitly (not just "existing tests
   still pass" — a direct comparison test) for a handful of query/code
   combinations across all three metrics (L2, Dot, Cosine).
2. If SIMD kernels are added: a test comparing the SIMD path's output
   against the scalar fallback path for the same inputs, across a range
   of dimensions (including non-SIMD-width-aligned dims, if the existing
   `simd.rs` kernels have padding/remainder handling — mirror whatever
   pattern they already use).

### Performance verification (MANDATORY)

Use the existing `quantization_f32_vs_sq8` bench (per the audit, this
bench already exists and would show the effect) for before/after
numbers. Follow the `CARGO_TARGET_DIR=<isolated dir>` convention. Report
honest numbers for whichever part(s) actually landed.

## General

Per this session's lighter per-task gate:
```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-storage -p shamir-index -p shamir-engine
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Part A] Status: fixed / partially-fixed (which methods) / deferred
  > Which scan methods got merge-overlay treatment, tombstone handling,
    ordering-preservation approach for range scans
  > Bench: scan-under-writes before/after

[Part B] Status: fixed (fused rescore) + fixed/deferred (SIMD kernels)
  > Fused rescore approach + equivalence test results
  > SIMD kernel decision (landed or deferred, with reasoning)
  > Bench: quantization_f32_vs_sq8 before/after

[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-storage -p shamir-index -p shamir-engine: pass/fail
```
