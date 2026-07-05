# V5.3 (#412) — SQ8 Quantization: f32 vs sq8

> Reproducibility key: `n=1200, dim=128, metric=cosine, k_clusters=50,
> σ=0.25, seed=0xBEEF`. QUICK mode (`shamir_bench_utils::tune`), Windows,
> rustc 1.93.0. Bench: `crates/shamir-engine/benches/quantization_f32_vs_sq8.rs`.

## Summary

The V5.2 (#411) SQ8 quantizer halves the per-vector storage (f32 → u8 codes)
and ships a deferred-fit dual-graph adapter. This report measures the three
trade-off axes (RSS / QPS / recall) on a clustered dataset and documents
the **dual-graph memory floor** that bounds the achievable RSS reduction
within the current adapter design.

## Methodology

- **Dataset**: `clustered(n=1200, dim=128, k=50, σ=0.25, seed=0xBEEF)` —
  same lineage as `quantized_graph_tests::recall_sq8_vs_f32_within_two_percent`.
  `n=1200` crosses the `FIT_THRESHOLD` (256) so the SQ8 adapter fits and
  builds the u8 graph.
- **f32 adapter**: `HnswAdapter::new` (no quantization) — the ground truth.
- **sq8 adapter**: `HnswAdapter::new_with_quantization(_, _, _, Some(Sq8))`.
- **RSS**: `memory_stats::memory_stats().physical_mem` sampled after each
  build (process-wide, order-of-magnitude — NOT a precise allocation profile).
- **QPS**: criterion bench, `--quick` mode, `Throughput::Elements(1)`,
  `ef_search=256`, `k=10`, 1 fixed query.
- **recall@10**: top-10 overlap between the sq8 adapter and the f32 adapter
  over the first 100 dataset vectors (used as queries).

## Measured results (QUICK, single run)

```
[quant-bench] n=1200 dim=128 metric=cosine
  RSS f32 = 16 457 728 bytes (~15.7 MiB)
  RSS sq8 = 27 115 520 bytes (~25.9 MiB)
  Δ       = +10 657 792 bytes (ratio = 1.648 — sq8 is LARGER, see below)
  recall@10 sq8-vs-f32 = 0.9720

quantization_f32_vs_sq8/f32_search
                        time:   [739.90 µs 746.41 µs 748.03 µs]
                        thrpt:  [1.3368 Kelem/s 1.3398 Kelem/s 1.3515 Kelem/s]
                                                 ~1340 QPS

quantization_f32_vs_sq8/sq8_search
                        time:   [2.1944 ms 2.2009 ms 2.2268 ms]
                        thrpt:  [449.07  elem/s 454.37  elem/s 455.71  elem/s]
                                                 ~454 QPS
```

## Analysis

### recall@10 = 0.9720 (≤ 3% drop)

This is within the expected band for SQ8 (4× compression). The V5.2 test
suite pins a 0.95 floor for the same dataset; this run measured 0.972,
comfortably above the floor and consistent with the run-to-run variance
documented in `quantized_graph_tests` (hnsw_rs 0.3.4 uses an unseedable
RNG for layer assignment, so recall varies 0.96–0.98 across builds).

### QPS: sq8 is ~3× slower than f32 at n=1200

| adapter | latency (median) | QPS   |
|---------|------------------|-------|
| f32     | 746 µs           | ~1340 |
| sq8     | 2.20 ms          | ~454  |

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
crossover; deferred to keep the QUICK budget tight.

### RSS: sq8 is LARGER, not smaller (dual-graph floor)

**The 4× per-vector storage reduction is real (u8 codes are `dim` bytes vs
`4·dim` bytes for f32), but the adapter retains BOTH graphs after the fit
transition**:

- the f32 graph (`hnsw: Arc<Hnsw<'static, f32, ShamirDist>>`) is NEVER
  dropped — it stays as the pre-fit fallback and is used by
  `collect_live_vectors` (compaction), `search_prefilter`, and the
  small-index brute-force path;
- the u8 graph (`hnsw_u8`) is ADDED on top;
- the `vectors_u8: scc::HashMap<usize, Vec<u8>>` map is added for rescore
  (the u8 graph does not expose a `get_vector(id)` accessor).

So the post-fit RSS is roughly `f32_graph + u8_graph + vectors_u8_map`,
not `u8_graph` alone. At `n=1200, dim=128` the per-vector u8 codes are
128 bytes vs 512 bytes for f32 — a 4× win per vector — but the f32 graph
struct + layer PointIndexation stays resident, so the NET process RSS goes
UP after the fit (the u8 graph is added, the f32 graph is not freed).

**To realise the 4× memory reduction the adapter would need to DROP the
f32 graph + f32 `vectors` map after the fit.** That is a #412 follow-up:
the f32 graph is currently retained for `collect_live_vectors` (compaction)
and the brute-force fallback, both of which would need a u8-native path.
The snapshot codec (#412, this sheet) already stores both graphs in a v2
snapshot, so a future "drop f32 after fit" change would shrink the live
RSS AND the snapshot size (the `graph`/`data` f32 sections could be
omitted for a fitted adapter).

## Conclusion

- **recall@10 = 0.9720** — SQ8 preserves recall within the ≤5% target.
- **QPS at n=1200: sq8 is ~3× slower** — the wide overscan + f32 rescore
  dominate at small `n`; the crossover is expected at larger `n`.
- **RSS: the 4× per-vector win is masked by the dual-graph floor** — the
  adapter retains the f32 graph post-fit. Realising the RSS reduction
  requires a follow-up to drop the f32 graph after the fit transition.

The snapshot v2 format (#412) is the foundation for that follow-up: it
persists both graphs, so a future "f32-free fitted adapter" can load ONLY
the u8 sections and skip the f32 graph entirely.
