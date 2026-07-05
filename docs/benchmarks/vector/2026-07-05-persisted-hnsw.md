<!-- persisted_hnsw — paste into docs/benchmarks/vector/2026-07-05-persisted-hnsw.md -->

## Persisted HNSW cold-start — 2026-07-05

- **Tool**: `persisted_hnsw` example binary, V2.4 (build with cargo, run the
  artefact directly — the perimeter guard blocks `cargo run`)
- **Dataset**: `clustered_vectors` — n=100000, dim=128, k_clusters=64, σ=0.1,
  seed=42
- **HNSW**: M=16, max_layer=16, ef_construct=200, ef_search=50
- **Metrics**: cosine, l2
- **Host**: windows/x86_64, 16 threads

### Cold-start wall-time: `load_snapshot` vs full-scan `rebuild`

The two paths `VectorBackend::restore_on_open` can take on a restart, timed
end-to-end (graph construction included):

- **load** — a valid snapshot exists → `load_snapshot` rebuilds the graph from
  the dumped chunks + sidecar. `rebuild_count == 0` (proven by the
  `load skipped scan? = yes` column — no data-store scan ran).
- **rebuild** — no snapshot → `rebuild` scans every row in the data store and
  re-inserts each vector into a fresh graph. `rebuild_count == 1`.

| n | dim | metric | load (s) | rebuild (s) | speedup | load skipped scan? |
|---:|----:|:-------|---------:|------------:|--------:|:-------------------|
| 100000 | 128 | cosine | 3.448 | 16.381 | 4.75× | yes |
| 100000 | 128 | l2 | 3.247 | 14.503 | 4.47× | yes |

- **DoD P2**: restart of a 100000-row index without a full data-store scan —
  **MET** (load path `rebuild_count == 0` for every cell). The P2
  persistence stack (snapshot codec V2.1 + startup integration V2.2 +
  delta-log/generation-flip V2.3) lets a warm restart skip the O(rows × dim)
  scan entirely and re-acquire the graph in O(dump size).
- **Savings**: the load path skips scanning all 100000 rows; on this host it
  is ~4.8× faster than the rebuild scan at dim=128. The absolute gap widens
  with row count (rebuild is O(N), load is O(dump-size) ≈ O(N) but with a
  far smaller constant — no per-row `extract_vec` + per-vector graph insert;
  instead one batched `hnsw_rs` file-load under `spawn_blocking`).
- **Why load is not ~instant**: the dominant cost is `hnsw_rs`'s own graph
  rehydration (`HnswIo::load_hnsw_with_dist` reads both dump files into
  memory and rebuilds the layer/ neighbour structure), which runs regardless
  of whether the rows came from a scan or a dump. The win is that load does
  NOT pay the `iter_stream` scan + per-vector `upsert_batch` cost the rebuild
  path pays on top.

### Crash-recovery coverage (V2.4)

The cold-start fast path is only safe if a CORRUPT snapshot falls back to
the rebuild path without aborting the open. V2.4's `crash_recovery_tests.rs`
proves every corruption mode routes through `restore_on_open`'s warn+rebuild
arm with `rebuild_count == 1` and the user's data intact:

- truncated chunk (payload bytes truncated, crc mismatch) → `Corrupt` → rebuild
- corrupt manifest (garbage bytes) → `Corrupt` → rebuild
- `hnsw_rs` version mismatch in the sidecar → `VersionMismatch` → rebuild
- e2e restart preserves recall@10 (10k vectors, recall ≥ 0.90 floor vs
  brute-force ground truth)

### Reproducibility key

- `cargo build --release --example persisted_hnsw`
- `./target/release/examples/persisted_hnsw` (QUICK: n=100000, dim=128)
- `PH_N=1000000 PH_DIM=128 ./target/release/examples/persisted_hnsw` for the
  1M long rung (env-gated via `PH_N_1M=1` — not in the default DoD; run
  only when a 1M number is explicitly requested).
- `./scripts/test.sh @vector --full` covers the crash-recovery + recall tests.

### Phase P2 — closed

With V2.4, phase P2 (persisted HNSW) is complete:

| Sheet | Delivered |
|:------|:----------|
| V2.1 (c80d99f9) | snapshot codec — dump/load, crc32 per chunk, MetaEnvelope sidecar, version checks |
| V2.2 (6596ac24) | startup integration — `restore_on_open` snapshot-first / rebuild-fallback, `rebuild_count` |
| V2.3 (a33cc120) | delta-log + generation flip + background snapshot (single-flight, threshold-triggered) |
| V2.4 (this sheet) | crash/corruption tests on the open path, e2e restart-preserves-recall, cold-start bench, P2 report |

**Open follow-up (deferred, not blocking P2 close)**: gap#1 — tx-path vector
deletes are not yet threaded into `append_vector_delta` (`commit_phases.rs`
passes `deleted = &[]`). The mechanism is proven by
`delta_log_tests::append_vector_delta_with_deleted_slice_persists_and_replays_delete`;
the wiring is a multi-layer tx-context change that P2-closing sheet chose not
to risk. See `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md` § "P2 follow-ups".
