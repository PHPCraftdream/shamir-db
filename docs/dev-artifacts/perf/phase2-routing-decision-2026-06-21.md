בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase 2 routing decision — next campaign

Per workflow logic in `perf-phase1-execution-2026-06-21-wf_*.js`:

> - fast_path result='confirmed' (bail still on HEAD) → next op = `fast_path_fix`
> - fast_path result='denied' (already fixed) → next op = `drainer_cursor`

## Phase 0b finding (cached)

`read_path_matrix/fast_path/1_000_000` = **138.32 µs** (median), CI [134.83 µs, 143.01 µs].
This is sub-ms — the sorted-index fast path IS active. Bail was fixed by #128/#130.

**Result: 'denied' — no fast_path campaign needed.**

## Recommendation

**Next campaign = Op #2 incremental drainer cursor** (roadmap §3 row #2, ROI 82).

### Why

- ROI 82 (next highest after Phase 1 was completed).
- Drainer `wal.recover()` re-reads + re-decodes + re-sorts the ENTIRE WAL on every commit-wake — latent O(N²) over WAL depth between truncations.
- THE dominant cost on the durable write path under load.
- Commit path already holds the `WalEntryV2` — push to a lock-free queue, fall back to `recover()` only on cold start.
- Unlocks downstream ops #5 (FTS top-K) and #8 (interner spine — already done as Op B).

### Scope

- Effort: **large** (xlarge boundary). Multi-file: `crates/shamir-engine/src/tx/commit.rs` (phases 1-7), `crates/shamir-tx/src/mvcc_store/`, `crates/shamir-wal/src/`, `crates/shamir-storage/src/storage_membuffer.rs` (drainer).
- Risk: **medium**. Cold-recovery contract: must remain byte-identical (B5 from roadmap §6).
- ⛔ **DO NOT** pursue single-writer-task WAL rewrite — already prototyped & reverted (+22% mem latency).

### Pre-flight (when launching Op #2 campaign)

1. Capture WAL-depth + drainer throughput baselines via existing `drain_throughput.rs` + `durable_concurrent_commit.rs` benches.
2. Identify the `WalEntryV2` shape passing through commit phase 4 → drainer.
3. Design the lock-free queue (mpmc? mpsc? what frees memory?) before code.

## Out of scope for this decision

Phase 2 is routing-only. Op #2 campaign is **not started** here — needs explicit user authorization (xlarge effort, multi-week potentially).

## Phase 1 wins summary (for context)

| Op | Speedup | Commit |
|---|---|---|
| A — fjall scan_prefix_stream | 164× | 98f256d |
| A.2 — sled scan_prefix_stream | 50× | 7140f82 |
| B — interner Arc<str> spine | 4× @ N=5k (grows linearly) | 35ebd40 |
| C — MemBuffer *_many sentinel | correctness + regression test | d2d3504 |

All four committed; gate green; tests pass. Total wall-clock for Phase 1: ~2 hours (Op A by sh-agent + Op A.2/B/C in main thread after workflow agents failed prompt discipline).
