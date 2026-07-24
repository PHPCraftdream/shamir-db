# Brief for #799 (F-9) — cursor reaper must not reap a cursor with an active fetch in flight

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (confirmed by reading both sides of the race)

`crates/shamir-server/src/cursor_registry.rs`'s `Cursor::is_expired`
(~line 292-296) decides idle-expiry PURELY from a `last_activity_nanos`
atomic timestamp:

```rust
pub fn is_expired(&self, now: Instant, idle_ttl: Duration) -> bool {
    let elapsed_now = now.saturating_duration_since(self.created_at).as_nanos() as u64;
    let last = self.last_activity_nanos.load(Ordering::Acquire);
    elapsed_now.saturating_sub(last) >= idle_ttl.as_nanos() as u64
}
```

`Cursor::bump_activity` (~line 282-285) is called **only once, at the very
END of a successful `FetchNext`**
(`crates/shamir-server/src/db_handler/cursor_handlers.rs`, ~line 1806:
`cursor.bump_activity();`, right after `drop(state)` at line 1805) — NOT
at the START of the fetch. This means: from the reaper's perspective, a
cursor looks exactly as idle WHILE a fetch is actively running as it does
while genuinely abandoned — `last_activity_nanos` does not move until the
in-progress fetch finishes. **A `FetchNext` whose own execution time (a
large `page_size`, an expensive scan, a slow keyset boundary search, etc.)
exceeds `idle_ttl` will be reaped by the background sweep WHILE it is
still running**, even though the cursor is being actively used the entire
time. The client's in-flight request still completes and gets a
successful response (the fetch holds its own `Arc<Cursor>` clone,
unaffected by registry removal), but the cursor is gone from the registry
by the time that response arrives, so the client's NEXT `FetchNext`
attempt (which they have every reason to expect will work, since the
prior one just succeeded) gets `CursorExpired`.

Traced the actual reap mechanics (`crates/shamir-server/src/cursor_registry.rs`,
~line 464-472 `expired_ids`, ~line 522-557 `spawn_reaper_task`'s sweep
loop): `expired_ids` filters `self.open` purely by `is_expired`, with NO
awareness of whether a `FetchNext` currently holds the cursor's
`state()` lock. The reaper loop then unconditionally calls
`remove_for_idle_reap` on every id in that list.

## Why a `try_lock` guard closes this (confirmed: the state lock IS held for the full fetch duration)

Traced `fetch_next` in `crates/shamir-server/src/db_handler/cursor_handlers.rs`:
`let mut state = cursor.state().lock().await;` (~line 1531) is acquired
BEFORE the authorize/resolve/read work begins, and the `MutexGuard`
(`state`) is referenced continuously throughout the function — including
across the actual keyset/offset page fetch calls (~line 1669-1784, which
read/write `state.tie_skip`/`state.offset`/`state.seek_key` inline) — and
is only dropped at the very end (~line 1805), immediately before
`bump_activity()` (~line 1806). **So the state mutex is held for the
ENTIRE duration of a `FetchNext`, including its I/O/scan work, not just
for brief bookkeeping.** This means a `try_lock()` on `cursor.state()`
from the reaper reliably detects "a fetch is currently in progress" for
this cursor: if a fetch is running, `try_lock()` fails; if none is
running, it succeeds immediately (and should be dropped right away,
never held).

## The fix

Add an in-flight check to the reap decision: a cursor is reaped only if
BOTH (a) it is idle by timestamp (`is_expired`) AND (b) 
`cursor.state().try_lock()` succeeds (meaning no `FetchNext` currently
holds it). Two placement options — pick whichever produces the cleaner
diff against the existing structure:

- Fold the `try_lock` check directly into `expired_ids`
  (`crates/shamir-server/src/cursor_registry.rs`, ~line 464-472): change
  its filter predicate to
  `e.value().is_expired(now, idle_ttl) && e.value().state().try_lock().is_ok()`
  — the `try_lock()`'s returned guard is dropped immediately (it's a pure
  availability probe, not something held across the filter), so this adds
  no new locking discipline to reason about.
- Or add a small dedicated method, e.g. `Cursor::is_idle_and_uncontended`,
  wrapping both checks, and use it from `expired_ids` — pick based on
  which reads more naturally against this file's existing style (prefer
  the smaller diff unless a named helper is clearly more readable; check
  how `is_expired` itself is documented/used elsewhere first).

Document the residual: this narrows the race to a MUCH smaller window
than today (a fetch running for the ENTIRE idle_ttl duration can no
longer be reaped mid-flight, since `try_lock` fails throughout), but does
NOT make the check-then-remove sequence fully atomic — `expired_ids`
collects a list, then the reaper loop calls `remove_for_idle_reap` per id
in a SEPARATE pass (~line 544-551 of the sweep). A NEW `FetchNext` could
theoretically start and acquire the lock in the gap between these two
passes. This residual window is many orders of magnitude smaller (single
reaper-tick-local, not idle_ttl-sized) than the bug this task closes, and
matches the review's own suggested mitigation ("reaper should use
try_lock on cursor state and skip active fetches") — do NOT attempt to
make the two passes atomic (e.g. by restructuring the reaper into a
single fused check-and-remove step per DashMap entry with the lock held
throughout) — explicitly out of scope; document the accepted residual in
a code comment near the fix, matching this campaign's existing style for
documented residuals (e.g. `rotate_bootstrap_credential_to_random`'s
"Residual (documented, not fixed)" comment pattern from an earlier task —
grep for it as a style reference).

## Tests

Find or create the test file(s) covering `cursor_registry.rs`'s reaper
behavior (check `crates/shamir-server/src/tests/cursor_registry_tests.rs`
— referenced in this campaign's own commit history — and
`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs` for
existing reaper/expiry coverage first) and add:

1. **Core regression**: a cursor whose `state()` lock is HELD (simulate an
   in-progress fetch by acquiring `cursor.state().lock().await` in the
   test and holding the guard) must NOT be reaped even though its
   `last_activity_nanos` timestamp shows it as idle-expired (construct
   this directly — you likely need to either advance a fake clock or
   construct a `Cursor` whose `created_at` is far enough in the past /
   `idle_ttl` is short enough that `is_expired` returns `true` while the
   lock is deliberately held). Assert `expired_ids` (or whatever the new
   check surfaces as) does NOT include this cursor's id while the lock is
   held, and DOES include it once the lock is released and enough
   (simulated or real, whichever this test file's existing convention
   uses) time has passed with no further activity.
2. **Genuinely idle, unlocked cursor is still reaped** — a regression
   guard proving this fix doesn't accidentally make EVERY cursor
   unreapable; a cursor with no lock contention and a stale timestamp
   must still appear in the expired set.
3. If an existing e2e/integration test already exercises the reaper
   end-to-end (spawn the real reaper task, wait for a tick), consider
   adding a lighter, more deterministic unit-level test for the
   `try_lock`-gated decision instead of relying purely on real time/sleep
   in a new test — check this file's existing conventions for how prior
   reaper tests avoid real-time flakiness (e.g. injectable `Instant`/
   `Duration` params, which `is_expired`/`expired_ids` already accept)
   and follow the same pattern.

## Constraints

- Do NOT change `bump_activity`'s call site or timing (still bumped once,
  at the end of a successful `FetchNext`) — this task's fix is entirely
  on the REAPER side (skip reaping while contended), not a change to when
  activity is recorded.
- Do NOT attempt to make the check-then-remove reap sequence fully atomic
  — see the documented residual above.
- Do NOT change `remove_for_idle_reap`, `remove`, or the tombstone
  mechanism.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, one primary export per
  file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```
