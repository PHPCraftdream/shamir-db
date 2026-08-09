# Brief 65 — #1060: crash-recovery invariant matrix for online CREATE INDEX

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Read this first — the task's original premise was wrong, corrected below

The original #1060 description (written when #1059 was decomposed) said a
"self-heal at table-open" mechanism drops partial postings and restarts
the whole build sequence on reopen after a crash, and that this task just
needs to prove that mechanism works across a crash-point matrix.

**That mechanism does not exist for the regular/hash (`base_index`)
family.** Verified by reading `IndexManager::new`
(`crates/shamir-index/src/base_index/index_manager.rs:445-499`) — it only
decodes `system:indexes`/`system:indexes_unique` via
`IndexInfo::decode_bytes`; it never re-runs a backfill. There is an
explicit, pre-existing doc comment confirming this is intentional, dating
to well before this RFC (F-72, #899):

> "Unlike index2, the base_index `IndexManager` family has NO automatic
> restart-from-scratch self-heal for a `Building` definition at table-open
> time... A `Building` definition left behind by a crash/error therefore
> stays durably `Building` — permanently planner-invisible, never silently
> resurrected as `Ready` — until an operator runs `TableManager::repair()`
> ... This is an accepted, explicitly-documented gap." (`index_manager.rs:1530-1541`)

So: **recovery from a crash mid-online-build is manual** (an operator runs
`TableManager::repair()`, `doctor.rs:523-598`, which unconditionally
rebuilds every regular/unique/sorted definition). This has always been true
for the base_index family, independent of the RFC v3 online-build
redesign — nothing in #1087/#1088/#1089 changed it or was supposed to.

## The corrected scope

Prove the ONE invariant that actually matters, at every point along the
Phase B→A→C→D pipeline where a crash could land: **a crash never leaves
the index durably `Ready` with incomplete or incorrect postings.** Either
the index stays durably `Building` (safe — planner-invisible per
`doctor.rs:97-101`, forever, until an operator notices and runs
`repair()`), or the crash landed AFTER the final Phase D flip, in which
case the index is durably `Ready` AND fully, correctly built (not a race —
Phase D's flip+persist is the only point `Ready` is ever set, per
`index_manager.rs:1645-1673`'s F-72 ordering, already proven by #1087-1089's
own tests).

This does NOT require testing "recovery completes the build" (nothing
completes it automatically) — it requires testing "the index never lies
about its own completeness."

## Existing precedent to copy — read this file section first

`crates/shamir-engine/src/table/tests/p1048_index2_drop_durability_tests.rs:120-175`
is the exact pattern to copy for crash simulation. Key points, verified by
reading it:

1. Install a `BackfillPauseHook` on the manager BEFORE starting the
   operation you want to interrupt.
2. Race the operation against `pause_hook.wait_until_parked()` via
   `tokio::select!` — the LOSING branch (the operation) is genuinely
   CANCELLED (its future dropped) when the hook fires first, not merely
   raced against a timer. This is a faithful crash simulation because
   `phase_b_a_backfill`/`phase_c_d_catchup_and_publish` run entirely on the
   caller's task with NO `tokio::spawn` anywhere in the pipeline
   (grep-verified) — dropping the future tears down every in-memory
   structure it was building (SnapshotGuard, barrier guards, everything),
   exactly like a real process crash would.
   ```rust
   tokio::select! {
       _ = mgr_c.some_operation(...) => {
           panic!("operation completed before the pause hook fired");
       }
       _ = pause_hook.wait_until_parked() => {
           // Parked at the exact window under test.
       }
   }
   ```
3. `drop` the manager handle(s) after the `select!` to make the "crash"
   explicit (harmless but matches the precedent's style).
4. Reopen: `TableManager::create(name, Arc::clone(&data_store),
   Arc::clone(&info_store))` on the SAME underlying store `Arc`s — this is
   what simulates "process restart, disk survives." **You do NOT need to
   re-attach `mvcc_store`/`changefeed` on the reopened manager** unless a
   specific test needs to also verify post-recovery live behavior — reading
   back `IndexDefinition.state` only needs `info_store`.
5. Assert against the FRESH manager's state: `iter_indexes()` (NOT
   `iter_indexes_ready`, which filters to `Ready` only — you need to see
   `Building` too) `.find(|d| d.name_interned == ...)`, then check
   `.state`.

**Do NOT use `tokio::spawn(...)` + `drop(join_handle)` to simulate a
crash** — dropping a `JoinHandle` does NOT cancel the spawned task; it
keeps running detached, parked on the hook forever, and the test hangs for
180s until nextest kills it. This exact mistake was already made once in
`#1048` and is called out by name in this task's own description as a trap
to avoid. Use `tokio::select!`, never `tokio::spawn`+drop.

## New pause-hook seams needed (3 — only Phase A has one today)

Mirror the EXACT pattern of the existing `online_index_backfill_hook`
field precisely (same type, same `#[cfg(test)]` gating, same 3
declaration/init sites):

- Field declaration: `crates/shamir-engine/src/table/table_manager.rs:277-278`
  (`#[cfg(test)] pub(super) online_index_backfill_hook:
  Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,`).
- `Clone` impl init: `table_manager.rs:356`
  (`online_index_backfill_hook: Arc::clone(&self.online_index_backfill_hook),`).
- Two constructor init sites: `table_manager.rs:504` and `:841`
  (`online_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),`).
- Usage inside `phase_b_a_backfill`'s scan loop:
  `table_manager_index_mgmt.rs:2376-2385` (roughly — grep for
  `online_index_backfill_hook` to find the current exact line after your
  own edits shift things).

Add THREE new fields, each following that exact same 4-site pattern
(declaration + Clone + 2 constructors), named:

1. **`phase_b_pause_hook`** — fires inside `phase_b_a_backfill`, right
   after `self.index_manager.mark_build_in_flight(name_interned);`
   (`table_manager_index_mgmt.rs:2307`) and BEFORE
   `drop(_barrier); drop(_uwl_guard);` (`:2313-2314`). At this point:
   `register_index_at_building`'s `save_index_info()` has already
   durably persisted the `Building` marker (verified by
   `register_index_at_building`, `index_manager.rs:1893-1912`); in-flight
   is marked (in-memory, irrelevant after crash); the barrier is STILL
   HELD. This tests "crashed mid-Phase-B, after the durable Building
   persist."

2. **`phase_c_pause_hook`** — fires inside `phase_c_d_catchup_and_publish`,
   at the TOP of the catch-up loop (`table_manager_index_mgmt.rs:2459`,
   the `for _ in 0..Self::CATCHUP_ITERATION_CAP` loop), before
   `self.index_manager.drain_dirty_set(name_interned)` on EVERY iteration
   OR — simpler and sufficient for one deterministic test — only on the
   FIRST iteration (mirror the `batch_no == 1`-style single-fire gate
   Phase A's hook already uses, e.g. gate on a loop-iteration counter
   `== 0`). At this point: Phase A's postings are durable (from the prior
   phase), `Building` is on disk, the `SnapshotGuard` is still held
   (`pin` still valid), no barrier is held (Phase C is barrier-free).

3. **`phase_d_pause_hook`** — fires inside `phase_c_d_catchup_and_publish`,
   right after the Phase D barrier is acquired
   (`table_manager_index_mgmt.rs:2469-2471`,
   `let (_barrier, _uwl_guard) = self.begin_write_barrier(...).await;`)
   and BEFORE the final residual drain/apply
   (`:2475` `self.index_manager.drain_dirty_set(name_interned)`). At this
   point: `Building` is STILL on disk (the flip is the LAST step of Phase
   D, per F-72 ordering) — even though the publish barrier is held.

Each new hook fires unconditionally when installed (unlike Phase A's,
which needs a 2nd stream batch to trigger — these three don't have that
constraint since they're not gated on stream progress). Use the same
`hook.wait_at_window()` / test-side `wait_until_parked()` /
`release()` API as the existing `BackfillPauseHook`
(`crates/shamir-engine/src/table/index2_backfill_hook.rs`) — no new hook
TYPE needed, just new FIELD instances of the same type so 3 independent
tests can each install and drive their own hook without interference.

## Tests — one per crash-matrix row, new file
`crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs`
(wire into `tests/mod.rs`)

Use a helper mirroring `make_table_with_mvcc_and_changefeed` from
`#1087`/`#1088`/`#1089`'s test files (`Arc<dyn Store>` for data/info/
history so they can be cloned and reused across the "before crash"/"after
reopen" manager instances — the crash-simulating manager needs
mvcc+changefeed to reach the online-build path at all; the reopened
verification manager does not).

1. **Before Phase B.** Insert some data, do NOT call `create_index` at
   all (or call it and let it complete/fail before any barrier — simplest:
   just never call it). Reopen. Assert: no index registered
   (`iter_indexes()` finds nothing for that name). Degenerate but cheap —
   confirms the baseline.

2. **Inside Phase B** (`phase_b_pause_hook`). Race
   `tbl.phase_b_a_backfill(name, index_def, 1000)` against the hook via
   `select!`. After the crash+reopen, assert: the index IS registered and
   its `state == IndexState::Building` (the persist from
   `register_index_at_building` survived), and it's absent from
   `iter_indexes_ready()` (planner-invisible).

3. **Inside Phase A** (reuse the EXISTING `online_index_backfill_hook`,
   already proven by `#1087`'s tests — no new hook needed here). Insert
   enough rows to force ≥2 stream batches (past the hardcoded
   `batch_size = 1000` in `create_index`'s wiring, or call
   `phase_b_a_backfill` directly with a batch_size you control, matching
   `#1087`'s own test pattern). Race against the hook. After crash+reopen:
   `Building`, planner-invisible — same assertions as test 2. Additionally
   assert SOME postings exist on disk (Phase A had written some before the
   pause) but this doesn't matter for correctness since `Building` is
   invisible regardless — the point is proving the STATE is `Building`,
   not asserting anything about partial posting counts.

4. **Inside Phase C** (`phase_c_pause_hook`). Run `phase_b_a_backfill` to
   completion normally (uninterrupted), THEN race
   `phase_c_d_catchup_and_publish` against `phase_c_pause_hook`. After
   crash+reopen: `Building`, planner-invisible.

5. **Inside Phase D** (`phase_d_pause_hook`). Same setup as test 4, but
   race against `phase_d_pause_hook` instead. After crash+reopen:
   `Building`, planner-invisible — explicitly note in the test's doc
   comment that this exposure window is now milliseconds (bounded residual
   under the barrier) vs. the old design's minutes (whole-table scan under
   the barrier), even though the RECOVERY ACTION (stays Building, manual
   `repair()`) is unchanged.

6. **After Ready, before in-flight cleanup.** No new hook needed — this
   window is provably safe without precision timing, since `Ready` is only
   ever set by the already-tested atomic flip+persist
   (`IndexManager::flip_to_ready`). Run `phase_b_a_backfill` +
   `phase_c_d_catchup_and_publish` to full, uninterrupted completion (no
   `select!`, no pause hook), THEN reopen anyway and assert the index is
   `Ready`, `iter_indexes_ready()` finds it, and its postings are correct
   (same lookups as `#1088`'s own tests). This is really "prove reopening
   after a successful build changes nothing" — a corollary, not a
   crash-simulation, but it completes the matrix's 6th row honestly rather
   than skipping it.

For every test, use DISTINCT status/name field values per record so
lookups are unambiguous (mirror the "alice"/"bob"/"charlie" pattern from
`#1087`/`#1088`/`#1089`'s test files).

**Do not write a tautological test** — the task description explicitly
warns about this. A test that only asserts "the function returned without
panicking" or "some index exists" proves nothing. Every test above must
assert the SPECIFIC `IndexState` (`Building` vs `Ready`) and, where
relevant, planner-visibility (`iter_indexes_ready()` inclusion/exclusion)
— these are the properties that actually matter for correctness.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

The LAST command (full `shamir-engine` suite) matters most — a previous
task in this chain (#1089) found a real regression only visible in the
FULL suite run, not a scoped one. Report the exact diff, which 6 tests you
wrote, their individual pass/fail status, and the full suite's final
summary line (must show 0 timed out, 0 failed).
