Task: investigate and fix intermittent failure/slowness of
`vr5_cofilter_sees_staged_and_filters_residual`
(`crates/shamir-engine/src/table/tests/filtered_ann_tests.rs:795-869`).

## Context

This test is flagged as an intermittent (FLAKE) failure — it may pass in
isolation but fail or time out under nextest's parallel load. It builds
25,000 committed records (5,000 "red" + 20,000 "blue", one insert_tx call
per record via `commit_cluster`) plus 2 staged (uncommitted) records,
then runs a vector-similarity + equality co-filter query and asserts on
the result.

## Investigation steps

1. Run the test in isolation multiple times AND under full parallel
   nextest load to reproduce the flake:
   ```
   ./scripts/test.sh -p shamir-engine -- vr5_cofilter_sees_staged_and_filters_residual
   ./scripts/test.sh -p shamir-engine
   ```
   Run the full-crate suite at least 3-5 times in a row (nextest's
   parallelism is what surfaces flakes — a single isolated run rarely
   reproduces it) and note whether it's a genuine FAIL (wrong result), a
   TIMEOUT (>30s / >180s kill), or a SLOW marker.
2. If it's a TIMEOUT/SLOW: this is almost certainly the 25,000-record
   setup cost (`commit_cluster` presumably calls `insert_tx`/commit
   per-record 25,000 times) combined with resource contention when many
   other test binaries run in parallel under nextest. Investigate
   `commit_cluster`'s implementation (grep for it in this test file or a
   shared test helper module) — is it doing 25,000 SEQUENTIAL
   `insert_tx` + commit round-trips? If so, check whether a bulk-insert
   API exists that would be both faster AND less prone to contention
   (matching this repo's `/opti` philosophy — batch over per-row).
   Reducing record count while preserving the test's actual invariant
   (selectivity ≤ 0.20, count > PRE_FILTER_MAX_CANDIDATES=4096) is
   ALSO an acceptable fix if a bulk-insert path doesn't exist or is
   out of scope — the test's own comments state the exact constraints
   (total_live ≥ 25000, selectivity ≤ 0.20) so any smaller construction
   preserving those two properties is valid.
3. If it's a genuine intermittent FAIL (assertion actually fails
   sometimes, not just slow): this points to a real race — investigate
   whether the co-filter/staged-merge path has a genuine concurrency bug
   (e.g. an ordering assumption that only sometimes holds under
   parallel test-binary contention affecting timing-sensitive internal
   state). Do NOT just increase timeouts or retry — find the actual
   race per this repo's CLAUDE.md "hangs and test-locks are BUGS" policy.
4. Fix the stray corrupted comment markers found while investigating:
   `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs:851` has
   `    \ VR-5 — same specific-coordinate check...` (a lone backslash
   where `    //` should be — this is currently INSIDE the token stream
   as a stray line continuation or similar, verify it actually compiles
   correctly today, it likely does by accident via Rust's line-
   continuation-like `\` inside doc comments or is simply dead/ignored,
   but it's clearly meant to be a `//` comment and should be fixed) and
   line 871 `\ **POST-FILTER path** regression: ...` (same issue, in
   what should be a `///` doc-comment for the next test function). Fix
   both to correct `//`/`///` comment syntax. This is a pure typo fix,
   unrelated to the flake logic — do it regardless of what the flake's
   root cause turns out to be.

## Fix

Whatever the root cause turns out to be (slow setup causing timeout
under parallel load, or a genuine race), fix it:
- If slow-setup: reduce iteration count via a legitimate smaller
  construction (preserving the documented invariants) and/or switch to
  a bulk-insert path if one exists and is a clean fit.
- If a genuine race: fix the actual synchronization bug, add a
  regression test if the race is narrow enough to target directly.
- Either way, re-run the full crate suite 3-5 times after the fix to
  confirm the flake no longer reproduces under load.

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```
Run the last command multiple times (3-5x) post-fix to confirm
stability, not just once.

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Flake root cause] slow-setup-under-load / genuine-race / other: <describe>
[Fix applied] <describe>
[Comment typos] fixed at lines 851, 871 (or actual current line numbers)
[Stability verification] N consecutive full-crate runs, all green
```
Full test/gate results (exact commands + pass/fail).
