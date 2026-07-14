Task: HIGH-concurrency — `publish_cell`/`seed_version` regress a
record cell's version backward when a slow drainer/recovery path races
a newer commit, causing stale reads and masked SSI conflicts
(audit finding A2, `docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-tx/src/mvcc_store/mod.rs:404-414` — `publish_cell`:
  ```rust
  pub(super) async fn publish_cell(&self, key: Bytes, version: u64) {
      match self.cells.entry_async(key).await {
          scc::hash_map::Entry::Occupied(mut e) => {
              e.get_mut().version = version;   // <-- unconditional overwrite
          }
          scc::hash_map::Entry::Vacant(e) => {
              e.insert_entry(RecordCell { version, reserved_by: 0 });
          }
      }
  }
  ```
- `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:244-253` —
  `seed_version` (a SEPARATE code path, does its own `cells.upsert_async(...)`
  rather than calling `publish_cell`, but has the exact same bug):
  ```rust
  /// `upsert_async` (not `insert`) so a re-replay of the same key
  /// advances monotonically rather than silently keeping a stale value.
  pub async fn seed_version(&self, key: Bytes, version: u64) {
      self.cells
          .upsert_async(key, super::RecordCell { version, reserved_by: 0 })
          .await;
  }
  ```
  **This doc comment is factually wrong** — `upsert_async` REPLACES the
  existing value unconditionally; it does not compare-and-advance. It
  does NOT "advance monotonically", it just always overwrites,
  including backward. Fix the comment along with the code.
- Callers that can race a newer commit: `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:326-334`
  (batched drain path calling `publish_cell` per op) and `:508-519`
  (single-entry drain path, same pattern) — both are labeled "seed the
  cell from the durable write (idempotent; needed on cold recovery)"
  but are NOT actually idempotent/safe when a newer in-memory commit
  raced ahead of the drain/recovery replay.

## Why this is HIGH

Interleaving from the audit (verify this is still accurate against
current HEAD before writing the fix — line numbers may have drifted):

1. Transaction A commits key `k` at version 10 (`cell.version = 10`,
   overlay has `(k, 10)`); this write is offered to the drainer;
   durable-watermark is still 9.
2. Drainer picks up A's write: `Phase B: history.transact(...)` for
   v10 — this is an `.await` point, so the drainer task can be
   suspended here for an arbitrary amount of time (I/O scheduling,
   backpressure, etc.).
3. **While the drainer is suspended**, transaction B commits the SAME
   key `k` at version 11: Phase 5a → `finalize_reservation(k, 11)` →
   `cell.version = 11`.
4. The drainer resumes and calls `publish_cell(k, 10)` (finishing its
   now-STALE seed-from-durable-write for v10) — this **overwrites
   `cell.version` from 11 back down to 10**.
5. A reader calling `get_current(k)` now sees `cell.version = 10`,
   `cur_v = 10 ≤ floor(=11)` → takes the "already durable, read
   directly" fast path → reads the overlay value for v10 (transaction
   A's value) — **transaction B's committed write (v11) is invisible**
   until the drainer's NEXT pass eventually re-corrects the cell
   (usually milliseconds later, but unbounded if the drainer is stuck
   or backpressured).
6. Worse: an SSI validator running with `version_seen = 10` during this
   window observes `current_version = 10` (matching what it read) —
   the write-write conflict that SHOULD have been detected is
   **silently masked**, compounding other SSI-related findings in this
   audit (A1, A3) if/when those are fixed later.

The same regression applies to `seed_version`'s cold-read path: a
cold-read racing the FIRST overlay-only commit of a key can seed an
OLDER history-derived version on top of a fresher in-memory cell,
producing the identical stale-read symptom.

## Fix

Make BOTH `publish_cell` and `seed_version` **max-monotonic**: only
advance `cell.version` if the new version is STRICTLY GREATER than the
cell's current version. Never regress it.

For `publish_cell` (already uses `entry_async` — trivial gate):
```rust
pub(super) async fn publish_cell(&self, key: Bytes, version: u64) {
    match self.cells.entry_async(key).await {
        scc::hash_map::Entry::Occupied(mut e) => {
            if version > e.get().version {
                e.get_mut().version = version;
            }
        }
        scc::hash_map::Entry::Vacant(e) => {
            e.insert_entry(RecordCell { version, reserved_by: 0 });
        }
    }
}
```

For `seed_version` — it currently uses `upsert_async` which has no
"read old value, compare, conditionally write" hook the way
`entry_async`'s occupied/vacant match does. Rewrite it to use
`entry_async` the same way `publish_cell` does (or, if there's a
reason `seed_version` needs to stay on a different scc API — check
whether it's called from a non-async context or has some other
constraint that `publish_cell` doesn't — state that reasoning in your
report; otherwise prefer converging the two functions onto the same
entry_async-based max-monotonic pattern for consistency, or even
consider whether `seed_version` should just delegate to
`publish_cell` directly since their target behavior is now identical
— check if there's any OTHER meaningful difference between them
besides the seed context, e.g. does `seed_version` need to skip
touching `reserved_by`, or handle the vacant-insert differently for
recovery-specific reasons? If truly identical in effect after this
fix, collapsing `seed_version` into a thin wrapper over `publish_cell`
would remove code duplication — but only do this if you're confident
it doesn't change any other behavior; a safer minimal fix is
duplicating the max-monotonic entry_async logic in both functions
without merging them, if you're unsure).

**Also fix the misleading doc comment on `seed_version`** (currently
claims `upsert_async` "advances monotonically" — false; state
accurately what the FIXED code now does).

## TDD requirement

1. **Red**: write a `#[tokio::test(flavor = "multi_thread")]` in
   `crates/shamir-tx/src/mvcc_store/tests/` (check the existing test
   module layout for `mvcc_store`/`mvcc_history` first) that
   reproduces the interleaving above directly against `publish_cell`
   and/or `seed_version` (no need to go through the full drainer/tx
   machinery — a focused unit test calling these functions in the
   "wrong" order is sufficient and more reliable than trying to
   orchestrate a real drainer race):
   - Call `publish_cell(key, 11)` (simulating the newer commit).
   - Then call `publish_cell(key, 10)` (simulating the stale, delayed
     drain-seed catching up).
   - Assert `cell.version` (via whatever accessor exists —
     `version_of` or similar) is STILL `11`, not regressed to `10`.
   - Repeat the same shape for `seed_version`.
   This should FAIL against the current code (version regresses to
   10) and PASS after the fix.
2. **Green**: apply the fix.
3. Confirm existing `mvcc_store`/`mvcc_history`/drainer-related tests
   still pass — the audit explicitly notes "drainer's own use of
   publish_cell always writes ≤ the actual current version in the
   NORMAL (non-racing) case, so making it max-monotonic should not
   break any currently-passing behavior."

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
- The exact max-monotonic guard added to both `publish_cell` and
  `seed_version`, and whether you collapsed `seed_version` into
  `publish_cell` or kept them separate (with reasoning either way).
- Whether the misleading `seed_version` doc comment was corrected.
- The failing-test-then-passing evidence for both functions.
- Test/gate results (exact commands + pass/fail).
