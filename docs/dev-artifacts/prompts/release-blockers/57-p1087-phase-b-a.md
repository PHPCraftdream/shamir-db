# Brief 57 — #1087: Phase B (micro-barrier) + Phase A (barrier-free backfill)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

First of three slices decomposing the online CREATE INDEX orchestration
(RFC v2, `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`,
approved). A prior attempt at the full Phase A→D orchestration in one pass
got only ~5% done and was honestly abandoned with a recommendation to split
by phase — this brief covers Phase B + Phase A only. Phase C+D and the final
wiring into `TableManager::create_index` are SEPARATE tasks (#1088, #1089)
that will build on what you deliver here.

## What exists and is tested — do not modify

- `MvccStore::snapshot_stream(batch, at_version)` (`crates/shamir-tx/src/mvcc_store/mod.rs`).
- `TableManager::open_index_build_snapshot(&self) -> Option<shamir_tx::SnapshotGuard>`
  (`crates/shamir-engine/src/table/table_manager.rs`), `async fn`. Pins at
  `guard.version()`. `None` only if `changefeed` isn't wired.
- `IndexManager::mark_build_in_flight(name_interned)` /
  `is_build_in_flight(name_interned) -> bool` /
  `clear_build_in_flight(name_interned)` /
  `drain_dirty_set(name_interned) -> Vec<RecordId>`, all `pub`, all in
  `crates/shamir-index/src/base_index/index_manager.rs`. Dirty-set capture
  already happens automatically inside `plan_record_created`/
  `plan_record_updated`/`plan_record_deleted` once `mark_build_in_flight`
  has registered a `Building` def — you don't write that capture logic,
  it's already there.

## Scope of THIS task

Deliver a callable unit (a new method — your choice of exact placement and
signature, but document your reasoning) that performs Phase B + Phase A and
leaves the index in `Building` state with postings correct as of the pinned
version. Do NOT flip to `Ready`. Do NOT drain the dirty-set or do any
catch-up — that's #1088. Do NOT wire this into the public
`TableManager::create_index` — that's #1089.

**Phase B (micro-barrier):**
1. `begin_write_barrier(REGULAR_INDEX_CREATE)` — same call, same order as
   today (`crates/shamir-engine/src/table/table_manager.rs`'s
   `begin_write_barrier`; the admission→bit→drain→lock order is load-
   bearing — see `writer_drain_barrier.rs:50-146` and the proven deadlock
   from the reversed order in `f70_lock_order_inversion_tests.rs`).
2. Under the barrier: call `open_index_build_snapshot()`. If `None`, this
   function should return a value/error that clearly signals "online build
   unavailable for this table" — the caller (#1089) decides what to do with
   that (fall back to the old path). Do not panic, do not silently proceed
   as if a build happened.
   If `Some(guard)`: pin = `guard.version()`. Register the index at
   `Building` — the EXACT sequence already used in
   `create_index_from_records`/`create_index_from_stream`'s Phase 1
   (`crates/shamir-index/src/base_index/index_manager.rs:1544-1558`):
   `self.indexes.add_index(index_def); self.bump_generation();
   self.has_indexes.store(true, Ordering::Release);
   self.save_index_info().await?;`. Call
   `mark_build_in_flight(name_interned)`.
3. Release the barrier (drop `_barrier`/`_uwl_guard`). Do NOT drop the
   `SnapshotGuard` yet — it must live through all of Phase A.

**Phase A (barrier-free scan):**
4. Stream `mvcc_store().snapshot_stream(batch, pin)`. Adapt each batch the
   same way `create_index`'s existing `list_stream` adaptation does
   (`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:707-713`,
   `RecordCow::into_inner()`). Write postings via the same batched
   `set_many` pattern Phase 2's body already uses
   (`index_manager.rs:1727+`, the `while let Some(batch) = stream.next()`
   loop) — same shape, different stream source.
5. Once the stream is fully drained, drop the `SnapshotGuard`.

Return whatever the caller needs to hand off to Phase C+D — at minimum
`name_interned`; you likely don't need to return anything else, since the
in-flight registry and dirty-set already carry all the state Phase C needs
externally (accessible via `IndexManager`, not something this function's
return value needs to smuggle out).

## Tests (TDD)

Add to `crates/shamir-index/src/base_index/tests/index_manager_tests/`
(check existing conventions there) or
`crates/shamir-engine/src/table/tests/` — whichever level your chosen
placement makes more natural; a new file,
`p1087_phase_b_a_tests.rs`-equivalent.

1. **Basic correctness, no concurrent writes.** Table with data, run
   Phase B+A, resulting postings byte-identical to what the OLD
   `create_index_from_stream` produces on the same fixture. Index remains
   `Building` afterward — assert this explicitly (this task must NOT flip
   to `Ready`).
2. **Concurrent writes during Phase A land in the dirty-set.** Use a
   pause-hook seam (add one at the right point in Phase A's scan loop if
   none exists — mirror the existing `create_index_backfill_hook` pattern)
   to pause mid-scan, issue a write from a separate task, resume, and
   confirm the written `RecordId` ends up in the dirty-set (via
   `drain_dirty_set`, which you can call directly in the test even though
   #1088 owns its real usage) — you're not testing NEW capture logic here,
   just confirming your Phase B correctly called `mark_build_in_flight`
   BEFORE Phase A started scanning, so the existing #1058 capture mechanism
   is actually active for the whole scan window.
3. **Fallback signal.** A table without `changefeed` wired → your function
   returns the "unavailable" signal, does not register anything, does not
   panic.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff, the exact function signature you landed on (and why
you placed it where you did), and the exact test names — paste the actual
nextest output for your new tests, not a paraphrase. If you cannot finish
everything in this brief within your time budget, STOP and report honestly
exactly what's done and what's missing — do not fabricate completion or
silently delete a test you couldn't get working. That has happened before in
this session and cost real time to catch; an honest partial report is
strictly better.
