# Deadlock hazard — `active_snapshots` and `locks` mix scc `_async`/`_sync` lock acquisition (same class as the fixed #589 bug)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## Background — the proven #589 mechanism (read `git log --oneline --grep 589` /
## commit `7a4abf62` first for the reference fix)

`scc::HashMap`'s async lock-wait path (`entry_async(...).await`, `read_async`,
`iter_async`, etc.) is a **lock-HANDOFF**: on release, the exclusive bucket
lock is granted DIRECTLY to a suspended waiter TASK, which then holds the
lock while merely sitting in tokio's run queue until it is next polled.
Synchronous accessors on the SAME map (`entry_sync`, `get_sync`, `read_sync`,
`iter_sync`, `is_empty()`, ...) instead **park the calling OS thread** while
the bucket is locked. If, in the window between "lock granted to a suspended
task" and "that task is actually polled again", every runtime worker thread
happens to park in a `_sync` accessor on the SAME bucket, the lock-owning
task can never be polled again — **whole-runtime deadlock**. This was proven
(via an isolated out-of-repo repro crate) and fixed for `MvccStore`'s `cells`
map in commit `7a4abf62`: `publish_cell`/`seed_version`/`prune_version_cache`
switched from `entry_async`/`retain_async` to `entry_sync`/`retain_sync` to
match an already-established sync convention on the same map. **The fix
convention is: every lock-acquiring op on a map that has ANY synchronous
accessor must ITSELF be synchronous — never mix `_async` and `_sync` lock
acquisition on the same scc map.**

A follow-up read-only research sweep found two more sites with the exact
same structural hazard, both ranked HIGH confidence because — like the
`cells` map — their contention is NOT spread across many hash buckets, it
funnels onto ONE naturally-hot key/bucket by construction (making the
deadlock window much more reachable than a uniformly-distributed-key map
would be). This task fixes both.

## Site 1 — `RepoTxGate::active_snapshots` (`crates/shamir-tx/src/repo_tx_gate.rs`)

- `bump_refcount` — `self.active_snapshots.entry_async(version).await` at
  **line 415** (called from `register_snapshot` → `open_snapshot` /
  `open_snapshot_serializable` — i.e. **every tx/snapshot open**).
- `SnapshotGuard::drop` — `self.snapshots.entry_sync(v)` at **line 216**
  (**every tx end**, running inline on whatever runtime worker drops the
  guard).
- `min_alive` — `self.active_snapshots.iter_sync(...)` at **line 557** (GC /
  vacuum / prune ticks).
- `active_snapshots_empty` — `self.active_snapshots.is_empty()` at **line 580**
  (vacuum fast paths — scc's `is_empty` also takes bucket read locks
  synchronously).

**Why this is sharper than a generic mixed-map hazard**: the map is keyed by
**version**, and every concurrent `open_snapshot` targets the *same* key —
the CURRENT `last_committed` version. So exactly like `cells`'s hot key, all
contention funnels into ONE bucket by construction, not by unlucky hashing.
Concrete interleaving:

1. Tx A calls `open_snapshot` → `entry_async(v_cur)` finds the bucket locked
   (some other opener/dropper holds it) and suspends.
2. The holder releases; saa **hands the exclusive bucket lock to A's
   suspended task**. A now owns the lock while sitting in tokio's run queue,
   unpolled.
3. Before A is polled, N committed txs finish on the N worker threads; each
   `SnapshotGuard::drop` runs `entry_sync(v_cur)` → **parks its OS worker
   thread** on the same bucket (guard drops run inline in whatever async fn
   drops the tx — i.e. on runtime workers).
4. All workers parked → A is never polled → nobody ever releases → whole-
   runtime deadlock. A GC tick's `min_alive` `iter_sync` reaching that same
   bucket parks additional threads the same way.

Load profile that triggers it: high tx open/close rate (every commit opens
and drops a snapshot) — no exotic conditions needed. On a small runtime
(1-2 workers, e.g. tests or embedded use) a single racing drop suffices.

### Fix

Change `bump_refcount` (line 415) to use `self.active_snapshots.entry_sync(version)`
instead of `entry_async(...).await` — the closure inside is a saturating
refcount increment (a few instructions, no `.await`), so the function CAN
stay `async fn` with an unchanged signature; only the specific `.entry_async`
call becomes `.entry_sync`. The existing cancel-safety note at line 303
("the `entry_async` calls are CAS-based") already documents the entry as
effectively atomic — nothing is lost by switching to the sync accessor,
since the critical section was already meant to be instantaneous.

## Site 2 — `MvccStore::locks` (`crates/shamir-tx/src/mvcc_store/mvcc_locks.rs`)

- `lock_key` — `self.locks.entry_async(key).await` at **line 54** (every
  Level-3 pessimistic lock acquisition).
- `release_locks` — `self.locks.get_sync(key)` at **line 206** (every
  Level-3 commit AND abort, via `release_pessimistic_locks`,
  `crates/shamir-engine/src/tx/commit.rs:825-826`).

The `locks` map is per-table, shared across all txs. The hot-key case is
precisely the interesting one for pessimistic locking: many txs contending
for the SAME record key → same bucket. Interleaving mirrors Site 1 exactly:
Tx A's `lock_key(k)` suspends in `entry_async(k)`; a completing op on that
bucket hands A the exclusive bucket lock while A sits unpolled; concurrent
txs B..N finish (commit or abort) on worker threads and call `release_locks`
→ `get_sync(k)` → each **parks its worker** (scc's `get_sync` takes a shared
bucket lock and parks while the bucket is exclusively owned); if all workers
park in `get_sync`, A never gets polled → deadlock. Extra nastiness: the
parked releasers are exactly the txs whose release would have let waiters
make progress — even a *near*-deadlock here inflates wound-wait latencies
badly.

### Fix

Change `lock_key` (line 54) to use `self.locks.entry_sync(key)` instead of
`entry_async(...).await` — the entry body is `Arc::clone`-or-insert
(bounded, no suspension), so this is the same shape of fix as Site 1.
`get_sync`→async at line 206 is the WRONG direction (per the project
convention noted in the research report — sync accessors already exist on
this map, and `release_locks` runs from a `Drop`-adjacent commit/abort path
that cannot `.await`), so the fix is entirely on the `entry_async` side.

While you are in this file, also verify `mod.rs:1276`'s `self.locks.len()`
already carries the required `#[allow(clippy::disallowed_methods)] // O(N)
ack: ...` comment per this workspace's `clippy.toml` `disallowed-methods`
rule (it should — confirm it, don't touch it if already present; this is
just a verification note, not expected to require a change).

## The fix — precedent to mirror

Both fixes are IDENTICAL in shape to the already-landed `#589` fix
(`crates/shamir-tx/src/mvcc_store/mod.rs`'s `publish_cell`/`seed_version`/
`prune_version_cache`, commit `7a4abf62`) — read that diff first (`git show
7a4abf62`) to see the established pattern (doc comment style, "DEADLOCK FIX"
labeling) and mirror it exactly, including a similar doc-comment explaining
the mechanism at each fixed call site.

## Tests

Read the existing `#589` regression test(s) for the `cells` map fix first
(search `crates/shamir-tx/src/tests/` for anything referencing `entry_sync`,
`deadlock`, or `#589`/`publish_cell` in a test context) to understand this
codebase's established style for proving a liveness fix — mirror that style
here rather than inventing a new one.

1. **Site 1 regression**: a test exercising concurrent snapshot open/close
   (`open_snapshot`/`SnapshotGuard::drop`) racing on the SAME version under
   load (ideally on a constrained `worker_threads` count, e.g. 1-2, where a
   single racing drop is enough to expose the pre-fix hazard) — must
   complete without hanging. Wrap the exercising loop in a bounded
   `tokio::time::timeout` with a NAMED assertion message (mirror
   `crates/shamir-index/src/vector/tests/quantized_graph_tests.rs:1630`'s
   pattern — "this test's own guard against a real regression hanging the
   whole suite," not a workaround for flakiness) so a future regression
   fails loudly and specifically rather than hanging the entire nextest run
   for 180s with an anonymous TIMEOUT.
2. **Site 2 regression**: similarly, a concurrent Level-3 pessimistic
   lock-acquire/release stress test on a SHARED hot key (multiple txs
   contending for the same `RecordKey`), under constrained worker threads,
   wrapped in the same bounded-timeout pattern.
3. It is OK if these tests do not deterministically reproduce the exact
   pre-fix hang on every run (the hazard is a race window) — the goal is
   BOTH (a) exercising the interleaving so nextest's own parallelism has a
   chance to catch a regression over time, and (b) a bounded timeout so a
   real regression fails fast and identifiably instead of hanging silently.
   Document this reasoning in the test module's doc comment, matching how
   the codebase already reasons about `overlay_ordering_tests.rs`'s own
   known-hard-to-deterministically-reproduce nature.
4. Existing tests exercising `open_snapshot`/pessimistic locking must
   continue to pass unchanged.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @tx @oracle --full` green, including all new tests
  (check `scripts/test.sh`'s `scope_args` for the exact scope names if
  `@tx`/`@oracle` don't match — use whichever scope covers `shamir-tx` and
  the Version-Oracle-area engine tests; report which you used).
- `cargo fmt --all -- --check` clean (or scoped to `shamir-tx`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) both fixes are structurally identical to the
  `#589` precedent (same class of change, same reasoning), (b) neither
  fix changes any OTHER observable behavior of `open_snapshot`/
  `SnapshotGuard::drop`/`lock_key`/`release_locks` beyond the sync/async
  lock-acquisition mechanism itself.

## Out of scope

- Do NOT touch the vector-index maps (`hnsw_adapter.rs`/`vector_backend.rs`),
  `per_table_mvcc`/`token_names` (`repo_instance.rs`), or `layered_interner.rs`
  — those are separate, already-tracked follow-up tasks (H3, H4/H5, H6 from
  the same research report), each with their own brief.
- Do NOT touch anything in `MvccStore`'s `cells` map — that is the ALREADY-
  FIXED `#589` bug (commit `7a4abf62`); this task is about the TWO NEW sites
  (`active_snapshots`, `locks`), not a re-verification of the old fix (though
  reading it as a precedent is expected and required).
- Do NOT raise any test timeout to paper over a hang — if you observe a real
  hang during testing, root-cause it (per this session's standing "hunt and
  fix hangs, never tolerate" discipline) rather than loosening a timeout.
