# Brief 64 — #1089 round 3: fix 2 pre-existing tests broken by the new default path

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What rounds 1+2 got right — do not touch

Round 1 wired `create_index` correctly (try online build, fall back to the
whole-barrier path when unavailable) and correctly left `doctor::repair()`
untouched (F-3 race, documented in the brief). Round 2 fixed the 2 new
integration tests' pause-hook hang by bumping their fixture size past the
hardcoded `batch_size = 1000`. All 4 `p1059_online_create_index_*` tests
pass, `cargo fmt`/`clippy` are clean, `./scripts/test.sh -p shamir-index`
is fully green. Leave all of that alone.

## The regression this round fixes

Running the FULL `shamir-engine` suite (not just the new/touched test
files) surfaces a real regression: 2 PRE-EXISTING tests now hang and hit
nextest's 180s TIMEOUT kill:

```
TIMEOUT [ 180.291s] table::tests::doctor_tests::verify_detects_building_regular_index
TIMEOUT [ 180.319s] table::tests::f72_planner_invisibility_tests::f72_regular_index_planner_invisible_during_backfill
```

**Root cause (diagnosed, not a guess).** Both tests build a table via
`RepoInstance::new(...) -> repo.add_table(...) -> repo.get_table(...)` —
this is the PRODUCTION table-construction path, which always attaches
BOTH `mvcc_store` and `changefeed` (same `RepoTxGate` instance). Both
tests then call:

```rust
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
...
let hook = Arc::new(BackfillPauseHook::new());
tbl.index_manager_ref()
    .set_create_index_backfill_hook(Some(Arc::clone(&hook)));
...
let create = tokio::spawn(async move { tbl_c.create_index(...).await });
hook.wait_until_parked().await;  // <-- hangs here
```

`set_create_index_backfill_hook` installs a pause seam INSIDE
`IndexManager::create_index_from_stream`'s Phase 2 backfill loop — the OLD
whole-barrier path. Before this task (#1089), `create_index` ALWAYS called
`create_index_from_stream`, so that seam always fired. **After round 1's
wiring, `create_index` now tries the ONLINE build path FIRST
(`phase_b_a_backfill`/`phase_c_d_catchup_and_publish`) whenever a
changefeed is attached — which it is here, because these tests use the
real `RepoInstance` construction path.** `phase_b_a_backfill` never calls
`create_index_from_stream` at all in this case, so the OLD hook's seam
never fires, `hook.wait_until_parked()` waits forever, and nextest kills
the test at 180s.

This is expected fallout from making online build the DEFAULT path for
changefeed-attached tables (which is the entire point of #1087-#1089) — it
is not a bug in the wiring itself, just two pre-existing tests whose pause
mechanism needs to target the NEW seam that actually fires now.

## Fix — swap both tests to the NEW pause-hook seam

The type to use is `crate::table::index2_backfill_hook::BackfillPauseHook`
(NOTE: despite the module name "index2_backfill_hook", this is the SAME
hook type `#1087`'s and `#1089`'s own tests already use for the
regular-family online-build path — see
`crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs` and
`p1059_online_create_index_tests.rs` for the proven usage pattern:
`tbl.online_index_backfill_hook.store(Some(Arc::clone(&hook)));` to
install, `tbl.online_index_backfill_hook.store(None);` to clear after use).
This is a DIFFERENT Rust type from
`shamir_index::base_index::backfill_pause_hook::BackfillPauseHook` (the old
one) — same shape (a `Notify`-based rendezvous with
`wait_until_parked`/`wait_at_window`/`release`), different crate, not
interchangeable — you must change the import and the install/clear
call-sites, not just keep calling `set_create_index_backfill_hook`.

**Important — the pause point differs slightly.** The old hook paused
INSIDE the backfill loop after some postings were written but BEFORE the
Building→Ready flip (same critical window). The new hook
(`online_index_backfill_hook`) pauses inside `phase_b_a_backfill`'s Phase A
scan loop, gated on `batch_no == 1` (i.e., only fires once a SECOND stream
batch arrives — see brief 60's diagnosis of the exact same batch_size
mechanic). **Both tests currently insert only 3 rows** — with the
hardcoded `batch_size = 1000` in `create_index`'s wiring
(`table_manager_index_mgmt.rs:669`), 3 rows fit in ONE batch, so the pause
would never fire even after swapping hook types. **You must ALSO bump each
test's fixture size past 1000 rows**, exactly like round 2 did for the two
`p1059_online_create_index_*` tests — add filler rows before (or
interleaved with) the existing named rows so the total exceeds 1000, and
adjust assertions to account for the filler data (e.g. filter matches to
just the specifically-tracked ids, not all rows). Reuse the same pattern
round 2 used in `p1059_online_create_index_tests.rs` for reference.

For `verify_detects_building_regular_index`
(`doctor_tests.rs:544-608`): after the pause fires (via the new hook, past
1000+ rows), the test's core assertions (Building state reported unhealthy
by `verify()`, message present, overall report unhealthy; then after
release, Ready + healthy) are STILL VALID and should NOT change — only the
hook type/install-site and the fixture size need to change.

For `f72_regular_index_planner_invisible_during_backfill`
(`f72_planner_invisibility_tests.rs:113-`): same — the core assertion (a
concurrent read during the pause window falls back to a full scan,
`index_used == None`, and returns the COMPLETE correct row set) is still
valid and must not change. Adjust the expected id set if filler rows
change what "active"/"inactive" status rows exist — keep the
originally-tracked 3 rows (ids 1/2/3, statuses active/inactive/active)
identifiable and distinct from filler rows (e.g. give filler rows a status
value that doesn't match "active", so the expected `vec![1, 3]` assertion
stays correct without needing to enumerate filler ids).

## After the fix — re-run and confirm

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- verify_detects_building_regular_index
./scripts/test.sh -p shamir-engine -- f72_regular_index_planner_invisible_during_backfill
./scripts/test.sh -p shamir-engine -- p1059_online_create_index
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Both previously-hanging tests must show `PASS`, not `TIMEOUT`. The LAST
command (full `shamir-engine` suite) is the one that matters most — it is
what caught this regression in the first place; a clean full run is the
actual proof this round succeeded. Paste the exact nextest summary line
for that last run (test count, pass count, 0 timeouts).

If either test still doesn't pass after this change (a genuine assertion
failure, not a timeout), STOP and report exactly what happened — do not
weaken an assertion or delete the test to force a pass.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff and the exact nextest output for the full
`shamir-engine` suite's final summary line (must show 0 timed out).
