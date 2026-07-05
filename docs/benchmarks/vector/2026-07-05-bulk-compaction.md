# Bulk-load and compaction benchmark — 2026-07-05

`upsert_batch` (single rayon `parallel_insert`) vs N serial `upsert`, and
rebuild-aside compaction cost at varying tombstone fractions.

## Reproducibility

- **Bench**: `crates/shamir-engine/benches/vector_bulk_compaction.rs`
- **Dataset**: `clustered_vectors` — n ∈ {1_000, 10_000}, dim=128, k_clusters=64,
  sigma=0.1, seed=42
- **Metric**: Cosine
- **HNSW**: M=16, max_layer=16, ef_construct=200, ef_search=50
- **Tombstone selection**: deterministic LCG (Numerical Recipes constants),
  seed=742
- **Host**: Windows 10 x86_64
- **Mode**: QUICK (sample=10, measurement=500ms, warm_up=500ms; criterion
  auto-extends measurement when a single iteration exceeds the budget)

## How to reproduce

```bash
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench \
  cargo bench -p shamir-engine --bench vector_bulk_compaction
```

## Group 1 — bulk-load: serial vs batch

Same dataset for both paths. **Serial** = N individual `upsert` calls (each its
own `spawn_blocking` graph insert). **Batch** = one `upsert_batch` call (single
rayon `parallel_insert` over the whole dataset).

| n      | Serial (mean) | Batch (mean) | Speedup | DoD ≥5× |
|-------:|--------------:|-------------:|--------:|:--------|
| 1_000  | 426.75 ms     | 124.77 ms    | 3.4×    | —       |
| 10_000 | 12.522 s      | 1.0198 s     | **12.3×** | ✅      |

**DoD ≥5× CONFIRMED** at n=10_000. The batch path's single `parallel_insert`
amortizes graph-insert overhead across rayon workers; at n=1_000 the fixed
batching overhead (slot-claim phase + spawn_blocking setup) narrows the gap to
3.4×, but the asymptotic advantage grows with N — at 10K the serial path is
dominated by per-upsert spawn_blocking scheduling, while the batch path does all
graph work in one parallel pass.

Throughput at n=10_000: **9.8K elem/s** (batch) vs **799 elem/s** (serial).

## Group 2 — compaction rebuild-aside cost

Index of n=10_000, tombstone fraction d via deterministic `delete`, then
rebuild-aside: collect the live-set (original batch minus tombstoned rids) and
build a fresh `HnswAdapter` from it via `upsert_batch`. This models the hot path
of `run_background_compaction` Steps 3–4 (`collect_live_vectors` +
`backfill_if_absent`) at the adapter level.

| Tombstone fraction | Live vectors | Rebuild-aside (mean) | Throughput |
|-------------------:|-------------:|---------------------:|-----------:|
| 0.30               | 7_000        | 744.35 ms            | 9.4K elem/s |
| 0.50               | 5_000        | 495.64 ms            | 10.1K elem/s |

**Scaling**: rebuild-aside cost is linear in the live count (~9.4–10.1K
elem/s — the same `parallel_insert` throughput as bulk-load batch). Compacting
a 30%-tombstoned index (7K live) costs ~744 ms; a 50%-tombstoned index (5K
live) costs ~496 ms. The cost is proportional to survivors, not to the original
index size — confirming that rebuild-aside is cheaper than a full rebuild when
the tombstone fraction is high (fewer live vectors to re-insert).

The throughput is stable across tombstone fractions (~9.4–10.1K elem/s), which
matches the bulk-load batch throughput — both exercise the same
`parallel_insert` code path, so compaction rebuild cost is predictable from the
live count alone.

## Compaction modelling note

The rebuild-aside primitives (`collect_live_vectors`,
`new_compaction_target`, `backfill_if_absent`) are `pub(crate)` in
`shamir-index` — the bench (an external consumer via `shamir-engine`) cannot
call them directly. Since the bench drives the dataset with a deterministic
seed, it knows exactly which rids were deleted, so it assembles the live-set
(original batch minus tombstoned rids) and feeds it to a fresh `HnswAdapter`
via the public `upsert_batch`. The work performed — O(live) graph inserts in one
`parallel_insert` — is identical to what `backfill_if_absent` does on the
live-set collected from a dirty adapter; the cost measured here is a faithful
proxy for the compaction rebuild step.

## Raw bench output

```
vector_bulk_load/serial/n1000
                        time:   [416.77 ms 426.75 ms 438.60 ms]
                        thrpt:  [2.2800 Kelem/s 2.3433 Kelem/s 2.3994 Kelem/s]
vector_bulk_load/batch/n1000
                        time:   [100.20 ms 124.77 ms 152.08 ms]
                        thrpt:  [6.5754 Kelem/s 8.0147 Kelem/s 9.9799 Kelem/s]
vector_bulk_load/serial/n10000
                        time:   [11.096 s 12.522 s 15.254 s]
                        thrpt:  [655.55  elem/s 798.61  elem/s 901.23  elem/s]
vector_bulk_load/batch/n10000
                        time:   [999.10 ms 1.0198 s 1.0383 s]
                        thrpt:  [9.6314 Kelem/s 9.8060 Kelem/s 10.009 Kelem/s]
vector_compaction/rebuild_aside/d30/n10000
                        time:   [735.42 ms 744.35 ms 752.78 ms]
                        thrpt:  [9.2989 Kelem/s 9.4042 Kelem/s 9.5184 Kelem/s]
vector_compaction/rebuild_aside/d50/n10000
                        time:   [484.99 ms 495.64 ms 506.74 ms]
                        thrpt:  [9.8670 Kelem/s 10.088 Kelem/s 10.310 Kelem/s]
```
