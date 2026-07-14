# Vector benchmarks

Canonical home for **vector-index benchmark reports** — the artefacts produced by
`scripts/bench-vector.sh` (which drives both the V0.3 criterion latency bench and
the V0.4 `vector_report` recall/RSS tool).

## Reports

- `2026-07-05-filtered-ann.md` — Pre/co/post-filter selectivity crossover bench (V3.3).
- `2026-07-05-bulk-compaction.md` — Bulk-load (serial vs batch) + rebuild-aside compaction cost bench (V4.3).

## File naming

`<YYYY-MM-DD>-baseline.md` — one file per dated baseline run. The date is UTC
(the `vector_report` example stamps it into the report header). Append
`-smoke` / `-full` / `-1m` suffixes for non-default tiers so the default
baseline stays the canonical quick-run snapshot.

## Report format

Each report is a markdown block **emitted verbatim** by the tool(s):

1. A **reproducibility header** — the `(n, dim, k_clusters, σ, seed)` key, HNSW
   params, host (OS/arch/threads), and tool version. This is the
   "Release Benchmark Checklist" key from
   `docs/dev-artifacts/roadmap/VECTOR_PRODUCTION_EXECUTION.md`.
2. A **markdown table** with one row per `(dim, metric)` cell:
   `recall@1`, `recall@10`, build time (wall), peak RSS.
3. The criterion **latency** table (mean / p99 from `benches/vector_search.rs`),
   appended below by `bench-vector.sh`.

Example skeleton:

```markdown
## Vector baseline — 2026-02-14

- **Tool**: `cargo run --release --example vector_report` (V0.4)
- **Dataset**: `clustered_vectors` — n=10000, dims=[128, 768], k_clusters=64, σ=0.1, seed=42
- **Queries**: 100 (seed=43)
- **HNSW**: M=16, max_layer=16, ef_construct=200, ef_search=50
- **Host**: linux/x86_64, 16 threads

| dim | metric | recall@1 | recall@10 | build (s) | peak RSS |
|----:|:-------|--------:|----------:|----------:|---------:|
| 128 | cosine | 1.000   | 0.998     | 0.42      | 28.3 MiB |
| 768 | cosine | 1.000   | 0.991     | 2.11      | 64.1 MiB |
| 128 | l2     | 1.000   | 0.997     | 0.40      | 28.1 MiB |
| 768 | l2     | 1.000   | 0.989     | 2.07      | 63.8 MiB |
```

## Running

```bash
# Full pipeline (criterion + vector_report), isolated target dir:
./scripts/bench-vector.sh

# Smoke tier (fastest, CI-friendly):
BENCH_SMOKE=1 ./scripts/bench-vector.sh

# 1M rung (long; opt-in):
BENCH_VECTOR_1M=1 ./scripts/bench-vector.sh

# Just the recall/RSS report, no criterion:
cargo run --release -p shamir-engine --example vector_report
```

## Why two tools

- **`benches/vector_search.rs` (V0.3, criterion)** — measures **latency** (mean
  p50/p99) of HNSW vs BruteForce top-k search. Criterion's statistical machinery
  (warm-up, sample loop, outlier detection) is what makes latency numbers
  trustworthy.
- **`examples/vector_report.rs` (V0.4)** — measures **quality** (recall) and
  **memory** (RSS). Recall is deterministic (no noise to average out), and RSS
  is an OS-level stat criterion's `Measurement` trait doesn't model — so this is
  a plain example binary, not a criterion bench.

Both consume the **same** `clustered_vectors` generator and the **same** HNSW
params, so a latency number and a recall number for the same `(n, dim, metric)`
cell are directly comparable across the two tools.

**RSS caveat.** The cells run sequentially in one process, so the `peak RSS`
column is process-wide at that point, not per-cell independent — rows ≥2 include
the memory of earlier cells' indexes. Treat RSS as an order-of-magnitude figure
per `(dim)` scale, not a precise per-cell delta.
