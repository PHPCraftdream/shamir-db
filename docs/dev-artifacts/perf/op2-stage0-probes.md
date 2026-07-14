בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Op #2 Stage 0 — Probes + Baselines

## P1 — commit_version density / ordering

**Verdict: OUT-OF-ORDER-POSSIBLE (dense counter, no gaps, but WAL finalization order is non-deterministic)**

**Evidence:**

`commit_version` is allocated by `RepoTxGate::assign_next_version` (`crates/shamir-tx/src/repo_tx_gate.rs:285`):

```rust
pub fn assign_next_version(&self) -> u64 {
    self.version_counter.fetch_add(1, Ordering::Relaxed) + 1
}
```

This is a bare `AtomicU64::fetch_add(1)` — every call gets a unique, strictly incrementing value. There are **no gaps**: aborted txs' versions are marked `Aborted` in the `CompletionTracker` via `VersionGuard::drop` (`crates/shamir-tx/src/version_guard.rs:100-115`), and the contiguous watermark advances past them. The drainer window filter (`commit_version > dur && commit_version <= vis` at `crates/shamir-engine/src/tx/drainer.rs:139`) only sees entries that made it into the WAL (successful Phase 4), so aborted versions never appear in the WAL at all.

However, the **lockfree commit path** (`commit_tx_lockfree` at `crates/shamir-engine/src/tx/commit.rs:592`) calls `pre_commit_locked_validate` WITHOUT holding `commit_mutex`. Two concurrent lockfree committers call `assign_next_version` (atomic fetch_add) and get versions N and N+1, then race to `wal.begin_grouped()` independently (`commit.rs:633`). The committer with version N+1 can finalize its WAL entry before version N. The WAL `recover()` returns entries in WAL-key order (txn_id), not commit_version order — that is why `drainer.rs:134` and `recovery.rs:250` both explicitly `sort_by_key(|e| e.commit_version)`.

Additionally, the **group-commit path** (`run_leader` at `crates/shamir-engine/src/tx/group_commit.rs:59`) processes multiple txs under one `commit_mutex` hold. Versions are assigned sequentially within the batch (`pre_commit_locked_validate` called in a loop, `group_commit.rs:159`), but the batch as a whole races with concurrent lockfree committers on other tables.

**Confidence: HIGH** — the code paths are unambiguous. The `fetch_add` guarantees density, but WAL persistence order is demonstrably non-deterministic across concurrent lockfree committers.

**Stage 1 implication: TreeIndex required.** A FIFO would only work if entries arrived in commit_version order. Since lockfree commits can finalize WAL entries out of order, the drainer's incoming stream is unordered. `scc::TreeIndex<u64, Arc<WalEntryV2>>` is the correct structure — it sorts by commit_version on insert and allows the drainer to consume the contiguous prefix.

## P2 — Baseline measurements

### Bench: drain_throughput.rs

This bench measures concurrent-commit ack-throughput (N writers, fresh repo per iteration). It does NOT isolate drain cost — it measures the full commit pipeline including WAL + overlay + drainer wake. The drain runs in the background; the bench measures commit latency, not drain throughput per se.

| Backend | Writers | Throughput (elem/s) | Time/sample (ms) | Notes |
|---------|---------|--------------------:|------------------:|-------|
| fjall   | 8       | 228 - 238           | 33.7 - 35.1      | |
| sled    | 8       | 1,869 - 3,147       | 2.5 - 4.3        | high variance |
| fjall   | 32      | 816 - 851           | 37.6 - 39.2      | |
| sled    | 32      | 5,289 - 6,959       | 4.6 - 6.1        | high variance |
| fjall   | 128     | 2,449 - 2,641       | 48.5 - 52.3      | |
| sled    | 128     | 8,633 - 11,767      | 10.9 - 14.8      | high variance |

### Bench: durable_concurrent_commit.rs

| Pattern | Writers | Throughput (elem/s) | Time/sample (ms) | Notes |
|---------|---------|--------------------:|------------------:|-------|
| same_table    | 1   | 27 - 29       | 34.4 - 37.7  | single-writer baseline |
| same_table    | 8   | 199 - 219     | 36.5 - 40.1  | |
| same_table    | 32  | 803 - 846     | 37.8 - 39.9  | near-linear scaling from N=1 |
| disjoint_tables | 1 | 25 - 28       | 36.1 - 39.4  | ~same as same_table/1 |
| disjoint_tables | 8 | 117 - 123     | 65.1 - 68.4  | SLOWER than same_table/8 |
| disjoint_tables | 32 | 387 - 415    | 77.2 - 82.7  | SLOWER than same_table/32 |

**Observed shape: sub-linear scaling on fjall** — throughput grows with N but per-commit cost is dominated by per-repo WAL + fjall overhead, not per-table contention. The disjoint_tables path is paradoxically SLOWER than same_table at N=8,32 — this is because disjoint tables provision N table managers (DDL overhead per iteration) and each lockfree committer does an independent `wal.begin_grouped` (no fsync amortization from group-commit).

**Key observation:** Neither bench directly measures the O(W^2) `recover()` cost because they use fresh repos per iteration (WAL depth W=N at most). The quadratic cliff would manifest only with accumulated WAL depth across many drain cycles without truncation — i.e. when the drainer falls behind. The existing benches prove the COMMIT path scales, but do not capture the DRAIN degradation that Op #2 targets.

## P3 — History-write topology on HEAD

**Call sites of `write_committed_batch_to_history`:**

- `crates/shamir-engine/src/tx/drainer.rs:251` — Phase B of `drain_step`: the drainer writes batched history per table.

**Call sites of `write_committed_to_history`:**

- `crates/shamir-engine/src/tx/recovery.rs:413` — recovery replay (startup only, not hot path).

**Call sites of `apply_committed_visible` (the inline commit-path write):**

- `crates/shamir-engine/src/tx/commit_phases.rs:357` — Phase 5a of `apply_data_batch`: writes ONLY the in-memory overlay (visible half). The comment at `commit_phases.rs:349-355` is explicit:

> D2 P1d-2b CUTOVER: the ack-path writes ONLY the in-memory visible half (overlay + cell + floor). It no longer writes `history` inline — the background `Drainer` [...] is now the SOLE history writer.

**Verdict: DRAINER-ONLY (no double-write)**

The inline commit path (Phase 5a) calls `mvcc.apply_committed_visible()` which writes only the in-memory overlay. It does NOT call `write_committed_to_history` or `write_committed_batch_to_history`. The D2 P1d-2b cutover already separated the concerns: commit writes overlay, drainer writes durable history.

**Stage 3 implication: drainer-side only.** There is no double-write to collapse. The speedup target is purely about removing `recover()` from the drain loop — replacing the full WAL re-read + CRC-decode + sort with an incremental cursor that consumes pre-sorted entries from the commit path.

## Open questions

- **No WAL-depth bench exists.** Neither `drain_throughput.rs` nor `durable_concurrent_commit.rs` measures drain cost as a function of WAL depth W. They use fresh repos per iteration, so W never exceeds N. To prove the O(W^2) -> O(1) improvement, Stage 5 will need a new bench that accumulates W entries before triggering drain, or measures drain latency at varying W. Consider adding a `drain_cost_vs_depth` bench in Stage 5.

- **disjoint_tables slower than same_table.** At N=8,32 the disjoint path is ~1.7-2x slower. This is likely DDL overhead (provisioning N table managers per iteration) + loss of group-commit fsync batching. Not directly related to Op #2 but worth noting: the lockfree path's independent WAL writes sacrifice batching.
