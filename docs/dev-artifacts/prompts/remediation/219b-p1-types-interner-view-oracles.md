BINDING ORACLE DESIGN (from @oh read-only design review; line numbers refer to this worktree's current interner.rs / messagepack.rs / common.rs):

## 1EBA1D8A
Hook: add a third closure `between_publications: impl FnOnce(u64)` to `touch_ind_impl_with`, exposed via widened `#[cfg(test)] touch_ind_with_test_hooks(s, after_reservation, after_id_allocation, between_publications)` (production passes no-ops). Invoke in `touch_ind` after the `set_reverse_slot_locked` check (interner.rs:266-269) and before `vacant.insert` (:270). Same seam in `touch_with_id_impl_with` between :497 and :498, exposed as `#[cfg(test)] touch_with_id_with_test_hook(name, id, hook)`. Semantics: runs after the first publication write, before the second.
Test:
```rust
let i = Interner::new(); let g0 = i.generation(); let mut seen = None;
i.touch_ind_with_test_hooks("auto", ||{}, |_|{}, |id| {
    // lock-free reads only: shard write guard + writer mutex are held
    seen = Some((i.get_str(&InternerKey::new(id)), i.generation()));
}).unwrap();
assert_eq!(seen, Some((Some("auto".into()), g0)));
// same for touch_with_id_with_test_hook("explicit", 9, ..) → Some("explicit")
```
Inside the hook NEVER call get_ind/touch_* (VacantEntry holds the DashMap shard write lock → deadlock). Pre-fix fails: vacant.insert dropped its guard before set_reverse_slot → get_str None at the midpoint, and generation already g0+1. No separate rollback test needed (forward insert is now last).

## 3CFA3FB6
Hook: `after_id_allocation: impl FnOnce(u64)` at interner.rs between :254 and :255 (after checked_add, still under the lock). Probe: `#[cfg(test)] pub(crate) fn publication_lock_held_for_test(&self) -> bool { matches!(self.reverse_write_lock.try_lock(), Err(TryLockError::WouldBlock)) }`.
Test (REPLACES the current one at publication_cards.rs:47-83):
```rust
let i = Arc::new(Interner::new()); let mut late = None; let mut early = None;
let k = i.touch_ind_with_test_hooks("automatic", ||{}, |id| {
    if i.publication_lock_held_for_test() {   // serialized: competitor must wait
        let c = Arc::clone(&i);
        late = Some(thread::spawn(move || c.touch_with_id("explicit", id)));
    } else {                                   // unlocked allocator: replay the window
        early = Some(i.touch_with_id("explicit", id));
    }
}, |_|{}).unwrap().into_key();
assert_eq!(i.get_str(&k).as_deref(), Some("automatic"));
let r = late.map(|h| h.join().unwrap()).or(early).unwrap();
assert!(matches!(r, Err(InternerError::IdConflict { id, .. }) if id == k.id()));
```
(adapt to the real error variant / key accessor names.)

## 58E7E6DC
No counting global allocator (would affect the whole lib test binary). Pointer-provenance probe at the sink `codecs/interned/common.rs:13` (`intern_string_key`), copying the thread-local pattern of `record_view/lens/cursor.rs:6-19`:
```rust
#[cfg(test)] thread_local!{ static LAST_KEY_PTR: Cell<usize> = const { Cell::new(0) }; }
#[cfg(test)] pub(crate) fn last_intern_key_ptr() -> usize { LAST_KEY_PTR.with(Cell::get) }
// first line of intern_string_key:
#[cfg(test)] LAST_KEY_PTR.with(|c| c.set(key_str.as_ptr() as usize));
```
Test: reuse bytes of messagepack_tests.rs:11-24, then `assert_eq!(last_intern_key_ptr(), bytes[6..].as_ptr() as usize)` (key starts after 0x81 0xdb + 4-byte len; verify the offset against the actual bytes). A temporary String fails it.

## Refund
- touch_ind Occupied (:257-258): already covered by quota_tests.rs:64 — keep.
- reverse-set failure branches (:266-268, :494-495): `#[cfg(test)] thread_local! FAIL_NEXT_REVERSE_SET: Cell<bool>` consumed at the top of `set_reverse_slot_locked` (:306) returning false. Arm it, call touch_ind("x") and touch_with_id("y", 5): assert Err(IdCollision), quota_usage()==(0,0), get_ind None, generation unchanged; then touch_ind("x") gets id 1.
- touch_ind overflow (:251-253): `#[cfg(test)] fn set_current_id_for_test(&self, v: u64)`; set u64::MAX, touch_ind with quota, assert usage (0,0).
- touch_with_id Occupied (:477-478) is dead: move `count_unmetered` (:472) to after `vacant.insert` (:498) and delete both touch_with_id refunds (no accounting before a failure point).
- :262-264 usize::try_from: unreachable on 64-bit — DEFERRED, note it in the final message.

## P3
interner_manager.rs:254-261 comment → "Seed the cursor from the end of the contiguous hydrated ID prefix (largest k with 1..=k all present). Do not use the tuple count, which duplicate recaptures inflate. Do not use the max ID: an ID later filled into a gap below max would be skipped. Persist rescans already-known IDs above the cursor." Drop "leaked raced touch_ind reservations" (:257). interner_manager_tests.rs:402-403 doc → contiguous prefix; rename `..._uses_max_id_not_tuple_count` → `rv_types_2a1ab215_hydrated_cursor_is_contiguous_prefix` (no active rust-guard campaign, rename is safe).

All hooks are `#[cfg(test)]`; no production behaviour change except the count_unmetered move.
