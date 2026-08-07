# Brief — #1037: P0-3a Slice 2 — wire `ReaderDrainGate` into `SortedIndexManager`

## Context

S.H.A.M.I.R. Database. Slice 1 (#1011, already merged) closed a race
between DROP INDEX and an in-flight reader for the **regular** hash-index
family: `IndexManager::drop_index` retires the definition from the
planner-visible RCU Vec, then sweeps postings from `info_store` with no
synchronization against a reader who resolved the definition just before
the retire — such a reader could observe a partially-swept keyspace. Slice
1 introduced `ReaderDrainGate` (`crates/shamir-index/src/
reader_drain_gate.rs`) and wired it around `IndexManager::lookup_by_index`
(the read chokepoint) and `IndexManager::drop_index` (the writer side).

The full cross-family plan is written up in `docs/dev-artifacts/research/
2026-08-06-p0-3a-reader-drain-gate-plan.md` — **read §3 ("Cross-family
rollout") and §4 ("Test plan") in full before starting**; this brief
summarizes and updates it against the current tree, it does not replace it.
Also read §1-2 for the primitive's own design and slice-1's exact call-site
pattern to mirror (§2, "Call-site plan (regular family)").

This task (#1037) is **Slice 2: the sorted family**
(`crates/shamir-index/src/base_index/sorted_index_manager.rs`,
`SortedIndexManager`) — same gap, same fix pattern, different manager.

## Already investigated — two corrections to the plan/task text, verified against the CURRENT tree

1. **`entry_count` is genuinely UNPROTECTED for the regular family too —
   the plan's own §3 mentions this as one of sorted's 8 chokepoints, and
   the task description's original text ("entry_count already closed by
   slice 1 via doctor::verify()'s begin_write_barrier") is CONFIRMED
   STALE, exactly as the task's own later annotation warned.** I verified
   this by reading the code directly:
   - `crates/shamir-engine/src/table/doctor.rs:234-245` — a doc comment
     explains a `begin_write_barrier` acquisition around `verify()`'s
     entry-count reads was tried and **REVERTED**: `verify()` is a
     read-only diagnostic that must be able to inspect a table with an
     index STUCK in `Building` (e.g. a crashed backfill holding the same
     barrier) — acquiring the barrier there self-deadlocked two
     `doctor_tests` (confirmed by genuine 180s TIMEOUTs, not a flake).
   - I grepped every `reader_gate.` call site in `index_manager.rs`
     (slice 1's own file) — only ONE exists: `.enter()` inside
     `lookup_by_index` (~line 2233) and `.begin_drop()` inside
     `drop_index` (~line 1783). **`IndexManager::entry_count` (line 2331)
     has NO gate call at all** — it's a raw `scan_prefix_stream` with zero
     synchronization against a concurrent DROP.
   - **Scope decision for THIS task**: gate **sorted's** `entry_count`
     (it's explicitly one of the plan's 8 listed sorted chokepoints — do
     it). Do **NOT** retroactively add a gate to the regular family's
     already-shipped, ungated `entry_count` — that's a separate,
     out-of-scope fix for a pre-existing gap slice 1 left open, not this
     task's job. If you disagree after your own investigation, stop and
     report back with your reasoning rather than silently expanding
     scope.

2. **Verify the line numbers in the plan doc against HEAD before relying
   on them** — the plan's own header warns it was written against commit
   `37cc59a3` and several commits have landed since (though none touching
   these files per the plan's own note); re-`grep` each named function
   rather than trusting a specific line number blindly.

## What to implement

Follow the plan's §3 exactly:

1. **Add a `reader_gate: ReaderDrainGate` field to `SortedIndexManager`**,
   constructed and cloned the same way `IndexManager` does it (mirror the
   `reader_gate` field/constructor/clone pattern at `index_manager.rs`
   lines ~315, ~458, ~341).
2. **Wire `.enter()` around all 8 chokepoint read methods**: `lookup_range`,
   `lookup_range_with_values`, `lookup_min`, `lookup_max`, `lookup_last_k`,
   `lookup_range_first_k_page`, `lookup_first_k`, `entry_count`. The 5
   `*_tx`-suffixed variants delegate into these and need no separate
   change (confirm this by reading them, don't just assume). Each gated
   method must back off with `Ok(None)` (never a silently-empty
   `Ok(Some(vec![]))`) when the gate signals a drop in progress — mirror
   `lookup_by_index`'s exact back-off contract.
3. **Wire `SortedIndexManager::drop_index`** with the same step 2.5/3.5/4.5
   insertions as `IndexManager::drop_index` (raise the intent flag via
   `begin_drop()` BEFORE the RCU retire, hold `drain_guard` until the sweep
   completes, RAII-release on early return). **Sorted's `drop_index` has
   its own extra rollback branch** (the `!existed` case) that regular's
   doesn't — find it and make sure `DropDrainGuard` is released correctly
   there too (RAII should handle this automatically if the guard is a
   local binding dropped at scope exit on every path, including early
   returns — verify this holds for the `!existed` branch specifically,
   don't assume).

## Tests

New test file `crates/shamir-engine/src/table/tests/p1037_sorted_reader_drain_tests.rs`
(sibling naming convention to slice 1's `p1011_reader_drain_tests.rs` if
that's what it was actually named — check and match). Per the plan's §4,
adapted for the sorted family:

1. **Proof test**: parked read holds the guard → spawned DROP blocks in
   `wait_for_readers` → direct `info_store` scan proves the sweep has NOT
   started while parked → release read → assert it returned the COMPLETE
   pre-drop set → DROP completes → sweep verified to have run only after.
2. **Back-off test**: a read arriving during the drop-in-progress window
   returns `Ok(None)`, never a silently-empty `Ok(Some([]))`.
3. **Negative/perf-sanity pairing**: assert `drain_waits() == 0` for an
   uncontended DROP AND a separate `drain_waits() == 1` assertion from the
   racing test — both required together (a lone `== 0` check passes
   vacuously if the counter is never wired — same defect class #1005 fixed
   for the variant-coverage check earlier this session; don't repeat it).
4. **Guard-release-on-error test**: force one of the 8 lookup methods to
   error mid-scan, confirm the guard still released (RAII, not manual).
5. **`entry_count` specifically**: a dedicated test proving IT ALSO backs
   off correctly during a drop-in-progress window (it's easy to wire the
   other 7 and forget this one since it doesn't look like a "search").
6. **`drop_index`'s `!existed` rollback branch**: a test proving the guard
   is released correctly on that path specifically (not just the happy
   path).
7. **Regression sweep**: re-run the full `shamir-index` and `shamir-engine`
   suites unchanged (part of the gate below, not a separate ad-hoc step).

Reuse the `f76_drop_visibility_tests.rs` pause-hook + spawn + deterministic
rendezvous pattern the plan's §4 names — no `sleep`-based timing
assumptions. Watch for the posting_cache warm-up gotcha #1011 hit: the
pause-hook probe runs AFTER any cache-warming probe, so sanity checks
issued before installing the pause hook must use DIFFERENT lookup values
than the one used by the parked/racing read, or the racing read may
observe stale cached data instead of genuinely racing the gate.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- `name_interned` in every one of these lookup signatures is the INTERNED
  INDEX NAME, not a field name — don't confuse the two when writing tests.
- Gate: `cargo fmt -p shamir-index -p shamir-engine -- --check`, `cargo
  clippy --workspace --all-targets -- -D warnings`, `./scripts/test.sh -p
  shamir-index -p shamir-engine --full`. Use the wrapper, never raw `cargo
  test`/`cargo nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] `SortedIndexManager` has its own `reader_gate` field, mirroring
      slice 1's `IndexManager` pattern exactly.
- [ ] All 8 named chokepoints (including `entry_count`) gated; `*_tx`
      variants confirmed to delegate into them with no separate change
      needed.
- [ ] `SortedIndexManager::drop_index` wired with the 2.5/3.5/4.5 pattern,
      including correct RAII release on its extra `!existed` rollback
      branch.
- [ ] Regular family's pre-existing ungated `entry_count` left untouched
      (out of scope for this task — noted, not silently fixed).
- [ ] Full test suite per the "Tests" section above, including the two
      easy-to-miss cases (`entry_count` back-off, `!existed` rollback
      guard release).
- [ ] fmt/clippy/test gates green, real output reported.
