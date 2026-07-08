Task: HIGH-concurrency — Level-3 pessimistic lock upgrade
(Shared→Exclusive) via the re-entrant path grants Exclusive mode
immediately whenever the REQUESTER already holds the key, WITHOUT
checking whether OTHER transactions also hold it Shared — violating the
single-writer invariant and enabling lost updates / dirty reads at the
lock-protocol level (audit finding A6,
`docs/audits/2026-07-06-concurrency-engine.md`).

## Where

`crates/shamir-tx/src/mvcc_store/mvcc_locks.rs`, `lock_key` (the whole
function spans ~line 41-180; the bug is in the compatibility check at
line 77-99):

```rust
let re_entrant = state.held_by(tx_version);
let compatible = re_entrant
    || state.mode.is_none()
    || (mode == LockMode::Shared && state.mode == Some(LockMode::Shared));

if compatible {
    if !re_entrant {
        state.holders.push(Holder { ... });
    }
    // Set the mode. An Exclusive requester that re-enters a key it
    // already holds Shared upgrades the recorded mode so a later
    // third-tx Shared requester correctly sees conflict.
    state.mode = Some(mode);
    return Ok(());
}
```

`re_entrant = state.held_by(tx_version)` is `true` whenever the
requesting tx ALREADY holds ANY lock on this key (e.g. it previously
acquired Shared via a plain read). The `compatible` check then short-
circuits to `true` via `re_entrant` alone — it does NOT check whether
`state.holders` contains OTHER, DIFFERENT `tx_version`s that also hold
Shared. The `if compatible` branch then unconditionally sets
`state.mode = Some(mode)` (Exclusive), while `state.holders` may still
contain another transaction's Shared holder entry (only pushed for
`!re_entrant`, so the OTHER tx's pre-existing holder entry is
untouched and still present) — violating the invariant (documented at
`key_lock.rs:42-44` per the audit — confirm current line numbers) that
`mode == Exclusive` implies `holders.len() == 1`.

## Why this is HIGH

**Concrete interleaving from the audit:**
1. T1 and T2 (both Pessimistic) both hold Shared locks on key `k`
   (each acquired it via a plain `read_one_tx` call, `acquire_pessimistic_read_lock`
   → `lock_key(k, Shared)`). `state.holders = [T1, T2]`,
   `state.mode = Some(Shared)`.
2. T1 proceeds to `update_tx(k)` → `acquire_pessimistic_write_lock` →
   `lock_key(k, Exclusive)`. `state.held_by(T1) = true` (T1 is already
   in `holders`) → `re_entrant = true` → `compatible = true` (short-
   circuited, `state.holders` still contains T2 but this is never
   checked) → **instant grant**, `state.mode = Some(Exclusive)`,
   `state.holders` STILL `= [T1, T2]` — the documented invariant
   "Exclusive implies exactly one holder" is now violated.
3. T2 (symmetrically) also calls `update_tx(k)` →
   `acquire_pessimistic_write_lock` → `lock_key(k, Exclusive)`. Same
   bug: `state.held_by(T2) = true` → instant grant, `state.mode`
   flips to `Exclusive` again (already was).
4. **Both T1 and T2 now believe they hold an Exclusive lock on `k`
   simultaneously.** Both proceed to read-modify-write `k` under what
   they believe is exclusive access. This is a lost update / dirty
   read at the LOCK-PROTOCOL level — worse than a logic bug, because
   the very primitive meant to prevent this (the lock) is what's
   broken.

## Fix

Per the audit's fix sketch: in the re-entrant branch, when the
requested `mode` is `Exclusive` AND `state.holders` contains ANY OTHER
(different-`tx_version`) holder, do NOT grant immediately — fall
through to the wound-wait conflict-resolution logic (the same
partition-by-age logic already implemented at line ~101-176 for the
non-re-entrant incompatible case) instead of short-circuiting via
`compatible = true`.

Concretely:
1. Change the `compatible` computation so that a re-entrant Exclusive
   request is compatible ONLY if there are no OTHER (different
   tx_version) holders currently on the key. I.e. something like:
   ```rust
   let has_other_holders = state.holders.iter().any(|h| h.tx_version != tx_version);
   let re_entrant = state.held_by(tx_version);
   let compatible = (re_entrant && (mode != LockMode::Exclusive || !has_other_holders))
       || state.mode.is_none()
       || (mode == LockMode::Shared && state.mode == Some(LockMode::Shared));
   ```
   (Adjust the exact boolean shape to fit the existing code's style —
   the key semantic requirement is: a re-entrant Shared→Shared
   re-acquire, or a re-entrant same-mode re-acquire, is ALWAYS fine
   regardless of other Shared holders (multiple readers are
   compatible); but a re-entrant Shared→Exclusive UPGRADE must be
   treated as INCOMPATIBLE whenever any other tx also holds the key,
   so it falls into the wound-wait partition logic below instead of
   being blindly granted.)
2. Ensure the existing partition-by-age logic (lines ~101-176) handles
   this case correctly when it now receives it: for T1 (older) vs T2
   (younger), T1's Exclusive-upgrade request should WOUND T2 (removing
   T2's Shared holder entry) and then loop to retry (now compatible,
   since T2 is gone). For the symmetric case — T2 requesting the
   upgrade while T1 (older) still holds Shared — T2 must WAIT (per
   the audit: "wound-wait по tx_id разрулит (младший будет wounded)" —
   confirm the existing age-comparison branch (`tx_version <
   h.tx_version` → wound, `tx_version > h.tx_version` → wait) already
   produces the correct outcome once the re-entrant short-circuit no
   longer bypasses it; you likely do NOT need to change the partition
   logic itself, only the `compatible` gate that decides whether to
   enter it).
3. Double-check the "same-tx holder" skip inside the partition loop
   (`if h.tx_version == tx_version { continue; }`, line ~117) still
   correctly ignores the REQUESTER'S OWN existing holder entry when
   iterating — the requester's own prior Shared holder record should
   neither be wounded nor cause a wait; only OTHER tx's holder entries
   should be evaluated for wound/wait.
4. Confirm the eventual grant (after wounding conflicting others, or
   after waiting for an older holder to release) correctly updates
   `state.mode` to `Exclusive` and leaves `state.holders` containing
   ONLY the requester (i.e. after the fix, the invariant "Exclusive ⇒
   len(holders) == 1" holds in every case, not just the originally-
   handled non-re-entrant path).

Do NOT change the release/wound notification mechanics
(`wound_notify`, `lock.notify`) or `release_locks` — those are
correct and out of scope; only the `compatible` gate inside `lock_key`
needs to change.

## TDD requirement

1. **Red**: write `#[tokio::test]`s in
   `crates/shamir-tx/src/mvcc_store/tests/` (check the existing test
   module layout for `mvcc_locks`/lock-related tests first — likely a
   `mvcc_locks_tests.rs` or similar, or check where the existing
   `pessimistic_deadlock_freedom_wound_wait_terminates`-style tests
   live per the A4 fix commit's report) that:
   - Reproduce the EXACT A6 interleaving: T1 and T2 both acquire
     Shared on key `k` (via `lock_key(k, T1_version, Shared)` and
     `lock_key(k, T2_version, Shared)` directly against `MvccStore`, or
     via the higher-level `acquire_pessimistic_read_lock` if that's a
     cleaner test surface — check which is more natural given existing
     test patterns). Then have T1 (assume T1 is the OLDER tx_version)
     request `lock_key(k, T1_version, Exclusive)`.
   - **Before the fix**: this call returns `Ok(())` IMMEDIATELY even
     though T2 still holds Shared — assert this is the CURRENT (buggy)
     behavior is what you're reproducing, i.e. write the test to assert
     the CORRECT post-fix behavior and confirm it fails pre-fix.
   - **After the fix**: T1's Exclusive request should either (a) wound
     T2 and succeed once T2 is removed (if T1 is older), producing
     `state.holders == [T1 only]` and `state.mode == Exclusive`; or
     wait/eventually succeed once T2 releases. Assert the invariant
     directly if `MvccStore`/`KeyLock` exposes an introspection hook
     (check for one — e.g. a test-only accessor for `holders.len()`
     or `mode`); if no such hook exists, assert via observable
     behavior instead: e.g. after T1's upgrade "succeeds", have a THIRD
     tx T3 attempt a Shared lock on `k` and confirm it correctly BLOCKS
     (or conflicts) because T1 now holds a genuine, exclusive-with-
     single-holder lock — this proves the invariant indirectly through
     the lock's observable behavior.
   - Also test the SYMMETRIC case: T2 (younger) requests the Exclusive
     upgrade while T1 (older) still holds Shared — T2 must WAIT (not
     get an instant grant), and should eventually be WOUNDED or succeed
     only after T1 releases, per wound-wait's existing age rules.
2. **Green**: apply the fix.
3. Confirm existing pessimistic-lock tests (from the A4 fix and any
   pre-existing `mvcc_locks`/wound-wait tests) still pass — in
   particular, a SINGLE tx acquiring Shared then upgrading to
   Exclusive on a key IT ALONE holds (no other concurrent holder) must
   still get an instant grant (this is the common, correct case the
   fix must not break) — write or confirm an existing test covers this
   exact "solo upgrade, no other holders" path explicitly.

## Test scope command

```
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine -- pessimistic
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-tx -- --check
cargo clippy -p shamir-tx --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- The exact new `compatible` gate logic (before/after) and confirmation
  it correctly distinguishes "solo re-entrant upgrade" (still instant-
  grant) from "re-entrant upgrade with other Shared holders present"
  (now correctly routed into wound-wait).
- Confirmation the existing partition-by-age (wound/wait) logic handles
  the newly-routed case correctly with NO changes needed to that logic
  itself (or, if changes WERE needed, what and why).
- The failing-then-passing test evidence for both the "requester older,
  wounds the other holder" and "requester younger, must wait" cases.
- Confirmation the "solo upgrade, no other holders" fast path still
  works (instant grant, no regression).
- Full test/gate results (exact commands + pass/fail).
