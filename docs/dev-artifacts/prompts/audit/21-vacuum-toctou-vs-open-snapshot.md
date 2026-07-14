Task: MEDIUM concurrency — a TOCTOU race between `open_snapshot` (reads
the current floor, THEN registers it) and the vacuum fast-path (checks
`active_snapshots_empty()`) can let vacuum delete a version that a
just-opening reader still needs, making a valid snapshot read return
empty for a key that should be visible (audit finding A10,
`docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-tx/src/repo_tx_gate.rs`, `open_snapshot` (~line
  223-231) and `open_snapshot_serializable` (~line 242-258): both do
  **two separate, non-atomic steps**:
  1. `let version = self.last_committed();` — read the current floor.
  2. `self.active_snapshots.insert_async(version, ()).await;` —
     register that version so vacuum knows a live reader is pinned to
     it.
  There is a genuine gap between step 1 and step 2 (an `.await` point
  even, in the `insert_async` call) during which the reader is NOT YET
  registered in `active_snapshots`, even though it has already decided
  which version (`V_old`) it will read at.
- `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs`, `vacuum_key`'s L6 fast
  path (~line 50-85, specifically the `snapshots_empty` check at line
  66-70): `let snapshots_empty = self.gate.active_snapshots_empty();`
  — if this returns `true` (no snapshot version is registered YET,
  because the racing reader from above hasn't called `insert_async`
  yet), the fast path proceeds to `self.history.remove(...)` the OLD
  version and its ts-key unconditionally (line 75-76), plus removes it
  from the overlay if durable (line 81-83).
- Same TOCTOU class affects the scan-path vacuum's `min_alive()`/
  `have_live_snapshot` reads (~line 101-102) and any other call site
  that reads `gate.min_alive()`/`active_snapshots_empty()` as an
  "is anyone reading an old version" oracle without accounting for a
  reader that has decided its version but not yet registered it —
  confirm during implementation whether `prune_commit_log_below` or
  other GC-adjacent functions share this same read-then-decide pattern
  against `active_snapshots`/`min_alive` and need the identical fix.

## Why this is MEDIUM

**Concrete interleaving from the audit (default retention =
CurrentOnly, the common case where this fast path fires normally):**
1. Reader: calls `open_snapshot()`. Step 1 runs:
   `version = last_committed()` → captures `V_old`. **Not yet
   registered** — the reader is about to call `insert_async` but has
   been PREEMPTED (descheduled) right before/during that await.
2. Writer: publishes a new version `new_v` for some key `k` (where
   `k`'s previous version was `V_old`, i.e. `old_v = V_old` in
   `vacuum_key`'s terms). Calls `vacuum_key(k, V_old)`.
3. `vacuum_key`'s L6 fast path: `is_current_only()` is true (default
   retention), `snapshots_empty = self.gate.active_snapshots_empty()`
   → **`true`** (the reader from step 1 has NOT registered yet) →
   `!vacuum_needs_scan` is also true (no prior snapshot epoch touched
   this path) → **fast path fires**: `history.remove(k, V_old)` and
   the overlay entry for `(k, V_old)` are deleted.
4. Reader: resumes, completes `insert_async(V_old, ())` — now
   registered in `active_snapshots`, believing it holds a valid
   snapshot pin at `V_old`.
5. Reader: reads key `k` at its snapshot version `V_old`. The cell's
   CURRENT version is `new_v > V_old` (published in step 2), so the
   read path falls back to "newest version `≤ V_old`" — but that
   version (`V_old`'s history entry) was **just deleted** in step 3.
   **The read returns empty/missing** for a key that should be
   visible at a valid, registered snapshot — a correctness violation
   of snapshot isolation (a registered reader's floor is supposed to
   be a "sacred" anchor that vacuum never crosses, per the doc comment
   at `mvcc_gc.rs:34-39`, but this TOCTOU lets it happen exactly once
   per race).

The same class of race applies to the scan-path vacuum (`min_alive()`)
and to `prune_commit_log_below` per the audit — anywhere "is there a
live snapshot at or below version X" is answered by reading
`active_snapshots`/`min_alive` at a moment that predates a
concurrently-opening reader's registration for that same X.

## Fix

Per the audit's fix sketch — two acceptable approaches, pick the one
that fits the code more cleanly (or a hybrid) and justify your choice:

**Approach 1 — register-then-verify-then-reconcile (closes the race at
the reader side):**
1. In `open_snapshot`/`open_snapshot_serializable`: register FIRST at
   a placeholder/candidate version, THEN re-read the floor, and if it
   moved, re-register at the NEW floor and remove the stale
   registration — i.e., "insert-new-then-remove-old" as the audit's
   sketch names it. Concretely:
   ```
   let v0 = self.last_committed();
   self.active_snapshots.insert_async(v0, ()).await;
   let v1 = self.last_committed();
   if v1 != v0 {
       // floor moved while we were registering v0 — register at the
       // NEW floor too before removing the old one, so there is never
       // a window with zero registered anchor for whichever version
       // vacuum might race against.
       self.active_snapshots.insert_async(v1, ()).await;
       self.active_snapshots.remove(&v0).await; // or remove_async, check API
   }
   // snapshot version actually used for reads must be the FINAL v1 if it moved
   ```
   Adjust exact ordering/API to what `active_snapshots` (an
   `scc::HashMap` or similar per repo conventions — confirm exact type)
   actually exposes; the key invariant is: **at every instant, the
   version this reader is ABOUT TO read (or already reading) at is
   registered in `active_snapshots` BEFORE vacuum can observe
   `active_snapshots_empty() == true` for a window that overlaps this
   reader's decision point.** A simpler equivalent: register at `v0`
   BEFORE reading `last_committed()` a second time is not quite right
   either — think through the exact ordering so there is no window
   where the reader has committed to a version but that version (or a
   version ≤ it, per the "anchor = largest version < min_alive" logic)
   is unprotected.
2. Given the loop-until-stable nature of "read floor → register →
   re-check floor moved → re-register", make sure this genuinely
   terminates (floor only moves forward under normal operation, so at
   most one or two iterations in practice — but write it as a bounded
   retry, not an unbounded loop, and decide what to do if it doesn't
   stabilize quickly, e.g. after N retries just keep the latest
   registration since a moving floor only ever needs the LATEST
   version protected for correctness — a snapshot always reads at
   `last_committed` at the time it stabilizes, not literally the very
   first version glimpsed).

**Approach 2 — fast-path vacuum always keeps one anchor (closes the
race at the vacuum side, simpler, matches the scan-path's existing
anchor logic):**
1. In `vacuum_key`'s L6 fast path, do NOT unconditionally delete the
   old version even when `active_snapshots_empty()` is currently true.
   Instead, ALWAYS retain the single most-recent previous version as
   an "anchor" (mirroring the scan-path's existing "anchor = largest
   version < min_alive" logic at ~line 133-140), and only actually
   physically delete it on a LATER vacuum call once a subsequent write
   proves the anchor is no longer the most-recent-previous (i.e.,
   defer deletion by one generation). This trades a small amount of
   extra retention (one version behind current, always) for closing
   the TOCTOU entirely — since a reader that "just missed" registering
   before this check would still find its needed version present.
2. This is likely the SIMPLER, more surgical fix given the existing
   scan-path already does exactly this anchor-retention pattern —
   consider whether L6's fast path can be changed to defer the removal
   of the immediately-prior version by one write (keep it until the
   NEXT vacuum_key call proves a newer old_v exists), rather than
   deleting it in the SAME call that created it as "old".

Pick ONE approach (or a justified hybrid) and implement it
consistently across `vacuum_key`'s fast path AND the scan-path's
`min_alive()`-based reads AND `prune_commit_log_below` if it shares the
same hazard — do not fix only one call site if the others share the
identical race shape.

## TDD requirement

1. **Red**: write `#[tokio::test]`s (check
   `crates/shamir-tx/src/mvcc_store/tests/` for existing vacuum/gc test
   modules and follow established patterns) that:
   - Deterministically reproduce the race WITHOUT relying on real
     scheduler timing: directly call the lower-level pieces in the
     "wrong" order — e.g. capture `last_committed()` as `v0` WITHOUT
     yet calling `insert_async`, then perform a write + `vacuum_key`
     call (which will see `active_snapshots_empty() == true` and fire
     the fast path), THEN register `v0` via `insert_async` (simulating
     the reader "catching up"), then attempt to read at `v0` and assert
     it is **MISSING** — this documents the pre-fix bug precisely (like
     the A9 brief's "negative proof" pattern: a test that demonstrates
     the OLD ordering is broken, to prove the race is real).
   - A second test using the FIXED ordering/logic (register/anchor
     mechanism) that performs the equivalent interleaving and asserts
     the read at `v0` correctly returns the expected value — should
     FAIL against the pre-fix code path and PASS post-fix.
   - A regression test confirming the COMMON case (no racing reader,
     retention/vacuum behaves as before) still works — a write followed
     immediately by `vacuum_key` with `active_snapshots_empty() ==
     true` genuinely reachable/registered should still reclaim old
     versions promptly (don't accidentally make retention permanently
     conservative/leaky).
2. **Green**: apply the fix.
3. Confirm existing GC/vacuum/retention tests still pass — this touches
   a hot, frequently-exercised path (every commit calls `vacuum_key`),
   so run the FULL `shamir-tx` suite, not just GC-focused tests.

## Test scope command

```
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine -- vacuum
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
- Which approach (register-then-verify-then-reconcile at the reader
  side, or always-keep-one-anchor at the vacuum side, or a hybrid) was
  applied, and why.
- Which call sites were touched: `open_snapshot`/
  `open_snapshot_serializable`, `vacuum_key`'s L6 fast path, the
  scan-path's `min_alive()` usage, `prune_commit_log_below` — confirm
  for each whether it shared the hazard and what was done.
- The failing-then-passing test evidence for the core TOCTOU
  reproduction.
- Confirmation the common (no-race) retention/vacuum behavior is
  unchanged (old versions still reclaimed promptly when no snapshot is
  truly active).
- Full test/gate results (exact commands + pass/fail).
