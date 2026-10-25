# P0 stage L3b — p0-index-promote-order

## Goal

Two independent rush reviewers and @oh CONFIRMED (unanimous, REAL) a vector-index correctness defect at master d681e98c (after PR #294): graph promotes of DIFFERENT transactions touching the SAME rid are not ordered. Each transaction appends its durable delta chunk (per-backend `next_delta_idx.fetch_add`) and publishes, then promotes into the live in-RAM HNSW graph LATER and independently (lockfree path: inline in the committing task via `finalize.rs` ~:104 after all guards are dropped; legacy async path: inside the spawned tail `legacy_and_lockfree.rs` ~:141/:162). Transaction B can start after A published (so no write conflict), append chunk i+1, publish and promote BEFORE A promotes, then A's stale mutation overwrites it. Nothing orders them: `apply_staged_vectors`/`apply_staged_vector_deletes` (backend.rs ~:552/:585) apply unconditionally with no version/index check; `enter_write` is only a shared admission lease; `claim_rid` only detects overlap during compaction mirroring; #294's `ack_delta_applied` orders only the prune boundary.

Effects: upsert/upsert -> stale embedding ranks the row; upsert/delete -> deleted rid stays live in the graph; delete/upsert -> an existing row is missing from vector search. After both acks a later snapshot persists the stale graph and prunes both corrective chunks, so a restart does NOT repair it (only a rebuild from the table does). Verify each fact against HEAD before editing.

## Binding design (@oh's turnstile, adopted)

A per-backend PROMOTE TURNSTILE keyed by the delta chunk index, separate from `applied_prefix`:
- New backend state: `promote_turn: AtomicU64` (the next chunk index allowed to promote), a lock-free ordered set (`scc::TreeIndex<u64, ()>`) of finished indexes ahead of the turn, and a `tokio::sync::Notify`. Seed them exactly where `applied_prefix` is seeded (restore_on_open / construction / post-snapshot).
- `append_vector_delta` returns a `PromoteTicket { backend, idx }` (instead of / in addition to the #294 acknowledgement closure) whose `Drop` releases the turn - covering success, a failed promote, a failed delta append, and a cancelled commit future. Releasing the turn on a failed promote does NOT acknowledge the chunk (the #294 prefix stays below it: the chunk remains replayable); it only lets later chunks proceed.
- In `apply_vector_graph_batch` (materialize.rs ~:504-545): `ticket.wait_turn().await` BEFORE `enter_write`, then apply the batch's deletes and inserts as ONE ordered unit inside the turn (this also closes the delete-half/insert-half interleaving between transactions), acknowledge only on success (#294 rule unchanged), release the turn on every outcome. A ticket only waits on tickets reserved EARLIER on the same backend; no commit lock, unique-write lock, drain guard or lease may be held while waiting (verify this on both commit paths) - no deadlock. Do NOT gate on `applied_prefix` (a failed promote never acks and would hang every later promote).
- Compaction mirroring (`claim_rid`, target mirroring) must keep working: the mirrored apply happens inside the same turn.
- Hot-path cost: no contention -> one Acquire load + a CAS and a notify on release; under same-table contention promotes on that backend run one at a time in chunk order (document it). Do not add a mutex.

## Tests (names contain `rv_index_l3b_promote_order`)

Deterministic, no sleeps: (a) engine-level: use the existing one-shot hook pattern (`broadcast_armed` / `reached` / `resume`, see `journal_publish_order_tests.rs` ~:68) to park transaction A between publish and promote, run transaction B (same rid, newer vector) to the point where it must wait at the turnstile (add a test-only `turn_wait_hook` fired when a promote has to wait, use `notify_one` so the permit cannot be missed), resume A, join both, assert the live graph holds B's vector for the rid; then force a snapshot (the #294 `snapshot_now` seam), reopen a fresh backend and assert it is still B's vector. (b) upsert-then-delete: the rid is ABSENT from the graph at the end. (c) delete-then-reinsert: the rid is present with the newest vector. (d) a failed promote (existing `FAIL_VECTOR_PROMOTE` failpoint) releases the turn: a later transaction still promotes (no hang) and the failed chunk stays unacknowledged. (e) a dropped/cancelled ticket releases the turn. Each test must fail with only its own fix reverted (state which revert).

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
