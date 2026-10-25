# P0 stage L3 — p0-index-delta-prune-race

## Goal

Two independent rush reviewers (and @oh) CONFIRMED a recovery-correctness defect in the vector index at master ada7fdd7: a durable vector delta chunk can be pruned by `run_background_snapshot` before its mutation reaches the dumped graph, so a restart loses the vector. Mechanism (verify against HEAD before editing):

1. Writer appends a delta chunk `i` (`next_delta_idx.fetch_add(1)` happens before the storage await) — `crates/shamir-index/src/vector/vector_backend/backend.rs` ~:897-908; the engine then triggers the snapshot check immediately after the append and BEFORE graph promotion — `crates/shamir-engine/src/tx/commit/pipeline/phase/commit_phases.rs` ~:915-919; graph promotion runs later in the spawned tail — `.../commit/legacy_and_lockfree.rs` ~:90-162.
2. The snapshot task samples `next_delta_idx` as `delta_applied_upto` BEFORE dumping the live graph (`vector_backend/maintenance.rs` ~:71-119), dumps a graph that does not contain the new RID, flips the manifest and prunes delta indices `< delta_applied_upto` (exclusive bound: `snapshot/load.rs` ~:471-479, :630-650), including chunk `i`.
3. Recovery loads snapshot + remaining deltas (`backend.rs` ~:759-838, `snapshot/load.rs` ~:547-570); the RID is gone. The same wrong boundary is used by the post-compaction snapshot (`maintenance.rs` ~:377-387). The comment at `snapshot/load.rs` ~:453-462 claiming graph apply precedes delta append is stale.

## Binding design (both reviewers agree)

- The boundary that authorises pruning is a per-backend CONTIGUOUS APPLIED PREFIX owned by the vector backend in `shamir-index`, never the allocation counter:
  1. Each successful durable chunk append yields a token (the chunk index).
  2. Graph promotion acknowledges the token ONLY after all its deletes and upserts succeeded.
  3. The boundary advances only across a contiguous acknowledged prefix (never `fetch_max(completed+1)`: completion of `k+1` must not cover unfinished `k`); failed, cancelled or pending promotion leaves its chunk replayable (a failed/never-acknowledged token must not wedge forever either: define and document the rule for a permanently failed promotion — the chunk stays replayable and the boundary stays below it).
  4. The snapshot samples this boundary BEFORE dumping and uses exactly that sampled value both for `manifest.delta_applied_upto` and for pruning.
  5. The post-compaction snapshot caller uses the same boundary.
- Keep HNSW work outside the engine commit lock. The engine carries the append token through promotion (narrow plumbing between the engine commit tail and the backend). Rejected alternatives: moving the trigger after this writer's promotion (another writer may have appended but not applied); sampling `next_delta_idx` after the dump.
- Fix the stale comment. No migration code (alpha policy); existing on-disk manifests stay readable (the field meaning is unchanged: indices below it are absorbed).

## Tests (names contain `rv_index_l3_delta_prune`)

Deterministic, no sleeps: (a) append a chunk, run the snapshot BEFORE acknowledging the token (use a barrier/hook at the append->apply seam), assert the chunk survives pruning and a reload from snapshot+deltas contains the vector; (b) out-of-order acknowledgement (k+1 acked, k pending) leaves chunk k unpruned; (c) after acknowledgement the chunk is absorbed and pruned; (d) failed promotion keeps the chunk replayable; (e) the post-compaction snapshot respects the same boundary. Each test must fail with only its own fix reverted (state which revert).


## Additional binding points from the third opinion (@oh)

- File paths above come from two reviewers and may differ from HEAD (@oh found the logic in `crates/shamir-index/src/vector/vector_backend.rs` ~:891-965 and `snapshot.rs` `flip_generation`/`replay_delta`); locate the real files first.
- `append_vector_delta` returns the reserved chunk index (`Option<u64>`); if the delta write itself FAILS, mark that index applied so the hole does not stall the prefix (nothing was persisted for it). New backend hook `mark_delta_applied(idx)` (default no-op on the trait) inserts the index into a lock-free ordered set (`scc::TreeIndex<u64, ()>`) and advances `applied_prefix` over the contiguous run with a CAS loop. The engine keeps `(backend, idx)` per transaction (e.g. in `TxContext`) and calls `mark_delta_applied` only after a successful `apply_staged_vector_deletes` + `apply_staged_vectors`. A failed promote (`FAIL_VECTOR_PROMOTE`) never marks its chunk. `restore_on_open` sets `applied_prefix = hwm + 1` next to `next_delta_idx`. Compaction also reads `next_delta_idx`: switch it to `applied_prefix`.
- Test seam: `#[cfg(test)] pub(crate) async fn snapshot_now_for_test(...)` that awaits the snapshot inline. Required scenario (in `delta_log_tests.rs`): seed base snapshot + `restore_on_open`; `append_vector_delta` of rid X WITHOUT graph apply; run the snapshot; then apply the graph and mark applied; open a fresh backend and assert top-k returns X. Second case: skip the graph apply (failed promote) and assert X is still recovered by delta replay on restart. The existing test at `delta_log_tests.rs` ~:171-176 uses the old order (graph first, then delta) — keep it, add the new ones.
- Out of scope (separate finding, do NOT fix here): post-lock promotes of different transactions on the same rid can reach the live graph in the opposite order to their delta chunks (live graph diverges until restart; replay restores order). Mention it in the final message.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
