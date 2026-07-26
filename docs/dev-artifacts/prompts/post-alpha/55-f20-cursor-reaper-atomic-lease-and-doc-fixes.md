# Brief for F-20 (#813, P1/P2 combined) — cursor-reaper atomic in-flight lease + doc residuals

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-9 (#799, already landed) added a `try_lock()` probe to
`CursorRegistry::expired_ids` (`crates/shamir-server/src/cursor_registry.rs`,
~line 494-500) so the reaper skips a cursor whose `state()` mutex is
currently held by an in-flight `FetchNext`. That closed the BIG bug (a
long-running fetch being reaped mid-flight because `last_activity_nanos`
only moves once, at the very END of a successful fetch).

But `expired_ids` only **collects a list** — the actual removal happens in
a **separate, later pass** in `spawn_reaper_task`'s sweep loop
(~line 569-581):

```rust
let expired = registry.expired_ids(now, idle_ttl);
...
for id in expired {
    let _ = registry.remove_for_idle_reap(id);   // unconditional — no re-check
}
```

`remove_for_idle_reap` (~line 427-432) removes the entry from `self.open`
(a `DashMap<u64, Arc<Cursor>, THasher>`) **unconditionally** — it does NOT
re-check `try_lock()` at the point of actual removal. So: a `FetchNext`
that calls `get_owned` (~line 393-412) and then acquires
`cursor.state().lock().await` (`db_handler/cursor_handlers.rs`, ~line 1511)
**after** `expired_ids` already collected this cursor's id but **before**
the removal loop reaches it, will have its cursor yanked out from under it
mid-fetch anyway — the exact bug F-9 was supposed to close, just narrowed
to a single-reaper-tick-sized window instead of an `idle_ttl`-sized one.
This was explicitly flagged as an accepted residual in F-9's own doc
comment (~line 483-493) and by two independent post-wave reviews (N-3 in
`docs/dev-artifacts/research/2026-07-26-wave-f-post-review/REPORT.md`; R6
in `docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`).
The third review escalated this from "just document the residual" to
"actually fix it via an atomic in-flight lease, checked at removal time" —
that escalation is this task's Part A.

## Part A (priority, P1) — atomic in-flight lease, checked atomically at removal

### Design: an `in_flight` counter on `Cursor`, incremented before the registry read-guard is released, checked by the SAME atomic step that performs removal

The key correctness property to establish: the moment a `FetchNext` marks
itself "in flight" for a cursor must be ordered **relative to the
reaper's removal decision** by the SAME lock the registry already uses for
that entry — not by a separate, independently-timed check. `dashmap`'s
per-shard `RwLock` already gives us this for free if we do the increment
**while still holding the `Ref` guard returned by `self.open.get(...)`**,
and do the reap decision (expiry + in-flight check + actual removal) as
ONE atomic step per entry via `DashMap`'s `remove_if`/`retain` (which runs
its predicate while holding that shard's write lock — see
`free_session_slot`, ~line 457-462, which already documents and relies on
this exact `dashmap` 6.1.0 guarantee for a different invariant; read that
doc comment for the citation of *why* this ordering guarantee holds).

Why this closes the residual that a plain re-check-`try_lock`-at-removal
would NOT fully close: even if removal re-checked `try_lock()` at the
exact moment of removal, there is still a gap between "`get_owned` returns
the `Arc<Cursor>`" and "the caller actually calls
`cursor.state().lock().await`" during which `try_lock()` would still
succeed (nothing is locked yet) — a reaper sweep landing in that exact
gap would remove the cursor even though a fetch is genuinely about to use
it. An `in_flight` counter incremented **inside the same critical section
as the registry lookup** (before the read-guard for that entry is
dropped) has no such gap: any reaper removal attempt for that key must
run either strictly before the lookup starts (in which case the lookup
observes `None` after removal, or observes the fresh row if lookup runs
first — see below) or strictly after the lookup's read-guard is dropped —
by which point the increment (which happened before that drop) is already
visible. There is no interleaving where the removal's predicate can
observe `in_flight == 0` while a lookup that already returned `Ok` is
still "between get and lock".

### Concrete steps (adapt to what reads cleanest against the existing file structure — this is the target shape, not a literal diff)

1. Add `in_flight: AtomicU32` (or `AtomicUsize`, match whatever integer
   width this file's other atomics already use — check `last_activity_nanos`'s
   type for the file's convention) to `Cursor`'s struct definition,
   initialized to `0` wherever `Cursor::new`/its constructor sets up the
   other atomic fields (`created_at`, `last_activity_nanos`, etc.).

2. Add a new registry method — do **not** modify `get_owned`'s existing
   signature/behavior (it's used by `cancel_cursor`, by `tx_registry`'s
   analogous method, and by ~10 existing tests in
   `crates/shamir-server/src/tests/cursor_registry_tests.rs` that assert
   its current `Result<Arc<Cursor>, CursorRegistryError>` shape — changing
   it would be a much bigger, unjustified diff). Instead add a sibling,
   e.g. `get_owned_for_fetch`, used ONLY by `fetch_next`:
   ```rust
   pub fn get_owned_for_fetch(
       &self,
       cursor_id: u64,
       sid: &[u8; 32],
   ) -> Result<(Arc<Cursor>, FetchLease), CursorRegistryError> {
       let arc = match self.open.get(&cursor_id) {
           Some(r) => {
               // Increment WHILE the shard read-guard `r` is still alive —
               // this is the ordering property the whole design rests on.
               r.value().in_flight.fetch_add(1, Ordering::AcqRel);
               Arc::clone(r.value())
           }
           None => {
               return Err(if self.reaped_tombstones.contains_key(&cursor_id) {
                   CursorRegistryError::CursorExpired
               } else {
                   CursorRegistryError::CursorNotFound
               });
           }
       };
       if &arc.owner_sid != sid {
           arc.in_flight.fetch_sub(1, Ordering::AcqRel); // undo — no lease returned on this error path
           return Err(CursorRegistryError::CursorOwnershipMismatch);
       }
       let lease = FetchLease(Arc::clone(&arc));
       Ok((arc, lease))
   }
   ```
   `FetchLease` is a small RAII guard (new type in `cursor_registry.rs`,
   the same file, since it's tightly coupled to `Cursor`/`CursorRegistry`
   — this file already groups `Cursor`, `CursorRegistry`, and their
   errors together, so a small guard type is consistent with its existing
   grouping, not a "one file one export" violation):
   ```rust
   /// RAII lease marking a `FetchNext` in flight against a cursor. Held
   /// for the full duration of the fetch (from lookup to completion),
   /// decremented on drop — including every early-return path, since
   /// `fetch_next` has several. See F-20 (#813) / F-9 (#799).
   pub struct FetchLease(Arc<Cursor>);

   impl Drop for FetchLease {
       fn drop(&mut self) {
           self.0.in_flight.fetch_sub(1, Ordering::AcqRel);
       }
   }
   ```

3. Update `fetch_next` (`crates/shamir-server/src/db_handler/cursor_handlers.rs`,
   ~line 1483-1489) to call `get_owned_for_fetch` instead of `get_owned`,
   binding the returned lease to a variable held for the function's full
   body (e.g. `let (cursor, _lease) = match ... { Ok(pair) => pair, Err(e) => return ... };`)
   so it drops (decrementing `in_flight`) on EVERY exit path — verify this
   covers every existing early `return` in the function (there are several:
   the `exhausted` early-return ~line 1512-1521, and others later in the
   function — grep for `return DbResponse` / `return error_response` within
   `fetch_next`'s body and confirm the lease variable is in scope for all
   of them, i.e. bound before the first possible early return and not
   shadowed/dropped early).

4. Investigate whether `in_flight` should now be the ONLY reap-gating
   check (replacing `try_lock()` on `cursor.state()` entirely, since
   `in_flight` covers a strict superset of the window `try_lock()`
   covered — from lookup to full completion, not just from
   `state().lock()` to `drop(state)`), or whether both checks should
   remain (belt-and-suspenders). Prefer replacing `try_lock()` with
   `in_flight` if the investigation confirms the superset claim holds
   (it should — read `fetch_next` end-to-end once more to confirm there's
   no code path between `get_owned_for_fetch` returning and the function's
   final exit that ISN'T covered by holding the lease) — a single,
   understood mechanism beats two overlapping ones. Document whichever
   choice is made and why.

5. Restructure the reaper's collect-then-remove two-pass sweep
   (`expired_ids` + `spawn_reaper_task`'s loop) into ONE atomic-per-entry
   pass. `dashmap` provides `retain(&self, f: impl FnMut(&K, &mut V) -> bool)`
   — entries where the predicate returns `false` are removed, each
   removal happening while that shard's write lock is held (same
   guarantee `free_session_slot` already relies on for `remove_if`).
   Replace `expired_ids` + the reaper's external removal loop with a
   single method, e.g.:
   ```rust
   /// Sweep idle-expired, uncontended cursors in ONE atomic-per-entry
   /// pass — the expiry check, in-flight check, and removal decision for
   /// each entry happen while that entry's shard write-lock is held, so
   /// there is no gap between "decided expired+idle" and "actually
   /// removed" for a NEW fetch to land in. See F-20 (#813).
   pub fn sweep_and_reap(&self, now: Instant, idle_ttl: Duration) -> usize {
       let mut reaped_owners: Vec<[u8; 32]> = Vec::new();
       let mut reaped_ids: Vec<u64> = Vec::new();
       self.open.retain(|id, cursor| {
           let expired = cursor.is_expired(now, idle_ttl)
               && cursor.in_flight.load(Ordering::Acquire) == 0;
           if expired {
               reaped_owners.push(cursor.owner_sid);
               reaped_ids.push(*id);
           }
           !expired // keep everything that is NOT being reaped
       });
       for owner in &reaped_owners {
           self.free_session_slot(owner);
       }
       let now_ts = Instant::now();
       for id in &reaped_ids {
           self.reaped_tombstones.insert(*id, now_ts);
       }
       reaped_ids.len()
   }
   ```
   (Adjust field/method names to match whatever the real `Cursor` struct
   exposes for `owner_sid` — check its visibility; `retain`'s closure
   gets `&mut V` so reading `cursor.owner_sid`/`cursor.in_flight` needs
   whatever visibility level the struct already uses internally within
   this same file — likely `pub(crate)` fields or existing accessor
   methods; do not widen visibility beyond what's needed.) Update
   `spawn_reaper_task` to call `sweep_and_reap` instead of
   `expired_ids` + a manual loop; keep the existing `tracing::info!`
   log line reporting the reaped count. Remove `expired_ids` and
   `remove_for_idle_reap` if they become fully unused afterward — check
   whether tests reference them directly (several do, per
   `cursor_registry_tests.rs`) and update those tests to exercise
   `sweep_and_reap` instead if so (do not leave dead pub methods with no
   callers AND no tests — either keep them genuinely used or remove them
   and update the tests that named them).

### Tests (Part A)

1. **Core regression — the residual this task closes**: a cursor is
   looked up via `get_owned_for_fetch` (simulating a fetch that has
   started but not yet finished — hold the returned `FetchLease` for the
   duration of the assertion), its timestamp made to look idle-expired,
   and `sweep_and_reap`/the new reap decision run concurrently (or
   sequentially, simulating the interleaving) — assert the cursor is
   **NOT** reaped while the lease is held, matching F-9's original test
   intent but now covering the specific gap F-9 left open (i.e., construct
   the test so the "lease held, `state()` NOT locked" case — which F-9's
   `try_lock`-only check would have gotten wrong — is exercised, not just
   the "state() locked" case F-9 already tested).
2. **Lease dropped → cursor is reaped normally** — once the `FetchLease`
   guard is dropped (simulating fetch completion) and the cursor remains
   idle past `idle_ttl`, it IS reaped.
3. **Genuinely idle, never-fetched cursor is still reaped** — regression
   guard that this doesn't make cursors permanently unreapable.
4. Keep or adapt F-9's existing `try_lock`-based tests in
   `cursor_registry_tests.rs` / `cursor_handler_tests.rs` — if `try_lock`
   is removed per step 4 above, update those tests to exercise the new
   `in_flight`-based mechanism instead (same intent, new mechanism); if
   both checks are kept, the old tests should still pass unchanged.
5. If a genuine concurrent repro is feasible (spawn a task holding the
   lease, spawn the reaper sweep concurrently, assert ordering via a
   channel/barrier), prefer it over a purely sequential simulation — but
   only if it can be made deterministic (no real-time sleeps); otherwise
   a sequential/injected-clock unit test making the interleaving explicit
   is acceptable, matching this file's existing convention for avoiding
   real-time flakiness in reaper tests.

## Part B (small, doc-only) — KNOWN_LIMITATIONS.md residuals + F-10 cross-reference fix

1. **Document F-9/#799 and this task's closure of its residual.**
   `docs/guide-docs/KNOWN_LIMITATIONS.md` currently has NO entry at all
   for the cursor-reaper in-flight problem (confirmed: grepped the file,
   zero matches for "reap"/"in_flight"/"idle_ttl"/"F-9"). Add a new bullet
   (match the file's existing bullet style/voice — see the `F-18`/`F-26`
   entries already there for the pattern) describing: the original
   problem (a long fetch could be reaped mid-flight because
   `last_activity_nanos` only moves at fetch completion), F-9's first fix
   (`try_lock` probe), the residual F-9 left open (collect-then-remove
   two-pass gap), and F-20/#813's closure (atomic `in_flight` lease +
   single-pass `sweep_and_reap`). Mark it CLOSED, not open — this task
   fixes the residual, it doesn't just document it.
2. **Document F-11/#802's still-open `deploy/server.example.ktav` gap.**
   `max_inflight_response_bytes` already appears in
   `deploy/server.medium.example.ktav` and
   `deploy/server.small.example.ktav` but is missing from the base
   `deploy/server.example.ktav` — confirmed via grep. This specific ktav
   edit is tracked as its own task (#814, F-21) — do NOT make that edit
   here, only add a one-line documented-residual bullet in
   `KNOWN_LIMITATIONS.md` noting the gap and pointing at the follow-up
   (mirror how other cross-references to a tracked follow-up task read
   elsewhere in this same doc, e.g. the F-10 sibling-files bullet already
   does this for its own follow-up).
3. **Fix F-10's cross-reference inaccuracy.** The existing
   `KNOWN_LIMITATIONS.md` bullet about corrupt-record reporting (search
   for "sibling files were explicitly left out of scope") cites BOTH
   `table_manager_index_mgmt.rs` and `table_manager_streaming.rs` as
   having "the same `Err(_) => continue` pattern" as `read_exec.rs`'s
   corrupt-VALUE-decode skips. **Verified by direct read: this is only
   true for `table_manager_streaming.rs`** (genuine value-decode-failure
   skips at ~line 185, ~321-323, matching the real pattern).
   `table_manager_index_mgmt.rs` has exactly ONE `continue` in the whole
   file (~line 854, inside `migrate_sorted_index_entries` or its
   equivalent rename-helper), and it is a **malformed-KEY length guard**
   (`if key.len() < 9 { continue; // malformed; skip }`) during a
   background index-rename key migration — an entirely different
   mechanism, not a corrupt record VALUE that got silently dropped from a
   query result. Correct the doc bullet to remove
   `table_manager_index_mgmt.rs` from the "still has the same
   corrupt-record-skip pattern" claim (either drop it from that sentence
   entirely, or — if you judge it's still worth a one-line mention for a
   different reason — describe it accurately as an unrelated
   defensive-skip in the rename-migration path, not a corrupt-record gap).
   Re-verify the `table_manager_streaming.rs` citation's accuracy the same
   way (it should hold up) before leaving it in place.

## Constraints

- Part A: do NOT change `bump_activity`'s call site or timing.
- Part A: do NOT widen any field/method visibility beyond `pub(crate)`
  unless the existing file already does so for sibling members.
- Part A: if you keep `try_lock()` alongside `in_flight` (per step 4's
  investigation outcome), do not remove the existing F-9 doc comment
  wholesale — update it to describe the now-closed residual instead.
- Part B: do NOT implement the `deploy/server.example.ktav` edit here —
  that's #814/F-21, a separate task. Only document the gap.
- Part B: do NOT touch `table_manager_streaming.rs` or
  `table_manager_index_mgmt.rs` themselves — doc-only correction.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, one primary export per
  file (or match this file's existing multi-small-type precedent for
  `Cursor`/`CursorRegistry`/errors, per Part A's design section above),
  surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- cursor_registry
./scripts/test.sh -p shamir-server -- cursor_handler
./scripts/test.sh -p shamir-server --full
```
