Task: MEDIUM concurrency/durability — crash recovery's `wal.commit`
(WAL-entry truncation marker) runs unconditionally, without the A5
interner-hwm safety gate the normal drainer path uses, and without
persisting the interner — a second crash between recovery and the
first post-recovery checkpoint can leave history entries referencing
interner ids the persistent interner never learns about (audit finding
A11, `docs/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-engine/src/tx/recovery.rs`, `recover_inflight_v2`
  (~line 258-315, confirm current lines): for each replayed inflight
  WAL entry, the loop does:
  1. `replay_v2_entry(&entry, repo).await?` — replays the entry's data
     ops AND its `interner_delta` (via `interner.touch_with_id(name,
     id)`, ~line 356-358) into the IN-MEMORY interner only.
  2. `gate.completion().mark(entry.commit_version, Materialized)` +
     `gate.mark_durable(entry.commit_version)`.
  3. `wal.commit(entry.txn_id).await?;` (~line 299) — **unconditional**.
     This removes the WAL's inflight marker for this entry, meaning a
     SUBSEQUENT crash will NOT re-replay this entry (it looks
     "already fully handled").
  - Contrast with the drainer's `drain_step` (`crates/shamir-engine/src/tx/drainer.rs`,
    ~line 486-520, the "CRIT-2" comment block): the drainer explicitly
    checks `interner_delta_safe_to_truncate(repo, delta_max_id).await`
    (the A5 gate) BEFORE finalizing/truncating, and if the gate says
    "not yet safe" (the interner hasn't been durably checkpointed past
    this entry's max referenced id), it retains the entry's marker
    (does NOT truncate) and logs a debug message instead.
    `recover_inflight_v2` has NO equivalent check — it unconditionally
    calls `wal.commit` regardless of whether the interner's
    `persisted_high_water()` covers this entry's `interner_delta`.
- No call to `repo_interner.persist()` (or equivalent checkpoint) exists
  anywhere in `recover_inflight_v2` — the replayed `(name, id)` pairs
  from every entry's `interner_delta` live ONLY in the in-memory
  interner (`touch_with_id`) after recovery completes; nothing forces
  them to be flushed to the durable interner chunk store.

## Why this is MEDIUM

**Concrete interleaving from the audit ("double crash"):**
1. Repo crashes with tx1's WAL entry inflight; tx1's entry carries
   `interner_delta = [("foo", 42)]` (it introduced a new field name).
2. On restart, `recover_inflight_v2` runs: replays tx1's entry
   (`touch_with_id("foo", 42)` — now IN MEMORY the interner knows id 42
   maps to "foo"), marks it Materialized/durable, and calls
   `wal.commit(tx1.txn_id)` — the WAL's inflight marker for tx1 is
   REMOVED. No A5 check, no interner persist.
3. **Before any background interner checkpoint runs** (the periodic
   checkpoint that flushes `last_persisted_len`/`persisted_high_water`),
   the process **crashes AGAIN**.
4. On the SECOND restart: the WAL has no inflight marker for tx1
   anymore (it was removed in step 2) — `recover_inflight_v2` sees
   NOTHING to replay for tx1. The persistent interner chunk store STILL
   does not know about id 42 (it was never checkpointed — only the
   in-memory interner from the now-dead process 2 knew about it, and
   that's gone). But tx1's data (written to `history` during the first
   recovery's `replay_v2_entry`, which is durable storage, not WAL) IS
   present and references field id 42 as a map key.
5. **tx1's records are now permanently undecodable** — the interner
   never learns id 42's name, in any future run, because the ONE
   WAL-carried record of that mapping was discarded in step 2 without
   confirming it was safe to discard (i.e., without confirming the
   interner had durably absorbed it first).

This is structurally the same failure shape as A8 (interner delta lost
before it's durably absorbed), but manifesting via the RECOVERY path's
premature `wal.commit` instead of a live commit's early lock release —
a distinct code path that needs its own, analogous fix.

## Fix

Per the audit's fix sketch: **after replaying all entries and BEFORE
calling `wal.commit` for entries whose `interner_delta` is not yet
covered by the interner's `persisted_high_water()`, force ONE
`repo_interner.persist()`** so every replayed delta is durably
checkpointed before ANY of this recovery pass's WAL markers are
removed. Concretely:

1. Reuse the drainer's existing gate function
   `interner_delta_safe_to_truncate` (find its exact location — likely
   in `drainer.rs` or a shared helper module; check whether it's
   `pub(crate)` and importable from `recovery.rs`, or needs to become
   so) — OR replicate the equivalent check inline if reuse isn't clean,
   but STRONGLY prefer reusing the exact same gate function so the two
   code paths (drainer's live truncation, recovery's WAL-marker
   removal) can never drift apart in behavior.
2. Simplest correct approach for recovery specifically (since recovery
   is a COLD, infrequent path — unlike the drainer's hot path, there's
   no strong perf reason to avoid an eager persist): after the replay
   loop finishes replaying ALL entries' data (so `interner.touch_with_id`
   has been called for every entry's delta) but BEFORE the
   `wal.commit` loop (or interleaved per-entry — pick whichever is
   simpler and still correct; a single persist after all replays and
   before any commits is simplest and sufficient since `persist()`
   flushes everything currently touched, covering every entry's delta
   in one shot), call `repo_interner.persist().await?` ONCE. Then it is
   safe to `wal.commit` every entry, since the interner now durably
   covers every id any of them referenced.
3. Confirm `persist()`'s failure mode: if it errors, do NOT proceed to
   call `wal.commit` for entries whose delta isn't covered — propagate
   the error (or retain markers and log, mirroring the drainer's
   "conservatively retain" behavior) rather than silently continuing
   as if it succeeded.
4. Alternative (matches the drainer's per-entry granularity more
   precisely, if the eager single-persist-covers-everything approach
   above seems too coarse for review comfort): gate the PER-ENTRY
   `wal.commit(entry.txn_id)` call on
   `interner_delta_safe_to_truncate(repo, entry_interner_max_id(entry)).await?`
   exactly as the drainer does, and if NOT safe, skip `wal.commit` for
   that entry (leave its marker in place) — the NEXT normal drainer
   pass (or a subsequent recovery, if another crash happens) will pick
   it up once the interner catches up. Pick whichever of these two
   approaches (eager one-shot persist, or per-entry gate matching the
   drainer) is simpler to implement correctly and justify the choice
   in your report — the audit's fix sketch names the "persist once,
   or use the A5 gate" as equally acceptable options.

Do NOT change the drainer's existing A5 gate logic itself (CRIT-2's
fix) — only add the equivalent protection to the recovery path, which
currently has none.

## TDD requirement

1. **Red**: write `#[tokio::test]`s (check
   `crates/shamir-engine/src/tx/tests/` for existing recovery test
   modules, likely `recovery_tests.rs` or similar, and follow
   established patterns) that:
   - Reproduce the double-crash interleaving: simulate a WAL with one
     inflight entry carrying an `interner_delta` for a NEW id (above
     the interner's current `persisted_high_water()`); run
     `recover_inflight_v2`; **without the fix**, assert that
     `wal.commit` was called (marker removed) while
     `persisted_high_water()` still does NOT cover the new id — i.e.
     demonstrate the pre-fix window where a second crash would lose
     the mapping. This may require checking WAL/marker state directly
     rather than actually crashing a second time — use whatever
     observable proxy is cleanest given the existing test harness
     (e.g., assert `interner.persisted_high_water() < delta_max_id`
     immediately after `recover_inflight_v2` returns, pre-fix).
   - Post-fix: run the same scenario and assert
     `persisted_high_water() >= delta_max_id` by the time
     `recover_inflight_v2` returns (whichever approach you chose —
     one-shot persist or per-entry gate — the OBSERVABLE invariant is:
     no WAL marker is removed for an entry whose interner delta isn't
     yet durably covered).
   - A regression test confirming the common case (no interner delta,
     or delta already covered by a pre-existing checkpoint) still
     replays and commits markers normally without unnecessary
     persist-overhead-induced failures.
2. **Green**: apply the fix.
3. Confirm existing recovery tests (crash-recovery round-trips,
   multi-entry replay ordering per HIGH-5, idempotent replay) still
   pass.

## Test scope command

```
./scripts/test.sh -p shamir-engine -- recovery
./scripts/test.sh -p shamir-tx
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Which approach was applied: one-shot `persist()` after all replays
  and before any `wal.commit` calls, or a per-entry gate reusing
  `interner_delta_safe_to_truncate` (or an equivalent inline check) —
  and why.
- Whether `interner_delta_safe_to_truncate` was reused directly or
  reimplemented, and if reimplemented, why reuse wasn't feasible.
- The failing-then-passing test evidence for the double-crash
  reproduction.
- Confirmation existing recovery tests (ordering, idempotency,
  round-trip) still pass.
- Full test/gate results (exact commands + pass/fail).
