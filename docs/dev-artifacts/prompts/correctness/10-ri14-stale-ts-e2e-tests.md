# RI-14: update 2 stale TS e2e tests to match already-landed behavior

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Found during RI-5 verification (first real TS e2e run against a real
server). Neither is a new bug — both are tests pinned to OLD behavior that
has since changed for the better. Fix the tests to match current, correct
behavior; do not touch the underlying implementation.

## Fix 1 — `e2e-cond.test.ts` ($cond query-ref planner bug, already fixed)

File: `crates/shamir-client-ts/src/__tests__/e2e-cond.test.ts`, ~lines
150-256. The test "documents a known bug: $cond branch referencing a prior
query result silently misses" pins OLD buggy planner behavior
(`BatchPlanner::extract_deps_from_filter_value` allegedly not recursing
into `Cond`/`Expr`/`FnCall`).

**Verified (orchestrator, already confirmed):**
`crates/shamir-query-types/src/batch/planner.rs:618-648` — the function
ALREADY recurses into `Cond`/`Expr`/`FnCall`; the catch-all `_ => {}` now
only applies to true leaves (`FieldRef`/`Param`) that have no nested
filter-values. The bug is already fixed. Re-verify this yourself by reading
the current `planner.rs` before touching the test — confirm the recursion
is really there.

Action:
1. Run the test as-is first to see the CURRENT actual result (should now be
   `['heidi', 'ivan']` instead of the pinned `['ivan']`).
2. Update the assertion to the new correct result.
3. Remove/replace the stale ~30-line "KNOWN ENGINE BUG" comment block
   (~lines 150-186) — it should no longer claim a bug exists.
4. Rename the test — it no longer documents a bug, it's now a positive
   regression test for correct `$query-ref`-inside-`$cond` semantics (e.g.
   `"$cond branch referencing a prior query result resolves correctly"`).
5. Check the Rust twin `crates/shamir-client/tests/batch_cond_e2e.rs`
   (referenced in the TS test's comment as "found while writing the Rust
   twin") — if it ALSO pins the old buggy semantics, update it the same
   way, synchronously.

## Fix 2 — `e2e-vector.test.ts` (efSearch clamp vs. client-side reject)

File: `crates/shamir-client-ts/src/__tests__/e2e-vector.test.ts`, ~lines
400-424. The test "efSearch: huge value is clamped, not rejected" expects
the client to forward an oversized `efSearch` to the server for a
server-side clamp.

**Verified (orchestrator, already confirmed):**
`crates/shamir-client-ts/src/core/builders/filter.ts:272-277` already has a
CLIENT-SIDE guard: `vectorSimilarity()` throws an `Error` when
`efSearch > MAX_EF_SEARCH` (10000), BEFORE sending anything to the server —
a deliberate decision (the guard's own comment: "the server would silently
clamp it instead of rejecting it, which degrades recall without telling
the caller").

Action: rewrite the test to match the CURRENT, intentional contract —
`expect(() => filter.vectorSimilarity(...)).toThrow()` for an over-limit
`efSearch` value (client-side rejection is the expected, correct behavior;
the client-side guard is a deliberate improvement, not an accidental
regression — confirm this reading of intent by re-checking the guard's own
comment in `filter.ts` before committing to this direction, and note in
your report if you find evidence it was actually unintentional).

## Verification (MANDATORY before you report done)

Goal: the `ts-e2e-nightly.yml` workflow's `ts-e2e` job must go green on the
next run. Both fixes are covered by the existing (updated) tests — no new
tests are required, only bringing existing tests in line with actual
behavior.

1. `cargo build --release -p shamir-server` (real server binary for e2e).
2. `cd crates/shamir-client-ts && npm ci && npm run typecheck && npm test`
   — full suite must pass, including your two updated tests.
3. If you touched the Rust twin: `./scripts/test.sh -p shamir-client --full`.
4. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
   -- -D warnings` clean (only relevant if the Rust twin was touched, but
   run both regardless — cheap confirmation nothing else drifted).

Report literal command output for all of the above, plus whether the Rust
twin needed the same fix or was already correct.

If either "verified" claim above (the planner fix, or the client-side
guard) turns out NOT to hold when you re-check it yourself — STOP and
report the discrepancy precisely rather than forcing the test to match a
belief that doesn't hold in the current tree.
