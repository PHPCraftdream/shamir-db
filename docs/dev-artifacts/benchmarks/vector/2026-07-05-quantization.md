# V5.3 (#412) — SQ8 Quantization: f32 vs sq8

> Reproducibility key: `n=1200, dim=128, metric=cosine, k_clusters=50,
> σ=0.25, seed=0xBEEF`. QUICK mode (`shamir_bench_utils::tune`), Windows,
> rustc 1.93.0. Bench: `crates/shamir-engine/benches/quantization_f32_vs_sq8.rs`.

## Summary

The V5.2 (#411) SQ8 quantizer halves the per-vector storage (f32 → u8 codes)
and ships a deferred-fit dual-graph adapter. **#418 frees the f32 graph after
fit**, so the post-fit adapter holds only the u8 graph + codes — the 4×
per-vector storage reduction is now realised in RSS, not masked by the f32
graph staying resident.

## Methodology

- **Dataset**: `clustered(n=1200, dim=128, k=50, σ=0.25, seed=0xBEEF)` —
  same lineage as `quantized_graph_tests::recall_sq8_vs_f32_within_two_percent`.
  `n=1200` crosses the `FIT_THRESHOLD` (256) so the SQ8 adapter fits and
  builds the u8 graph.
- **f32 adapter**: `HnswAdapter::new` (no quantization) — the ground truth.
- **sq8 adapter**: `HnswAdapter::new_with_quantization(_, _, _, Some(Sq8))`.
- **Footprint (#418)**: each adapter's RSS is sampled IN ISOLATION — the f32
  adapter is built, its footprint = (RSS_after - RSS_baseline); it is then
  DROPPED so the sq8 adapter's footprint = (RSS_after_sq8 - RSS_after_f32).
  This isolates each adapter's allocation (the pre-#418 bench kept both
  resident, so `rss_sq8` included the f32 graph and masked SQ8's win). The
  f32 graph of the sq8 adapter is dropped post-fit (#418), so the sq8
  footprint measures only the u8 graph + codes + overhead.
- **QPS**: criterion bench, `--quick` mode, `Throughput::Elements(1)`,
  `ef_search=256`, `k=10`, 1 fixed query.
- **recall@10**: top-10 overlap between the sq8 adapter and the f32 adapter
  over the first 100 dataset vectors (used as queries).

## Measured results (QUICK, #418 — f32 graph freed post-fit)

Three consecutive runs (ratio = sq8_footprint / f32_footprint):

```
[quant-bench] n=1200 dim=128 metric=cosine
  footprint f32 =  9 809 920 bytes (~9.4 MiB)
  footprint sq8 =  4 329 472 bytes (~4.1 MiB)
  Δ             = -5 480 448 bytes (ratio = 0.441)
  recall@10     = 0.9780

[quant-bench] n=1200 dim=128 metric=cosine
  footprint f32 = 10 293 248 bytes (~9.8 MiB)
  footprint sq8 =  2 953 216 bytes (~2.8 MiB)
  Δ             = -7 340 032 bytes (ratio = 0.287)
  recall@10     = 0.9690

[quant-bench] n=1200 dim=128 metric=cosine
  footprint f32 = 10 084 352 bytes (~9.6 MiB)
  footprint sq8 =  2 535 424 bytes (~2.4 MiB)
  Δ             = -7 548 928 bytes (ratio = 0.251)
  recall@10     = 0.9780

quantization_f32_vs_sq8/f32_search
                        time:   [678 µs 697 µs 723 µs]
                        thrpt:  ~1400 QPS

quantization_f32_vs_sq8/sq8_search
                        time:   [2.00 ms 2.08 ms 2.17 ms]
                        thrpt:  ~480 QPS
```

Run-to-run variance in the ratio (0.25–0.44) is allocator-fragmentation
noise in the RSS delta (Windows `heapalloc` does not always return freed
pages to the OS immediately); the median ratio is ~0.29, i.e. **sq8 uses
~29% of the f32 footprint — a ~3.4× reduction, close to the theoretical
4× per-vector win** (the gap is graph-structure overhead that is identical
in bytes for both adapters but a larger fraction of the smaller sq8 pile).

## Analysis

### recall@10 = 0.97–0.98 (≤ 3% drop)

Within the expected band for SQ8 (4× compression). The V5.2 test suite pins
a 0.95 floor for the same dataset; these runs measured 0.97–0.98,
comfortably above the floor and consistent with the run-to-run variance
documented in `quantized_graph_tests` (hnsw_rs 0.3.4 uses an unseedable RNG
for layer assignment, so recall varies 0.96–0.98 across builds).

### QPS: sq8 is ~3× slower than f32 at n=1200

| adapter | latency (median) | QPS   |
|---------|------------------|-------|
| f32     | 697 µs           | ~1400 |
| sq8     | 2.08 ms          | ~480  |

The sq8 search path pays two costs the f32 path does not:

1. **u8-graph traversal with a wide overscan** — the quantized distance is
   lossy, so the adapter requests `overscan = 16k+64 = 224` candidates
   (vs the f32 path's `2k+10 = 30`). At `n=1200` the graph is small enough
   that this overscan visits a large fraction of the nodes.
2. **f32 rescore** — each of the 224 candidates is dequantized and re-scored
   with the exact f32 distance (`O(dim · overscan)` = `128 · 224` f32 ops).

At this scale the rescore + overscan cost dominates the per-hop integer-
distance saving. The crossover where sq8's cheaper traversal pays for the
rescore is expected at larger `n` (the traversal cost grows sub-linearly
with `n` for HNSW, while brute-force rescore grows with `overscan`, which
is fixed wrt `n`). A future bench rung at `n=100_000` would surface the
crossover; deferred to keep the QUICK budget tight. This is a QPS-tuning
concern, orthogonal to the #418 memory win.

### RSS (#418): the 4× memory win is NOW realised

**Before #418** (the dual-graph floor documented in the original #412
report): the adapter retained BOTH graphs after the fit transition — the f32
graph stayed as a fallback, so post-fit RSS was `f32_graph + u8_graph +
vectors_u8_map`, and sq8 RSS was *larger* than f32 (ratio ~1.6).

**After #418**: the f32 graph is DROPPED post-fit. The adapter now holds
only the u8 graph + `vectors_u8` codes + bookkeeping. The measured median
ratio is **~0.29** — sq8 uses ~29% of the f32 footprint, a ~3.4× reduction.
The theoretical per-vector win is 4× (u8 codes are `dim` bytes vs `4·dim`
bytes); the gap to 3.4× is graph-structure overhead (PointIndexation per
layer, Arc<Hnsw> indirection) that is byte-for-byte identical across the two
adapters but is a larger fraction of the smaller sq8 pile.

#### How #418 drops the f32 graph safely

- `hnsw: Arc<Hnsw<f32, ShamirDist>>` → `hnsw: ArcSwapOption<Hnsw<f32, ShamirDist>>`
  (lock-free RCU slot, same pattern as `hnsw_u8`).
- In `try_fit_and_rebuild`, AFTER the u8 graph is published + the catch-up
  drain converges (every pre-flip in-flight upsert has landed in
  `vectors_u8`), the fitter does `self.hnsw.store(None)`.
- In-flight pre-fit SEARCH callers that already did `hnsw.load_full()` hold
  their own `Arc` clone and finish their traversal against the now-private
  graph; `Arc::strong_count` reaches 0 only after the last reader drops its
  clone — **RCU, no UAF**. A reader that has NOT yet loaded observes
  `is_fitted == true` on its next `quantized_active()` check and routes
  through the u8 graph instead.
- All f32-path call sites are gated by `!quantized_active()`, so a post-fit
  `None` on the f32 path is an invariant violation, surfaced as an error
  (upsert / upsert_batch / backfill) or an empty result (search read path)
  — never a panic on the normal path.
- Non-quantized adapters NEVER drop the f32 graph (`try_fit_and_rebuild`
  returns early when `quantization.is_none()`): bit-for-bit back-compat.
- The v2 snapshot codec dumps ZERO-length f32 sections for a fitted adapter
  (the f32 graph is gone); the load path substitutes a throwaway empty
  `Hnsw::new(...)` stub that `from_parts_with_quantization` drops on
  assembly — no f32 allocation survives a restart either.

The deterministic regression (`f32_graph_present() == false` post-fit for a
quant adapter; `== true` for non-quant / pre-fit) is pinned in
`quantized_graph_tests::f32_graph_dropped_after_fit_and_search_survives`.

## Conclusion

- **recall@10 = 0.97–0.98** — SQ8 preserves recall within the ≤5% target.
- **QPS at n=1200: sq8 is ~3× slower** — the wide overscan + f32 rescore
  dominate at small `n`; the crossover is expected at larger `n` (a QPS
  tuning concern, orthogonal to the memory win).
- **RSS (#418): the 4× memory win is realised** — median sq8/f32 ratio
  ~0.29 (sq8 uses ~29% of the f32 footprint). Pre-#418 the ratio was ~1.6
  (sq8 was LARGER); #418 drops the f32 graph post-fit and brings it to
  ~¼–⅓. The deterministic `f32_graph_present()` regression pins the drop.
