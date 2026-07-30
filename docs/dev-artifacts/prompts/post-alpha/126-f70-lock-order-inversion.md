# F-70 (#897) — fix the commit/DDL lock-order inversion (real, reachable deadlock)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The bug — confirmed by reading the current code this session

`pre_commit_prelock` (`crates/shamir-engine/src/tx/pre_commit.rs`, the Phase
2.5 block, currently around lines 452-520) does, for a committing
transaction:

1. Loop over every table in `tx.write_set`: `enter_writer_drain()` (bump the
   `active` counter) THEN read `needs_write_barrier()`. If `true` (this
   table needs the lock), **drop the drain guard immediately** and add the
   table's token to `unique_tokens`. If `false` (fast path), **keep the
   drain guard alive** — it must stay alive until Phase 5c materialize, so
   a DDL that raises the barrier after this read genuinely waits for this
   committer via `drain_writers()`.
2. Sort + dedup `unique_tokens`, THEN loop over them acquiring
   `unique_write_lock` for each (`tbl.unique_write_lock().lock_owned().await`).

So a committer can be **holding a drain guard on table X (kept alive from
step 1) while blocking on `unique_write_lock` for table Y (step 2)** — both
within the SAME `pre_commit_prelock` call, for the SAME transaction.

Meanwhile, EVERY DDL path (`create_index_v2`, `create_index`,
`create_unique_index[_locked]`, sorted-index create —
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs` and
`table_manager_sorted_index.rs`, all wired up by F-57 #883) does the
OPPOSITE order: `let _uwl_guard = self.unique_write_lock.lock().await;`
FIRST, then `self.drain_writers().await;` SECOND.

**The reachable 3-party cycle** (review #2's finding, independently
confirmed plausible by re-deriving it this session — VERIFY THIS RIGOROUSLY
YOURSELF, don't just trust this restatement):
- DDL (`CREATE INDEX` on table X): holds `unique_write_lock(X)`, blocked in
  `drain_writers(X)` waiting for committer A's drain guard on X to clear.
- Committer A: holds its drain guard on X (fast path, kept alive per step 1
  above), blocked acquiring `unique_write_lock(Y)` for some OTHER table Y
  this same transaction also wrote to (Y already needs the lock — either a
  pre-existing legacy unique index via `tx.unique_guards`, or `Y` itself
  gained a write-barrier condition).
- Committer B: holds `unique_write_lock(Y)`, itself mid-materialize and
  (if B ALSO wrote to X, and X's barrier was raised by the DDL after B
  started) blocked trying to acquire `unique_write_lock(X)` — held by the
  DDL.

DDL → A → B → DDL. A genuine deadlock, not a race.

**F-57 made this reachable.** Before F-57 (#883) wired lock+drain into
`create_index`/`create_unique_index`/sorted-index create, only
`create_index_v2` had this pairing (from F-56), and the specific cycle
above needs at least one non-index2 DDL family to close. This is a
regression our own remediation wave introduced, not a pre-existing bug —
treat it with the seriousness that implies.

## Fix direction — a hypothesis to verify rigorously, not a mandate

This session's own analysis (re-derive and VERIFY, don't just adopt):
flipping the DDL side to **drain-then-lock** (matching the committer's own
implicit order — the committer's drain-guard-retention effectively happens
"before" any lock attempt) may be sound, because:

- `drain_writers()`'s `active` counter only ever counts writers who were
  ALREADY on the fast path (entered the drain set) before the DDL raised
  its intent flag — a writer's fast-path/slow-path fork is decided at ITS
  OWN read of the flag, independent of when the DDL happens to acquire (or
  not yet acquire) the lock. So draining first does not change WHO the
  drain waits for.
- A NEW writer arriving after the DDL raises its flag (during a
  drain-before-lock DDL's gap between drain-complete and lock-acquired)
  reads the flag as already `true` and takes the slow path — it competes
  for the SAME lock the DDL is about to take. Whichever of them wins the
  race, the DDL's eventual backfill/registration snapshot is taken AFTER
  the DDL itself holds the lock, so it is safe reeither way: if the new
  writer wins the lock first, it completes and releases before the DDL's
  snapshot; if the DDL wins, the new writer simply waits its turn.

**This reasoning could be wrong or incomplete** — there may be an
interaction with multiple concurrent DDLs on the same table, with the
`unique_tokens` sort-order ABBA-freedom invariant (re-read the comment
block right before the `uwl_guards` acquisition loop in `pre_commit.rs`),
or with `create_unique_index`'s specific "first unique index on this
table" transition. Prove whichever direction you choose with the same
rigor F-56/F-69 required — a written argument, not a vibe — OR find a
different fix entirely (e.g., have the committer release ALL its drain
guards before attempting ANY lock acquisition, re-entering drain only
for tables it still needs after all locks are held — but that reopens the
F-48b check-then-act gap for those tables unless you can show it doesn't;
work through it explicitly if you go this route instead).

Whichever order you land on, add an explicit comment at EVERY acquisition
site (the DDL call sites AND `pre_commit_prelock`) naming the full
lock-order hierarchy, so a future change cannot silently reintroduce the
inversion — this is exactly how F-57 introduced this bug in the first
place (the hierarchy was never written down anywhere globally, only
implied per-call-site).

## Definition of done

- A deterministic test (this codebase's existing pause-seam convention —
  `TEST_POST_BARRIER_PRE_WRITE_HOOK` or a new equivalent hook if needed,
  NO `sleep`-based timing) that constructs the 3-party cycle described
  above against the CURRENT (pre-fix) code and demonstrates it deadlocks
  (or, if a live deadlock is impractical to assert directly in a test
  without hanging the test suite itself, demonstrates the cycle via a
  timeout-bounded assertion that the current code hangs past a short
  bound while the fixed code completes promptly — use a tokio
  `timeout()` wrapper around the operations, not a bare `sleep`, so the
  test itself cannot hang the suite even if the fix regresses later).
- The SAME test passes cleanly after the fix.
- The chosen lock order is documented at every acquisition site (DDL
  paths + `pre_commit_prelock`) with a shared, explicit hierarchy
  statement — consider a single doc comment on `WriterDrainBarrier` or a
  new small module that BOTH sides link to, so there's one canonical
  source instead of N independent copies that can drift.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine -p shamir-tx --full` green, with NO
  `SLOW`/`TIMEOUT` markers on the affected suites (per this project's own
  CLAUDE.md: "a TIMEOUT is the signature of exactly this class of bug" —
  do not accept a slow-but-passing result as sufficient here, since a
  reduced-but-nonzero deadlock window would show up exactly that way).
- Re-verify F-57's original correctness argument (`git show fcaae001`)
  and F-56's (module doc on `writer_drain_barrier.rs`) still hold under
  whatever order you land on — cite them explicitly in your commit
  message, don't just assert it.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
