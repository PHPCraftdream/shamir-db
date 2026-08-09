# Brief 59 — #1087 round 3: barrier guards never dropped before Phase A — real deadlock

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 2 got right — do not touch

Round 2 fixed all 3 compile-blocking issues from round 1 (decode API,
private-field access via `IndexManager::write_postings_batch`, the
per-batch shadowing bug). `cargo check`, `fmt`, `clippy`, and
`./scripts/test.sh -p shamir-index` are all clean. Leave that part alone.

## The bug round 2 self-reported but did not root-cause

Round 2 honestly reported (did not hide) that
`p1087_phase_b_a_concurrent_write_captured_in_dirty_set` hangs, and guessed
("likely... MVCC snapshot stream iteration or the posting write loop")
without confirming which test actually hangs.

I re-ran the 3 tests myself
(`./scripts/test.sh -p shamir-engine -- p1087_phase_b_a`) and got a precise
result — nextest's own timeout/slow markers, not a guess:

```
PASS    [   0.187s] p1087_phase_b_a_fallback_when_changefeed_absent
PASS    [   0.205s] p1087_phase_b_a_correctness_no_concurrency
TIMEOUT [ 180.294s] p1087_phase_b_a_concurrent_write_captured_in_dirty_set
Summary [ 180.350s] 3 tests run: 2 passed, 1 timed out
```

Only the concurrent-write test hangs. The other two pass. This pins the
root cause precisely — it's a real deadlock, not a stream-iteration bug.

## Root cause (found by reading the diff, not guessing)

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s
`phase_b_a_backfill` binds the barrier guards at the top:

```rust
let (_barrier, _uwl_guard) = self
    .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
    .await;
```

The comment above the Phase A section claims:

```rust
// ── Phase A: barrier-free backfill scan ─────────────────────────────────
// Drop barrier guards here (RAII), but keep `guard` (SnapshotGuard) alive
// through the scan — it pins the version floor for the snapshot_stream.
```

**But there is no actual `drop(_barrier); drop(_uwl_guard);` call anywhere
in the function.** `_barrier`/`_uwl_guard` are ordinary local bindings —
Rust drops them at the END of their enclosing scope (the end of the
function), not at the point of a comment. So they stay held through the
ENTIRE Phase A scan, including the pause window in the concurrent-write
test — directly contradicting the whole point of "barrier-free backfill"
(the RFC's Phase A is supposed to let writers proceed concurrently).

This is why the concurrent-write test deadlocks: `needs_write_barrier()`
checks `write_barrier_flags.any_set()` (`table_manager.rs:1255-1257`) —
regardless of WHICH bit is raised, so `tbl.insert()`'s slow path tries to
acquire `unique_write_lock`, which is still held by the very
`phase_b_a_backfill` task that is parked (via the test's pause hook)
waiting for the concurrent insert to happen before it resumes. Task A
holds the lock and won't release it until Phase A finishes; task A won't
finish because it's parked waiting on the hook; the hook only releases
after task B's insert returns; task B's insert never returns because it's
blocked on the lock task A holds. Classic deadlock.

There is an EXISTING production precedent for exactly this "release the
barrier, then continue barrier-free" pattern — `doctor.rs:633-634`:

```rust
drop(_barrier);
drop(_uwl_guard);
```

(right after the backfill replay loop, before the barrier-free recount
pass that follows). Mirror this exactly.

## Fix

In `phase_b_a_backfill`, add:

```rust
drop(_barrier);
drop(_uwl_guard);
```

immediately after `self.index_manager.mark_build_in_flight(name_interned);`
and BEFORE the `// ── Phase A: barrier-free backfill scan` comment block
begins (i.e., right where the existing — currently false — comment claims
the drop already happens). Do NOT drop `guard` (the `SnapshotGuard`) here —
it must stay alive through the entire Phase A scan; only drop `_barrier`
and `_uwl_guard`.

Double check: `_barrier` is the `WriteBarrierGuard` (clears the intent bit
on drop), `_uwl_guard` is the `OwnedMutexGuard<()>` for `unique_write_lock`.
Drop order should match the `doctor.rs` precedent (`_barrier` first, then
`_uwl_guard`) — this is NOT the reverse of acquisition order, it matches
the existing working precedent, so don't "fix" it to be lock-then-bit.

## After the fix — re-run the 3 tests and confirm no more TIMEOUT

```
cargo check -p shamir-engine --lib
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine -- p1087_phase_b_a
./scripts/test.sh -p shamir-engine
```

Paste the exact nextest output for the 3 `p1087_phase_b_a_*` tests — all 3
must show `PASS`, not `TIMEOUT`/`SLOW`. If the concurrent-write test still
doesn't pass after this fix, STOP and report exactly what you tried and
what the new failure mode is — do not silently delete or weaken the test
to make it "pass" (e.g. removing the actual concurrent-write assertion).
That has cost real time to catch in this session before; an honest partial
report is strictly better than a fabricated pass.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff (should be a 2-line addition) and the exact nextest
output for all 3 `p1087_phase_b_a_*` tests.
