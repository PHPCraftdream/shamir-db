Task: HIGH-concurrency — Level-3 Pessimistic isolation loses updates
because (a) exclusive locks are released BEFORE the write becomes
visible (before `materialize`/`apply_data_phase`), and (b) a read taken
UNDER a pessimistic lock still resolves against the transaction's OLD
snapshot instead of the latest committed value, defeating the purpose
of taking the lock (audit finding A4,
`docs/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-engine/src/tx/commit.rs`:
  - Async path (`commit_tx_inner`, the non-lockfree branch): line 537
    `release_pessimistic_locks(&tx, repo).await;` runs BEFORE line 543
    `apply_data_phase(&mut tx, repo, commit_version).await;` (Phase 5a —
    the actual publish of the write, making it visible to other
    readers/lockers).
  - Lockfree path (`commit_tx_lockfree`): line 728
    `release_pessimistic_locks(&tx, repo).await;` runs BEFORE line 736
    `materialize(&mut tx, repo, version_guard, validated.uwl_guards).await;`
    (which performs the equivalent publish).
  - (Confirm exact current line numbers before writing the fix — they
    may have drifted since 2026-07-06, but the shape — release-then-
    publish instead of publish-then-release — is what matters.)
- `crates/shamir-engine/src/table/table_manager_streaming.rs`,
  `read_one_tx` (~line 331-398, see `docs/prompts/audit/16-*` for the
  exact current lines from the A3 fix — line numbers shifted slightly
  after that commit): after `acquire_pessimistic_read_lock`/an
  exclusive-lock acquisition, the actual read still goes through
  `mvcc.get_at(key.as_ref(), tx.snapshot_version).await?` — i.e. it
  resolves against the transaction's ORIGINAL snapshot version, not the
  LATEST committed value at the moment the lock was actually granted.

## Why this is HIGH

Two independent, compounding breakages:

**(a) 2PL violation — early lock release.** Two-phase locking requires
that ALL locks a transaction holds stay held until the transaction's
outcome (commit/abort) is fully visible to others. Releasing the
exclusive lock on a key BEFORE that key's new value is published
(`apply_data_phase`/`materialize`) means a second transaction can
acquire the lock and read/write the key while the FIRST transaction's
write is still in-flight (WAL-durable but not yet cell-published). This
defeats the entire purpose of taking an exclusive lock under
Pessimistic isolation.

**(b) Snapshot-stale read under a held lock.** Even if lock ordering
were fixed, a transaction that has just acquired a lock and is about to
read a key for a read-modify-write cycle reads via
`get_at(key, tx.snapshot_version)` — its ORIGINAL snapshot, taken when
the transaction began. If another transaction committed a newer
version of that key AFTER this transaction's snapshot was taken but
BEFORE this transaction acquired the lock (e.g. it had to wait for the
lock), the read returns STALE data even though the transaction now
holds an exclusive lock and should be entitled to see the true latest
committed value. Under Pessimistic isolation specifically, a lock
holder reading "the current value under lock" should mean "the
LATEST COMMITTED value", not "whatever my snapshot happened to be when
I started".

**Concrete interleaving from the audit:**
1. T1 (Pessimistic): acquires X-lock on `k`, reads `k=v0`, stages
   `k=v1` in its write-set.
2. T1 commits: WAL write succeeds → **locks released** (bug a) → Phase
   5a (`apply_data_phase`/`materialize`, the actual cell publish) has
   NOT run yet.
3. T2 (Pessimistic, began EARLIER than T1, so its snapshot predates
   T1's commit): was waiting on the X-lock for `k`; now that T1
   released it (prematurely), T2 acquires the X-lock.
4. T2 calls `read_one_tx(k)` → `get_at(k, snapshot_T2)` → returns `v0`
   (T2's OLD snapshot, predating T1's commit — even though T1's write
   may ALREADY be durably committed and even published by now, T2
   still sees the stale snapshot value because the read path doesn't
   distinguish "under an exclusive Pessimistic lock" from "ordinary
   snapshot read").
5. T2 stages `k=v2` (computed from the stale `v0`) and commits.
6. **T1's update is lost** — T2 overwrote it with a value computed
   from data that predates T1's write, even though T2 held an
   exclusive lock the entire time it "should have" seen T1's write.
   Wound-wait (the deadlock-avoidance mechanism) does not help here:
   both transactions "successfully" acquired their locks in sequence
   and both committed — there was no wait-cycle to detect, just a
   protocol violation.

## Fix

Two independent, additive fixes — apply both:

1. **Move `release_pessimistic_locks` to AFTER the publish step**, on
   BOTH commit paths (`commit_tx_inner`'s async branch and
   `commit_tx_lockfree`). Specifically:
   - Async path: move the `release_pessimistic_locks(&tx, repo).await;`
     call (currently before `apply_data_phase`) to AFTER
     `apply_data_phase` completes (after the cell-publish/
     `finalize_reservation` step, i.e. after the `cell_guards` disarm
     block, or wherever `apply_data_phase`'s effects are guaranteed
     visible — check the exact ordering `apply_data_phase` establishes
     before choosing the precise insertion point).
   - Lockfree path: move the equivalent
     `release_pessimistic_locks(&tx, repo).await;` (currently before
     `materialize`) to AFTER `materialize` completes.
   - Check EVERY other call site of `release_pessimistic_locks` in
     `commit.rs` (there are several — early-abort paths at lines ~435,
     440, 462, 515, 526, 679, 689, 716 per current grep) — these are
     ABORT/early-exit paths where no write was published, so releasing
     locks immediately there is CORRECT and must NOT be changed. Only
     the two SUCCESSFUL-commit-path release calls (the ones that run
     after a real write was staged and WAL-durable) need to move.
     Confirm this distinction explicitly in your report.
   - Watch for a potential regression: moving lock release later
     extends the critical section during which OTHER transactions
     wait/wound on this key. This is the CORRECT behavior per 2PL (the
     lock genuinely needs to be held that long) but confirm no
     deadlock/wound-wait invariant elsewhere assumes locks are released
     "early" (e.g. immediately after WAL-durability) — grep for any
     wound-wait or lock-timeout logic that might have been tuned
     assuming the old (buggy) release timing, and flag it in your
     report if found; do not silently "fix" such logic without noting
     it.

2. **Make Pessimistic-isolation reads under a held lock resolve to the
   LATEST COMMITTED value, not the transaction's snapshot.** In
   `read_one_tx` (and the analogous `read_one_tx_bytes`), when
   `tx.isolation == IsolationLevel::Pessimistic` AND a lock was
   genuinely acquired for this read (i.e. we're past
   `acquire_pessimistic_read_lock`/the exclusive-lock acquisition),
   read via `mvcc.get_current_bytes(key)` (or the equivalent "latest
   committed" accessor — check `mvcc_store`'s public API for the
   correct method name; the audit's fix-sketch names
   `get_current_bytes`) instead of `get_at(key, tx.snapshot_version)`.
   For Snapshot/Serializable isolation, the EXISTING snapshot-gated
   `get_at` behavior is correct and must NOT change — this fix is
   scoped specifically to the Pessimistic isolation branch. Check
   whether `read_one_tx_bytes` needs the identical treatment (the audit
   names both `read_one_tx` and the write-lock acquisition path in
   `table_manager_streaming.rs:379`/`:441` — confirm current line
   numbers and whether both point-read functions need this branch).

Do NOT change Snapshot or Serializable isolation's read semantics — the
audit finding is Pessimistic-specific.

## TDD requirement

1. **Red**: write `#[tokio::test]`s reproducing BOTH parts of the bug:
   - **Lock-ordering test**: T1 (Pessimistic) stages a write and
     commits; instrument or time the commit so a concurrently-waiting
     T2 (Pessimistic, blocked on the same key's lock) can be observed
     acquiring the lock BEFORE T1's write is visible via a "latest
     committed" read — i.e. assert that by the time T2 acquires the
     lock, T1's write IS already visible (this should FAIL before the
     fix, since the lock is released before publish, and PASS after).
   - **Stale-snapshot-under-lock test**: reproduces the exact lost-
     update interleaving from the audit — T1 commits `k=v1`; a second,
     EARLIER-started Pessimistic tx T2 (snapshot predates T1's start)
     acquires the lock on `k` (after T1 released it) and reads via
     `read_one_tx`; assert T2 sees `v1` (T1's committed write), NOT the
     stale `v0` from T2's original snapshot. This should FAIL before
     the fix (T2 sees stale v0) and PASS after (T2 sees v1).
   - A combined end-to-end test: run the full lost-update scenario
     (T1 commits v1, T2 reads-under-lock and writes v2-based-on-stale-
     v0, both commit) and assert the FINAL committed value correctly
     reflects that T2's write was based on T1's committed value (i.e.
     no lost update) — or, if a stricter outcome is more appropriate
     (T2's read now correctly returns v1, so T2's own subsequent write
     logic naturally avoids the lost update), assert whichever outcome
     the FIXED code actually produces and justify it in your report.
2. **Green**: apply both fixes.
3. Confirm existing Pessimistic-isolation tests (lock acquisition,
   wound-wait, deadlock-avoidance) still pass — the release-timing
   change extends critical sections, which could interact with
   wound-wait/timeout tests; re-run the full Pessimistic-isolation test
   suite and report any behavior changes even if they still pass (e.g.
   a test that now takes measurably longer due to more contention).

## Test scope command

```
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-tx
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Which `release_pessimistic_locks` call sites were moved (async path,
  lockfree path) vs. left untouched (abort/early-exit paths), with a
  one-line justification for each left-untouched site confirming it's
  a genuine abort path with no published write.
- Whether any wound-wait/lock-timeout logic was found to implicitly
  assume the old (early) release timing, and if so what you did about
  it (fixed vs. flagged as a follow-up).
- The exact read-path change for Pessimistic isolation (which function
  now calls `get_current_bytes` or equivalent, and confirmation
  Snapshot/Serializable reads are unchanged).
- The failing-then-passing test evidence for both the lock-ordering
  test and the stale-snapshot-under-lock test.
- Confirmation existing Pessimistic/wound-wait/deadlock tests still
  pass, noting any timing/behavior changes observed.
- Full test/gate results (exact commands + pass/fail).
