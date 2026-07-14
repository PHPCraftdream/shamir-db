Task #535 — fix a genuine clear-race in `MemBufferStore`'s `dirty_nonempty`
fast-path sentinel that can mask an already-ACKed write. Found during G5/#530's
`@fl` review (pre-existing bug, not introduced by that diff — the merge-overlay
scan's fast path just inherits it).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context (confirmed by reading the current code — re-confirm line numbers,
## they may have shifted)

`crates/shamir-storage/src/storage_membuffer.rs`:
- `MemBufferState.dirty: Arc<DashMap<RecordKey, Slot, THasher>>` (~line 154) —
  the write-back buffer.
- `MemBufferState.dirty_nonempty: AtomicBool` (~line 162) — a fast-path
  sentinel so `get()` and `snapshot_overlay_sorted()` can skip the DashMap
  probe entirely when dirty is known-empty.
- Writer sequence (`insert`/`set`/`remove`, ~lines 596-687): each does
  `dirty_nonempty.store(true, Release)` **THEN** `dirty.insert(key, slot)`.
  This ordering is correct and intentional (documented at ~line 600-601):
  a reader's `Acquire` load can never see `false` while a write that already
  landed in `dirty` is still there — no false negative from THIS half.
- Drain sequence (`drain_once`, ~lines 358-409): snapshots a batch of
  `dirty` entries, flushes them to `inner`, `remove_if`s the drained
  entries (CAS-style — only removes if unchanged), THEN, at the very end:
  ```rust
  if state.dirty.is_empty() {
      state.dirty_nonempty.store(false, Ordering::Relaxed);
  }
  ```
  (~line 406-408). The comment there ("we only clear when dirty is
  actually empty, which is a stable post-drain observation") is the
  flawed assumption: `DashMap::is_empty()` and the subsequent `store(false)`
  are two SEPARATE, non-atomic steps with nothing connecting them — a
  writer's `insert` (on a different DashMap shard, or even the same one)
  can land in the gap between the `is_empty()` check returning `true` and
  the `store(false)` actually executing.

## The race, concretely

1. Writer A: `dirty_nonempty.store(true, Release)` — about to insert.
2. Drain B (a genuinely concurrent call — either the background auto-flush
   task, `MemBufferStore::flush()`'s explicit `drain_all()`, or `Drop`'s
   `drain_all()`; more than one of these CAN run concurrently, check
   `crates/shamir-storage/src/storage_membuffer.rs` around lines 275, 309,
   446-465, 782, 816 to confirm the actual call graph before assuming only
   one drainer ever runs): reaches its own `dirty.is_empty()` check —
   observes `true` (A's `insert` hasn't landed yet).
3. Writer A: `dirty.insert(key, slot)` — completes. The write is now
   "ACKed" (A's `Store::insert`/`set`/`remove` call returns `Ok` to its
   caller) and genuinely sitting in `dirty`.
4. Drain B: `dirty_nonempty.store(false, Relaxed)` — clears the sentinel,
   even though `dirty` is NOT actually empty (A's entry from step 3 is
   there).

Result: `dirty` holds a real, ACKed entry, but `dirty_nonempty == false`.
Every subsequent `get()` (~line 648) and `snapshot_overlay_sorted()`
(~line 430) fast-path check will skip the DashMap probe entirely — the
write becomes invisible to reads until some LATER, unrelated write
happens to flip `dirty_nonempty` back to `true` again (at which point the
masked entry becomes visible again, since it was never actually removed
from `dirty` — only the SENTINEL was wrong, not the data). Between step 4
and that next unrelated write, reads silently miss a real write.

## The fix — verify-after-clear, restore-on-mismatch

Do NOT trust a single `is_empty()` → `store(false)` sequence. After
storing `false`, immediately re-check `dirty.is_empty()` one more time:
if it is now NOT empty, a writer's `insert` raced into the gap — restore
`dirty_nonempty` to `true`.

Why this closes the race with certainty: the writer ALWAYS does
`store(true)` strictly before its own `dirty.insert()` (same thread,
program order — this ordering is already correct and untouched by this
fix). So if the drain's re-check observes ANY entry in `dirty` after its
own clear, that entry's writer must have ALREADY executed its own
`store(true)` in real time, at or before the point that entry became
observable — meaning restoring `dirty_nonempty = true` at that point is
never wrong (it may occasionally be a redundant/duplicate `true` store
racing with the writer's own `true` store, which is harmless — same
value, no lost update). Conversely, if the re-check observes `dirty` is
STILL empty, clearing to `false` is genuinely safe: nothing can have
raced in since the FIRST `is_empty()` check without ALSO being visible
to the second one (the second check happens strictly after the first,
and a raced-in writer's insert would show up in EITHER check once it has
happened — if it hasn't happened by the second check either, it hasn't
raced into this window at all).

Sketch (adapt to whatever's idiomatic in this file — a small loop is
fine, this is not a hot path: it only runs once per `drain_once` call,
after the flush itself, not per-entry):

```rust
if state.dirty.is_empty() {
    state.dirty_nonempty.store(false, Ordering::Release);
    // Re-verify: did a writer race an insert into the gap between the
    // check above and the store we just did? If so, the sentinel must
    // stay `true` — restore it. See the doc comment above for why this
    // is always correct (never a false negative, occasionally a
    // redundant no-op `true` write).
    if !state.dirty.is_empty() {
        state.dirty_nonempty.store(true, Ordering::Release);
    }
}
```

Use `Ordering::Release` (not the current `Relaxed`) for BOTH stores in
this function — the reader's `Acquire` load in `get()`/
`snapshot_overlay_sorted()` needs a real release to pair with, matching
the ordering discipline already used by the writer methods (`insert`/
`set`/`remove` already use `Release`). Using `Relaxed` here was arguably
part of the original bug's root cause (no ordering guarantee at all on
the clearing side) — confirm this reasoning holds and fix it, or explain
in your report if `Relaxed` is provably fine here (it should not be,
given the reader pairs `Acquire` against a `Release` store elsewhere).

Also check: is there a plausible ABA-style version of this same race
with the SECOND check itself (i.e., is a THIRD interleaving possible
that this two-step check-restore doesn't cover)? If you find one, either
close it (e.g. a bounded retry loop instead of a single re-check) or
document precisely why it can't happen. Reason through this carefully —
this is exactly the kind of subtle concurrency bug where "looks fixed"
and "is fixed" can diverge.

## Two call sites use this flag as a fast-path gate — both benefit automatically

`get()` (~line 648) and `snapshot_overlay_sorted()` (~line 430) both gate
on `dirty_nonempty`. Fixing the ONE place the flag is cleared
(`drain_once`) fixes both read paths — no changes needed at the read
sites themselves. Confirm this is genuinely true (i.e. no OTHER place in
this file also clears `dirty_nonempty` with the same flawed pattern —
grep for every `.store(false` on this field).

## TDD

1. A deterministic regression test that reproduces the race WITHOUT
   relying on `sleep`-based timing luck — use the same rendezvous
   pattern this campaign has used elsewhere (`tokio::sync::Notify` /
   `Barrier`, or a test-only hook) to force the exact interleaving:
   writer's `store(true)` happens, then drain's FIRST `is_empty()` check
   runs and observes empty, THEN writer's `dirty.insert()` lands, THEN
   drain's clear runs. If the current code has no seam to inject this
   deterministically, add the minimal `#[cfg(test)]` hook needed (mirror
   the style of task #534's `index2_backfill_hook.rs` — a `Notify`-based
   rendezvous, not a raw `sleep`). Prove this test FAILS against the
   pre-fix code (temporarily revert your fix, run it, confirm the
   assertion catches the masked write) and PASSES after.
2. Existing MemBuffer test suite (`crates/shamir-storage/src/tests/
   storage_membuffer_tests.rs` and the G5/#530 merge-overlay tests) must
   stay green — this is a targeted correctness fix, not a behavior
   change to any other path.

## Test scope

```
./scripts/test.sh -p shamir-storage
```

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-storage
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed
  > Where the verify-after-clear pattern was added, and the exact
    ordering (Relaxed -> Release, or your own reasoning if you kept
    Relaxed and can justify it)
  > Confirmed no other `.store(false, ...)` site on dirty_nonempty exists
  > New regression test: how it deterministically forces the race
    (no sleep-based timing), and how you verified it fails pre-fix
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-storage: pass/fail
```
