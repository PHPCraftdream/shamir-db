# Brief 62 — #1089: wire Phase B+A → Phase C+D into `create_index`, integration tests

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

This is slice 1d-3, the FINAL wiring task of online CREATE INDEX (RFC v3,
`docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`). Both
prerequisites are done and committed:

- `#1087`: `TableManager::phase_b_a_backfill(index_def, batch_size) ->
  DbResult<Option<PhaseBAResult>>` — Phase B (micro-barrier: register at
  Building, mark in-flight, open a `SnapshotGuard`) + Phase A (barrier-free
  snapshot scan + backfill). `Ok(None)` = changefeed not wired, online build
  unavailable for this table. `PhaseBAResult { guard, pin }` on success —
  the guard MUST be passed through to Phase C/D, not dropped early.
- `#1088`: `TableManager::phase_c_d_catchup_and_publish(name_interned,
  phase_ba: PhaseBAResult) -> DbResult<()>` — Phase C (barrier-free catch-up
  loop, pin-vs-current diff) + Phase D (short publish barrier, flip
  Building→Ready, clear in-flight, drop the guard).

Both are `pub(crate)`, currently unused outside their own tests
(`#[allow(dead_code)]`). This task wires them into the real
`TableManager::create_index` entry point and proves the whole pipeline
end-to-end.

## Part 1 — a gap in `phase_b_a_backfill` that MUST be closed first

Read today's `create_index` (`table_manager_index_mgmt.rs:635-726`)
carefully. It does, IN THIS ORDER, ALL under one `begin_write_barrier`
critical section (lines 659-725):

1. Acquire barrier (raises `ddl_admission` + `REGULAR_INDEX_CREATE` bit +
   `unique_write_lock`).
2. `any_index_exists(name)` check — R0-C (#1010) cross-family
   name-uniqueness preflight. **This MUST happen inside the SAME admission
   critical section as registration** — see the comment at line 662-669:
   "done WHILE holding `ddl_admission`... closes the TOCTOU gap." If the
   check and the registration are two SEPARATE barrier acquisitions, a
   second concurrent `create_index` call with the SAME name can slip
   between them and also pass the "does not exist" check — both then try
   to register, a name collision the R0-C fix specifically closes.
3. `self.interner.persist().await?` (F-42 — persist before publish).
4. `create_index_from_stream` (registers + backfills + flips, still under
   the SAME barrier).

`phase_b_a_backfill` (#1087) currently does NOT do the `any_index_exists`
check at all — it goes straight from barrier acquisition to
`open_index_build_snapshot`/`register_index_at_building`. **Add the check
inside `phase_b_a_backfill`**, in the same position relative to the barrier
that today's `create_index` uses (right after acquiring
`(_barrier, _uwl_guard)`, before `open_index_build_snapshot`), so the
TOCTOU-closing property is preserved for the online-build path too. This
requires adding a `name: &str` parameter to `phase_b_a_backfill`'s
signature (it currently only receives `index_def`, which has
`name_interned` but not the display name `any_index_exists` needs). Return
the SAME `DbError::KeyExists` shape `create_index` uses today (line
671-674) on collision. Update `phase_b_a_backfill`'s 3 existing tests
(`p1087_phase_b_a_tests.rs`) to pass a `name` argument — pick any
non-colliding string, e.g. `"idx_name"` (already used as the index's own
display name in those tests via `build_index_def`).

## Part 2 — wire `create_index`

`create_index`'s PUBLIC signature does not change. Restructure the body:

1. Keep the preamble EXACTLY as today: `build_index_definition`, the
   `in_flight_creates` guard, `index_def.state = Building`,
   `self.interner.persist().await?`. **Do NOT move `interner.persist()`
   inside `phase_b_a_backfill`'s barrier** — it only needs to happen before
   backfill/publish (per the F-42 comment, line 676-693), which holding it
   in `create_index`'s own preamble already satisfies; it does not need to
   be under the SAME admission lock as registration (unlike the
   `any_index_exists` check in Part 1, which does).
2. Try the online-build path first:
   ```rust
   match self.phase_b_a_backfill(name, index_def.clone(), 1000).await? {
       Some(phase_ba) => {
           self.phase_c_d_catchup_and_publish(index_def.name_interned, phase_ba)
               .await
       }
       None => {
           // Fallback: table has no changefeed wired (e.g. a system table
           // or a directly-constructed test table) — online build is
           // unavailable. Use the EXACT same whole-barrier path
           // `create_index` uses today.
           let (_barrier, _uwl_guard) = self
               .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
               .await;
           if self.any_index_exists(name).await {
               return Err(shamir_storage::error::DbError::KeyExists(format!(
                   "index '{name}' already exists on this table (possibly in a different \
                    index family — names are unique per table across all families)"
               )));
           }
           let stream = self.list_stream(1000).map(|batch| {
               batch.and_then(|rows| {
                   rows.into_iter()
                       .map(|(id, cow)| cow.into_inner().map(|v| (id, v)))
                       .collect()
               })
           });
           self.index_manager
               .create_index_from_stream(index_def, stream)
               .await
       }
   }
   ```
   Adjust to actual field/method names as needed — this is scaffolding, not
   gospel, but the STRUCTURE (try online build, fall back to today's exact
   whole-barrier path on `None`) is the required shape.
3. `index_def.clone()` is needed above because the fallback branch also
   needs `index_def` (moved into `create_index_from_stream`) — check
   whether `phase_b_a_backfill` takes `index_def` by value (it does, per
   its `#1087` signature) and clone accordingly, or restructure so only one
   branch consumes it. Either is fine; just make it compile without an
   unnecessary clone in the COMMON (online-build-available) path if you can
   avoid it cleanly — not a hard requirement.

**Do NOT touch** `create_index_from_records`/`create_index_from_stream`
(`crates/shamir-index/src/base_index/index_manager.rs`) — their doc
comments require byte-for-byte-unchanged behavior for the F-78
correctness-equivalence test (see `index_manager.rs:1690-1691`:
"`[create_index_from_records]` is preserved byte-for-byte unchanged so the
F-78 correctness-equivalence test can build the SAME index the OLD..."). Do
NOT touch `create_unique_index`/`create_unique_index_body` or
`create_index_v2` — both stay on today's whole-barrier path per RFC v2
§5.2/§5.4 (unique-family concurrent-duplicate-detection and index2 are
explicitly out of scope for slice 1).

## Part 3 — `doctor::repair()` is OUT OF SCOPE for this task (do not touch it)

The original task description for #1089 said to swap `repair()`'s
regular-family rebuild call site to the new online-build entry point,
calling it "a minor thing." **Investigation found this is unsafe as
described — do not do it.**

`repair()` (`doctor.rs:523-643`) holds ONE `begin_write_barrier` across the
ENTIRE drop→recreate sequence for ALL THREE index families (regular +
unique + sorted) in a single loop. The comment at `doctor.rs:533-557` (F-3,
#1030) explains why: a concurrent `CREATE INDEX` could otherwise race
`repair()`'s direct `drop_index`/registration calls against the same
generation-counter sequence, unserialized — closed specifically by holding
`ddl_admission` (embedded in the `WriteBarrierGuard`) for the WHOLE
self-heal block, not once per index. Swapping the regular-family rebuild to
call `phase_b_a_backfill`/`phase_c_d_catchup_and_publish` PER DEF (each of
which acquires and releases its OWN `ddl_admission` internally) would
release and re-acquire that mutex multiple times inside `repair()`'s loop —
reopening exactly the race F-3 closed (a concurrent `CREATE INDEX` could
interleave between two of `repair()`'s per-def barrier acquisitions).
Fixing this properly needs its own design (e.g. a bulk online-build entry
point that holds one outer admission guard across multiple inner
short-barrier registrations) — clearly a separate task, not "a minor
swap." Leave `doctor.rs` completely unmodified. Report this finding in your
final summary so it can be filed as an explicit follow-up task.

## Integration tests — the main deliverable

New file `crates/shamir-engine/src/table/tests/p1059_online_create_index_tests.rs`
(wire into `tests/mod.rs`). Reuse the `BackfillPauseHook` seam from
`#1087`'s tests (`tbl.online_index_backfill_hook`) for the pause-and-drive
pattern already proven in `p1087_phase_b_a_tests.rs`.

1. **Basic correctness, byte-for-byte vs. the old path.** Build a table
   with test data, call `create_index` through the NEW path (a table WITH
   mvcc+changefeed attached — same helper pattern as `#1087`/`#1088`'s
   `make_table_with_mvcc_and_changefeed`). Separately, on an EQUIVALENT
   fixture (same data), call the OLD path directly
   (`index_manager.create_index_from_stream` or a table WITHOUT
   changefeed, forcing the fallback branch) and compare the resulting
   postings are identical (same records match the same lookups). Do not
   assert byte-identical serialized index_info if that's fragile — assert
   observable equivalence (same `lookup_by_index` results for every test
   value).

2. **Concurrent writes during Phase A land correctly in the final index —
   direct proof of RFC Claim 2.** Insert initial data, install the pause
   hook, spawn `create_index` in a task, wait for it to park mid-scan, then
   from a separate task issue a MIX of concurrent operations: an insert, an
   update-to-different-value on a pre-existing row, and a delete of another
   pre-existing row (this exercises all three RFC v3 diff cases at once —
   see `#1088`'s tests for the individual-case patterns). Resume, await
   completion, then assert the final index reflects the FINAL state of
   every record: the new insert is findable, the updated row is findable
   ONLY under its new value (not the old), the deleted row is not findable
   under its old value at all. Clear the hook when done.

3. **Writer latency is bounded, not O(table).** Build a table with a
   non-trivial fixture (some hundreds to low thousands of rows — big enough
   that a full-barrier scan would be observably slow, small enough the test
   stays fast). Install the pause hook, spawn `create_index`, wait for it
   to park mid-scan (Phase A definitely still "in flight" from the test's
   perspective), then time a concurrent `tbl.insert(...)` — assert it
   completes within a generous bound (e.g. under 500ms; the point is
   "did not wait for the whole scan," not a tight perf assertion — that's
   `#1062`'s job). Resume and let `create_index` finish.

4. **Fallback path.** A table WITHOUT a changefeed still gets a correctly
   built index via `create_index` (the `None` branch in Part 2). Assert
   normal correctness (all inserted rows queryable through the index) —
   this is the existing whole-barrier behavior, just confirm the wiring's
   fallback branch actually reaches it (don't just assert it "worked",
   assert the SPECIFIC old code path ran if there's an observable way to
   tell, e.g. no `SnapshotGuard`/in-flight registry entry involved).

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

This gate is NOT optional and NOT satisfied by a self-report — the
orchestrator re-runs it personally and reads the diff before accepting.
Report exactly which tests you wrote, their individual pass/fail status,
and any deviation from this brief with your reasoning (especially if Part 1
or Part 3's reasoning turns out to be wrong once you're looking at the
actual code — say so plainly rather than silently working around it).
