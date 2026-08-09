# Brief 53 — #1057 round 2: end-to-end data-survival test, not just min_alive()

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 1 got right

The production code (`TableManager::open_index_build_snapshot` in
`table_manager.rs`) is correct — do not touch it. The reachability finding
(`changefeed.gate.open_snapshot()`) and the guard-first-then-`.version()`
pattern are both right.

## What's missing — the tests check `min_alive()`, not actual data survival

`crates/shamir-engine/src/table/tests/p1057_snapshot_guard_tests.rs`'s 3
tests all assert on `gate.min_alive()`'s numeric value. That's a proxy, one
level removed from the actual property that matters: **does data physically
survive a real GC pass while the guard is held, and get correctly lost only
after release?** None of the 3 tests calls `mvcc.gc_below(...)` against a
scenario where data WOULD be destructively reclaimed, and none reads the
pinned version back via `snapshot_stream` (#1056) to prove the value is
still there. `anti_gc_fails_without_guard` in particular never even calls
`open_index_build_snapshot` — it just re-demonstrates `RepoTxGate`'s
pre-existing `min_alive()`/`last_committed()` relationship, which is true
regardless of whether this task's new method exists at all. It does not
discriminate this task's code from a no-op.

## The mechanism you need to actually exercise

`MvccStore::gc_below(min_version)` (`crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:300-350`):
for each key, it keeps the LATEST version strictly `< min_version` (the
"anchor" — needed so a snapshot between the anchor and `min_version` can
still resolve) and deletes every OLDER version of that key. Critically: if a
key has versions v1 < v2 < v3, and you call `gc_below(v3.version + 1)`
(or any `min_version` > v3), the anchor becomes v3, and v1 AND v2 both get
deleted — even if v2 is the version something has "pinned". This is exactly
the hazard: without a guard holding `min_alive()` down at the pin, the
production GC caller (which passes `gate.min_alive()` as `gc_below`'s
argument) will delete a pinned-but-unprotected version once new writes
advance the watermark far enough.

## Write ONE new test: end-to-end pin-survives-real-GC

Add to the SAME file (`p1057_snapshot_guard_tests.rs`), a new test,
`pinned_version_survives_real_gc_pass_with_guard_held`:

1. Use `make_table_with_mvcc_and_changefeed()` (already in the file).
2. Write a SINGLE key with 3 distinct versions: v1 (`gate.publish_committed_max(1)`),
   v2 (`publish_committed_max(2)`), v3 (`publish_committed_max(3)`) — same
   key, 3 different values, so all 3 physically exist in `history` before
   any GC.
3. Acquire the guard via `tbl.open_index_build_snapshot().await`. Confirm
   `guard.version() == 2` (i.e., pin at v2 — NOT the latest write).

   Wait — think about WHEN to acquire the guard relative to the writes.
   For this test to be meaningful, the guard must be acquired to pin at v2,
   with v3 written AFTER the guard is acquired (so the guard is protecting a
   version that is NOT the newest, exactly the online-build scenario: Phase A
   pins version V, then MORE writes land while Phase A is scanning). Reorder:
   write v1, write v2, acquire the guard (pin = 2), THEN write v3.
4. **Call `mvcc.gc_below(gate.min_alive())` directly** (the real production
   GC caller pattern — NOT a hardcoded version number). With the guard held,
   `gate.min_alive()` returns 2 (the pin), so `gc_below(2)` deletes nothing
   below version 2 that matters (v1 might be reclaimed as the non-anchor
   below the pin — that's fine, v1 isn't what Phase A needs; what matters is
   v2 SURVIVES).
5. **Read back via `MvccStore::snapshot_stream(batch, pin)`** (#1056's
   primitive) — assert the key's value equals v2's value, not v3's, and that
   the read succeeds at all (proving v2's history entry was NOT deleted by
   the `gc_below` call in step 4).
6. Drop the guard. Call `mvcc.gc_below(gate.min_alive())` AGAIN — now
   `min_alive()` tracks `last_committed()` (3), so this second call's anchor
   for the key becomes v3, and v2 (no longer the anchor, no longer protected
   by any guard) is now eligible for deletion. Confirm — however you choose
   to verify this (row count in `history` before/after, or a subsequent
   `snapshot_stream(batch, pin=2)` now returning nothing or erroring for
   that key) — that v2 is gone. This is the "without the guard, data is
   lost" side of the proof, in the SAME test, not a separate weaker test.

## Do not remove the 3 existing tests

They're not wrong, just insufficient alone — keep them as fast unit-level
checks of the `min_alive()` mechanism, and add the new one as the real
end-to-end proof. If time is tight, the new test is the one that matters
most.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p1057
```

Report the exact diff and the exact nextest output for all 4 tests in this
file (3 existing + 1 new), all PASS.
