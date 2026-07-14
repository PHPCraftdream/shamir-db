בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase 0c — Baselines (2026-06-21)

Targeted baselines for Phase 1 ops. Captured manually after o46l agent failed prompt discipline (twice) — see #154 for the bench-infra bug that caused the original livelock.

Mode: `BENCH_SMOKE=1` (note: `store_raw` uses raw Criterion defaults, not `shamir_bench_utils::tune_tiered` — SMOKE env is ignored. Sample size shown reflects this).
Target dir: `D:/dev/rust/.cargo-target-bench`.
read_path_matrix intentionally skipped — known hard cell `s3_range_and/1M` would hang (~3h), tracked in #154.

---

## store_raw — baseline for Op A (scan_prefix_stream)

| Backend | Op | Time (median) | Throughput | Notes |
|---|---|---|---|---|
| in_memory | insert/single | (see full log) | — | |
| in_memory | get/single | — | — | |
| **fjall** | **insert/single** | **91.7 µs** | **10.9 K/s** | |
| **fjall** | **get/single** | **57.7 µs** | **17.3 K/s** | |
| **fjall** | **scan/iter_stream/1000** | **621.4 ms** | **1.61 K/s** | nearby sibling of Op A target |
| **fjall** | **set_many/batch/100** | **8.86 ms** | **11.3 K/s** | |
| sled | scan/iter_stream/1000 | 391.3 ms | 2.56 K/s | reference: 1.6× faster than fjall — fjall has the cliff Op A targets |
| sled | get/single | 52.9 µs | 18.9 K/s | comparable to fjall |
| sled | set_many/batch/100 | 6.14 ms | 16.3 K/s | |

**Note for Op A:** the existing `store_raw` bench has NO `scan_prefix_stream` cell. The bench shows `scan/iter_stream` only. Op A agent MUST add a `prefix_scan_50k_fjall` cell (50k-row shared-prefix scan_prefix_stream) BEFORE measuring — that's the Phase 1 step 1 (baseline). The 621ms `scan/iter_stream/1000` cell is provided here as the closest sibling reference but is NOT the Op A target.

---

## interner_cold_growth — baseline for Op B (Arc<str> reverse-spine)

| Cell | Time (median) | Bytes ratio (old/new) | Notes |
|---|---|---|---|
| interner_cold_growth_bytes/noop/1000 | 33.0 ns | 380.9× | already-optimized noop probe; shows current Arc-saving impact |
| interner_cold_growth_bytes/noop/5000 | 31.6 ns | 1924.7× | grows with N — exactly what Op B targets (O(N²) → O(N)) |

The bench only has 2 cells. Op B agent should ADD scaling cells at N={20_000, 50_000} to capture the O(N²) cold-growth curve that the Arc<str> change is meant to flatten. Current `bytes_written` ratio already shows a prior optimization (380× at N=1k → 1924× at N=5k); Op B should ratio-check after-change vs this baseline.

---

## Phase 1 readiness summary

| Op | Target file | Bench cell to use | Baseline value | Status |
|---|---|---|---|---|
| A — scan_prefix_stream range-seek | `storage_fjall.rs` | `prefix_scan_50k_fjall` (TO ADD) | TBD by agent (closest sibling: 621ms iter_stream/1000) | needs new bench |
| B — interner reverse-spine Arc<str> | `shamir-types/.../interner.rs` | `interner_cold_growth_bytes/noop/N` (extend N) | 33ns @ N=1k, 32ns @ N=5k | extend N range |
| C — MemBuffer insert_many sentinel | `storage_membuffer.rs` | regression test (not perf) | n/a | correctness only |

---

## Methodology notes / blind spots

- `store_raw` bench uses Criterion defaults (sample=100, measurement=5s) — `BENCH_SMOKE=1` not honored. Heavy fjall cells (scan, set_many) extended Criterion measurement to ~60s/cell. Total bench: ~10 min.
- `interner_cold_growth` honored sample=10 in SMOKE — but only 2 cells. The full bench is small.
- Both benches showed "Performance has regressed" against previous saved baseline (`base` storage saved). Investigate independently if these regressions are real or environment noise (CPU thermal throttling on this Windows machine during the 0c+1A campaign).

Source raw logs:
- `/tmp/baseline-store-raw.log`
- `C:/Users/Computer/AppData/Local/Temp/.../bcqpebo1w.output`
