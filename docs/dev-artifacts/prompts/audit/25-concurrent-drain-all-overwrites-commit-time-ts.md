Task: LOW-MEDIUM concurrency — two independent code paths that can both
drain the SAME committed version (the background drainer loop and an
explicit/forced `flush_buffers` call) each try to consume the
commit-time timestamp stamped in `pending_ts`; whichever runs SECOND
finds the entry already removed (by the first) and silently falls back
to `now_millis()` — writing a WRONG (drain-time, not commit-time)
timestamp into durable history for that version (audit finding A14,
`docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-tx/src/mvcc_store/mvcc_history.rs`:
  - `write_committed_batch_to_history` (~line 284-355, the batch/drain
    path): line ~304-308:
    ```rust
    let ts_ms = self
        .pending_ts
        .remove(&v)
        .map(|(_, ms)| ms)
        .unwrap_or_else(|| self.now_millis());
    ```
  - `write_committed_to_history` (~line 469-494+, the single-entry
    drain/recovery path): line ~483-487, the IDENTICAL pattern:
    ```rust
    let ts_ms = self
        .pending_ts
        .remove(&commit_version)
        .map(|(_, ms)| ms)
        .unwrap_or_else(|| self.now_millis());
    ```
  - Both functions independently call `self.pending_ts.remove(&v)` —
    an `scc::HashMap` (or similar) `remove` that returns `Some((v,
    ts_ms))` on the FIRST caller to reach it for a given version and
    `None` on any SUBSEQUENT caller for the same version (the entry is
    gone after the first successful remove).
  - Callers per the audit: the background drain cycle (`drainer.rs:647`,
    confirm current line) and `flush_buffers`
    (`repo_instance.rs:1146`, confirm current line) — these are TWO
    INDEPENDENT triggers that can, under the right timing, both
    attempt to drain/write-to-history the SAME `commit_version` (e.g.
    the background drainer is mid-cycle for version `v` at the exact
    moment an explicit `flush_buffers()` call — perhaps from a test,
    an admin operation, or a graceful-shutdown path — ALSO decides to
    drain `v`). Also `drain_to_history` (used by `rename_table_stores`,
    per A13's brief context) is a THIRD potential racer against the
    background drainer for the same version, if a rename happens to
    race the drainer's normal cycle.

## Why this is LOW-MEDIUM

**Concrete interleaving from the audit:**
1. Tx commits version `v` at real commit time `T0`; the ack-path
   (`apply_committed_visible` or equivalent) stamps `pending_ts[v] =
   T0`.
2. The background drainer's normal cycle reaches `v` and calls
   `write_committed_batch_to_history` (or the single-entry
   `write_committed_to_history`) for `v` — but is DELAYED/PREEMPTED
   right at (or just before) the `pending_ts.remove(&v)` line.
3. CONCURRENTLY, an explicit `flush_buffers()` call (or
   `drain_to_history` from a rename) ALSO reaches the point of trying
   to persist `v` to history and calls the SAME
   `pending_ts.remove(&v)` FIRST — it wins the race, gets `Some(T0)`,
   correctly writes `ts_key(v) = T0` to history.
4. The background drainer's call (from step 2) now runs its own
   `pending_ts.remove(&v)` — finds `None` (already removed in step 3)
   — falls back to `self.now_millis()` (call it `T1 >> T0`, since the
   drainer resumed some arbitrary time later) — and (if it also writes
   `ts_key(v)`, e.g. via its OWN `history.transact` call in its own
   batch) **OVERWRITES** the correct `T0` timestamp with the wrong
   `T1` (drain time, not commit time) in the durable ts-index.
5. **Consequence**: any feature relying on the commit-time timestamp
   being ACCURATE (e.g. time-based retention/purge —
   `purge_below_ts` per A10's context — audit trails, "when was this
   committed" queries, time-travel-by-timestamp reads) now sees the
   WRONG timestamp for `v`. This is LOW-MEDIUM severity because it does
   NOT lose data or violate MVCC visibility — it corrupts a metadata
   field (the commit timestamp) that some auxiliary features depend
   on for correctness/accuracy, but the core read/write/isolation
   guarantees are untouched.

## Fix

Per the audit's fix sketch — the core problem is that `pending_ts` is
being used as a **destructive, single-consumer** cache (whoever gets
there first "wins" and the entry vanishes for everyone else), but the
codebase actually has (at least) two legitimate, independent code paths
that may all need to observe the SAME version's commit timestamp. Pick
one of these approaches (or a justified hybrid):

**Option A — non-destructive read, garbage-collect separately:**
1. Change the consumption pattern from `remove` (destructive,
   single-winner) to a non-destructive `get`/`peek` that reads the
   timestamp WITHOUT removing it, so every racing caller sees the SAME
   correct `T0` value.
2. Since `pending_ts` entries need to be cleaned up EVENTUALLY (they're
   a bounded, transient cache — check for any existing capacity/GC
   logic on this map, e.g. does it grow unboundedly otherwise?), add an
   explicit removal step that runs ONCE, idempotently, decoupled from
   which caller "consumed" the value — e.g., remove the entry only
   AFTER the corresponding version's data is confirmed durable in
   history (which is roughly what's already happening, just tangled
   together with the timestamp read). Consider: could the removal
   safely happen unconditionally at the END of whichever call
   ultimately writes history for that version, using a "remove if
   present, ignore if already gone" idempotent pattern (`remove_if` or
   equivalent) AFTER using `get`/`peek` to read the value non-destructively
   first? This decouples "read the ts" (idempotent, safe for N racers)
   from "clean up the cache" (also idempotent, safe to attempt from
   multiple racers since only one needs to actually succeed).
3. Reason through: is `now_millis()` fallback still needed/correct in
   this design? It should now only trigger for the GENUINELY cold-start
   case (recovery, where no ack-path ever stamped `pending_ts` for this
   version at all) — confirm the non-destructive read preserves this
   distinction correctly (i.e., `None` still correctly means "never
   stamped", not "already consumed by a racing caller").

**Option B — serialize the two racing call sites against each other
for a given version (simpler, if the concurrency here is genuinely rare
and a bit of serialization is acceptable):**
1. If `flush_buffers`/`drain_to_history`/the background drainer already
   have (or could cheaply acquire) some per-version or per-table
   coordination mechanism that prevents them from BOTH attempting to
   drain/persist the exact same version concurrently in the first
   place, closing the race at the CALLER level (never let two
   `write_committed_*_to_history` calls for the same version run
   concurrently) may be simpler than reworking `pending_ts`'s
   consumption semantics. Evaluate whether such coordination already
   exists (e.g. some kind of per-version "in-flight" marker) or would
   need to be added, and whether that's more or less invasive than
   Option A.

Prefer **Option A** if it's a clean, surgical change to `pending_ts`'s
consumption pattern (likely the audit's intended fix, given the "T1c"/
"L2" comments already scattered through this code acknowledging the
timestamp-preservation intent) — but use your judgment given the
actual code shape and report which you chose and why.

## TDD requirement

1. **Red**: write `#[tokio::test]`s (check
   `crates/shamir-tx/src/tests/mvcc_store_tests/` for existing
   history/ts-related test modules, e.g. `mvcc_history_tests.rs`, and
   follow established patterns) that:
   - Reproduce the race deterministically (no reliance on real
     scheduler timing): stamp `pending_ts[v] = T0` (however the
     ack-path does this — find the stamping call, likely near line 412
     per the earlier grep, `self.pending_ts.insert(commit_version,
     self.now_millis())`), then call BOTH
     `write_committed_batch_to_history`/`write_committed_to_history`
     (or whichever pair is the actual racing pair) for the SAME
     version in sequence (simulating the race deterministically by
     just calling both, one after the other, rather than needing real
     concurrency) and assert that BOTH calls' resulting durable
     `ts_key(v)` reads (or whatever the final state is) reflect the
     CORRECT `T0`, not a `now_millis()`-time fallback from the second
     caller. This should FAIL before the fix (second caller's
     `now_millis()` fallback overwrites `T0`) and PASS after (both
     calls observe/preserve `T0`).
   - A regression test confirming the cold-recovery fallback path still
     works correctly: a version whose `pending_ts` was NEVER stamped
     (genuine cold start, no ack-path stamp) still correctly falls back
     to `now_millis()` — this must NOT regress into "always require a
     stamp or panic."
   - A regression test confirming the SINGLE-caller (non-racing) case
     is unaffected — a version processed by exactly one drain path
     still correctly consumes/reads its `pending_ts` value and (if
     Option A's GC step is added) the entry is eventually cleaned up
     (doesn't leak forever).
2. **Green**: apply the fix.
3. Confirm existing history/ts-index/drain tests still pass.

## Test scope command

```
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine -- drain
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
- Which approach was applied (non-destructive read + separate GC, or
  caller-level serialization, or a hybrid) and why.
- If `pending_ts`'s consumption pattern changed, confirm the
  cold-recovery `None` → `now_millis()` fallback distinction still
  works correctly (a genuinely-never-stamped version still falls back;
  a stamped-but-already-read-by-another-racer version does NOT
  incorrectly fall back).
- Whether `pending_ts` has any existing capacity bound / GC and
  whether the fix could introduce a new leak risk (entries that are
  read but never cleaned up) — and what was done about it if so.
- The failing-then-passing test evidence for the core race
  reproduction.
- Confirmation existing history/ts-index/drain tests still pass.
- Full test/gate results (exact commands + pass/fail).
