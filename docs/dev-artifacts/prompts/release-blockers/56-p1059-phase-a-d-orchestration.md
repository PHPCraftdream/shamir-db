# Brief 56 — #1059: orchestrate Phase A→D for regular/hash CREATE INDEX

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

This is the largest, highest-risk slice of the online CREATE INDEX redesign
(RFC v2, `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`,
approved 2026-08-09). Take your time; correctness matters far more than
elegance here. If something in this brief conflicts with what you find in
the actual code, the code wins — re-verify every citation before relying on
it, several things have shifted across the last 3 slices.

## What you have to work with (all landed and tested — do not modify their tests)

- `MvccStore::snapshot_stream(batch, at_version)` (#1056,
  `crates/shamir-tx/src/mvcc_store/mod.rs`) — version-pinned, tombstone-
  suppressing scan.
- `TableManager::open_index_build_snapshot(&self) -> Option<SnapshotGuard>`
  (#1057, `crates/shamir-engine/src/table/table_manager.rs`) — `async fn`,
  pins at `guard.version()`. Returns `None` only if `changefeed` isn't
  wired (system tables / raw test tables — treat as "online build
  unavailable, caller should fall back to the old whole-barrier path" for
  such tables).
- `IndexManager::mark_build_in_flight(name_interned)` /
  `is_build_in_flight(name_interned) -> bool` /
  `clear_build_in_flight(name_interned)` (#1058,
  `crates/shamir-index/src/base_index/index_manager.rs`) — now `pub`,
  lock-free (`scc::HashMap`). Dirty-set capture already happens
  automatically inside `plan_record_created`/`plan_record_updated`/
  `plan_record_deleted` once `mark_build_in_flight` has been called for a
  `Building` def — you don't need to touch that capture logic.

## A gap you must close first — #1058 has no public dirty-set DRAIN

`IndexManager`'s `dirty_sets` field and `get_or_create_dirty_set` are
private/`pub(super)` — Phase C (which you're writing, likely from
`shamir-engine`, a different crate) cannot reach them directly. Add a public
method on `IndexManager`, e.g.:

```rust
/// Drain the dirty-set for `name_interned`, returning every RecordId
/// captured since the last drain (or since the build started, for the
/// first drain). Returns an empty Vec if nothing was captured or the
/// index isn't in-flight. Draining clears the set — a RecordId captured
/// AFTER this call returns is not lost, it accumulates for the NEXT drain.
pub fn drain_dirty_set(&self, name_interned: u64) -> Vec<RecordId> {
    let mut dirty = self.dirty_sets.lock().unwrap();
    match dirty.get(&name_interned) {
        Some(set) => {
            let mut inner = set.lock().unwrap();
            std::mem::take(&mut *inner).into_iter().collect()
        }
        None => Vec::new(),
    }
}
```

Adjust to match `dirty_sets`' exact current type (verify against
`index_manager.rs` — it may have changed slightly). Add at least one unit
test for this: capture some ids, drain, confirm drain returns them and a
second immediate drain returns empty; capture more after the first drain,
confirm the second drain sees only the new ones.

## The new orchestration entry point

**Do NOT modify `create_index_from_records`/`create_index_from_stream`** —
their doc comments explicitly state they must stay byte-for-byte identical
for the F-78 correctness-equivalence test (materialize-vs-stream postings
must match). Add a NEW method — your call on where it lives (a new
`IndexManager` method callable from `TableManager::create_index`, or the
orchestration living directly in `TableManager::create_index`'s body calling
smaller new `IndexManager` primitives — whichever keeps the phase boundaries
clean). `TableManager::create_index(name, paths)`'s PUBLIC signature stays
unchanged; only regular/hash CREATE INDEX now runs the new sequence
internally.

**Reusable pieces already in `create_index_from_records`/
`create_index_from_stream`** (`crates/shamir-index/src/base_index/index_manager.rs:1523-1662`
and `:1719+`) — read both in full before writing anything:
- Phase 1 (register at Building): `self.indexes.add_index(index_def);
  self.bump_generation(); self.has_indexes.store(true,
  Ordering::Release); self.save_index_info().await?;` (`:1544-1558`). You
  need this exact sequence for Phase B — either factor it into a small
  shared private helper both the old and new paths call, or duplicate the
  ~5 lines (your call — factoring is cleaner but duplication is lower-risk
  if you're not fully confident the two call sites need IDENTICAL
  semantics; state which you chose and why).
- Phase 2's batch-write body (the `while let Some(batch) = stream.next()`
  loop in `create_index_from_stream`, `:1727+`) is the template for Phase
  A's backfill loop — same shape, different stream source
  (`snapshot_stream` instead of `list_stream`).
- Phase 3 (flip to Ready): `:1636-1654` — reuse for Phase D's final flip,
  after applying the final dirty-set residual.

## The sequence to implement

**Phase B (micro-barrier):**
1. `begin_write_barrier(REGULAR_INDEX_CREATE)` — same call, same order as
   today (`crates/shamir-engine/src/table/table_manager.rs`'s
   `begin_write_barrier`; the admission→bit→drain→lock order is load-
   bearing, see `writer_drain_barrier.rs:50-146` and the proven deadlock in
   `f70_lock_order_inversion_tests.rs` for the reverse order).
2. Under the barrier: acquire `open_index_build_snapshot()`. If `None`
   (no changefeed), fall back to today's whole-barrier
   `create_index_from_stream` path entirely (don't attempt online build for
   such tables) — hold the SAME barrier you already have and just call the
   existing method, then return.
   If `Some(guard)`: pin = `guard.version()`. Register the index at
   `Building` (Phase 1 sequence above). Call `mark_build_in_flight(name_interned)`.
3. Release the barrier (drop `_barrier`/`_uwl_guard`) — do NOT drop the
   `SnapshotGuard` yet, it must live through Phase A.

**Phase A (barrier-free scan):**
4. Stream `mvcc_store().snapshot_stream(batch, pin)`, adapt each batch the
   same way `create_index`'s existing `list_stream` adaptation does
   (`table_manager_index_mgmt.rs:707-713` — `RecordCow::into_inner()`).
   Write postings via the same batched `set_many` pattern as Phase 2's body.
   Drop the `SnapshotGuard` once this stream is fully drained (scan done).

**Phase C (barrier-free catch-up loop):**
5. Loop: `drain_dirty_set(name_interned)`. If empty on a full iteration →
   converged, proceed to Phase D. Otherwise, for each drained `RecordId`:
   read the record at CURRENT version (not the pin) — find the right
   accessor (`MvccStore::get_at`/`current_committed_version()`, or whatever
   this table already uses elsewhere for "read current state of one row";
   check `table_manager_crud.rs` or `table_manager_tx_ops.rs` for the
   existing single-record current-read pattern rather than inventing one).
   If the record still exists, recompute its posting via
   `IndexManager::plan_record_created`-equivalent single-record logic (the
   index is still `Building` + in-flight at this point, so calling
   `plan_record_created` directly would route back into dirty-set capture
   instead of producing a posting — you need a way to force a DIRECT
   posting write for this specific def during Phase C, bypassing the
   in-flight check. Consider a small internal helper, e.g.
   `IndexManager::write_posting_direct(name_interned, record_id, value)` or
   similar, that skips the `is_build_in_flight` branch — name it clearly as
   Phase-C-only). If the record was deleted, remove any stale posting for
   it (same "direct write" bypass, but a removal).
   Convergence: stop looping once a full drain returns empty, OR after a
   fixed hard iteration cap (RFC v2 §2.4/§6.2 — exact threshold is an open
   question, ship a conservative fixed constant, e.g. 10 iterations; put it
   in `shamir-tunables` if that crate already has a precedent for similar
   small operational knobs, otherwise a local `const`).

**Phase D (short publish barrier):**
6. `begin_write_barrier(REGULAR_INDEX_CREATE)` again.
7. Final `drain_dirty_set` + apply residual (same direct-write mechanism as
   step 5, now guaranteed small/bounded by the convergence criterion).
8. Flip `Building → Ready` + `save_index_info()` (Phase 3 sequence above).
9. `clear_build_in_flight(name_interned)`.
10. Release the barrier.

## Boundaries — regular/hash family ONLY

Do NOT touch `create_unique_index`/`create_unique_index_body`
(`table_manager_index_mgmt.rs:~731-849`) or `create_index_v2`
(`~:140-142`) — both stay on today's whole-barrier path (RFC v2 §5.2/§5.4).

## Doctor::repair() call-site swap (small, RFC v2 §4.3)

`doctor.rs`'s regular-family rebuild path (`~:509+`, the `Building | Failed`
gate `~:632-652`) currently calls the old whole-barrier entry point. Swap it
to call your new online-build entry point instead, so a manual repair also
gets the reduced-writer-stall benefit. This is a call-site change only, not
new logic.

## Invariant that must not break

`Building` indexes are already planner-invisible today (`doctor.rs:97-101`).
Phase D must remain the ONLY place that sets `IndexState::Ready`. Verify no
planner code needs to change — if you find it does, STOP and report that as
a finding rather than making planner changes (out of scope for this brief).

## Tests — this is the payoff, be thorough

Add to `crates/shamir-engine/src/table/tests/` (new file,
`p1059_online_create_index_tests.rs`, one file per this repo's convention):

1. **Basic correctness, no concurrent writes.** Table with data, CREATE
   INDEX via the new path, resulting postings byte-identical to what the
   OLD `create_index_from_stream` path produces on the same fixture (the
   degenerate empty-dirty-set case).
2. **Concurrent writes during Phase A land correctly.** Use a pause-hook
   seam (add one if none exists at the right point — mirror
   `create_index_backfill_hook`'s existing pattern) to pause mid-Phase-A,
   issue inserts/updates/deletes from a separate task (mixed, including a
   row inserted-then-deleted before Phase A ever reaches it), resume, wait
   for completion, assert the final index correctly reflects every write's
   FINAL state — this is the direct proof of RFC v2 Claim 2.
3. **Writer stall is bounded, not O(table).** A concurrent writer attempting
   a write during Phase A (barrier-free) does NOT block for the scan's
   duration — assert it completes quickly (e.g., under some generous
   millisecond bound) even while Phase A is still running on a
   non-trivially-sized fixture. Don't need microbenchmark precision here —
   a coarse "writer completed while Phase A is still in flight" check
   proves the point; #1062 owns the real bench.
4. **Fallback path.** A table without changefeed wired still gets a
   correctly-built index via the old whole-barrier fallback (Phase B step 2's
   `None` branch).
5. **doctor::repair() uses the new path.** A `Building`-stuck index gets
   `repair()`'d via the new entry point — same postings result as before,
   confirm via existing doctor test patterns.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff (this will be large — summarize by file, then show
the orchestration function in full), the exact new test names, and confirm
existing tests for `create_index`/`doctor::repair()` still pass unchanged
(don't just say "gate green" — name which existing test files you re-ran
and confirm they weren't touched).
