# #1104 MEDIUM — close the test-coverage gap on `update_tx_bytes`'s `is_record_touched` closures

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

⛔ This brief is running in an isolated git worktree, in parallel with
another agent working on task #1100 in the MAIN repo directory (not a
worktree). #1100 touches `crates/shamir-engine/src/table/table_manager_tx_ops.rs`'s
`delete_tx`/`update_tx`/`update_tx_bytes` PLANNING logic and possibly
`crates/shamir-engine/src/tx/pre_commit.rs`. **This task is test-only —
do not modify any production code.** If your investigation makes you want
to change production logic (not just add tests), STOP and report it in
your final summary instead of editing it — that's #1100's territory, not
this task's.

## Background

Found by an `@oh` adversarial review of `#1099`'s merge (commit
`6a78d8f5`, already on `master`). `update_tx_bytes`
(`crates/shamir-engine/src/table/table_manager_tx_ops.rs:1246` and
`:1315` — two branches, map-lens and tree-fallback) has two
`is_record_touched` closures that are **never invoked by any test in the
crate**. Verified empirically by the reviewer: inserting `panic!()` as
the first statement of both closures and running
`./scripts/test.sh -p shamir-engine --full` still passed all 2010 tests
cleanly.

Root cause: the only test reaching `update_tx_bytes`'s unique validation
(`crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check.rs:800`,
`update_tx_bytes_to_a_durably_owned_unreleased_key_still_rejects`) uses a
brand-new `tx2` with an empty `tx.index_write_set`, so
`released_unique_keys_in_tx` returns an empty set, and the
`released_in_tx.contains(...) && is_record_touched(...)` short-circuits on
the FIRST condition (`index_manager_unique.rs:302`) before
`is_record_touched` is ever called.

This gap is PRE-EXISTING (identically untested before `#1099`, when the
parameter was a pre-built `&TFxSet<[u8;16]>` instead of a closure) — this
task closes it, it isn't fixing a regression `#1099` introduced. But
`update_tx_bytes` is explicitly the O(N²) hot path `#1099` exists to fix
(the one called PER ROW on the wire transactional-UPDATE path), making it
the single most load-bearing site among the 6 that got this treatment —
and it currently has the LEAST test coverage of any of them.

### Failure scenario this hides

A wrong `table_token`, an inverted condition, or passing the candidate id
instead of `existing_id` in either closure would let a stale-snapshot
release plan be tolerated on the wire UPDATE path — the exact SECURITY
bypass `#1096` was opened for — and the full suite would stay green with
zero signal.

## What to build

Read `p1096_tx_aware_unique_check.rs` in full first — it already has the
established test shapes and helpers for tx-aware unique-check scenarios;
match its style and helper reuse rather than inventing new patterns.

1. **Positive test, driven through `update_tx_bytes` specifically** (not
   `update_tx`/`insert_tx`, which route through different code and are
   already covered): `tx: delete_tx(A{email:"x"})` then
   `update_tx_bytes(D, ..., email="x")` in the SAME tx must **succeed**
   (release-then-reclaim via the wire UPDATE path).
2. **Negative test, driven through `update_tx_bytes` specifically**: the
   `stale_snapshot_release_does_not_bypass_a_concurrent_reclaim` shape
   already in `p1096_tx_aware_unique_check.rs` (a release planned against
   a snapshot that's gone stale because a CONCURRENT tx already reclaimed
   the key for an unrelated record) — reproduce the same race but drive
   the reclaiming half through `update_tx_bytes` instead of `insert_tx`,
   and confirm it correctly **rejects** (`DuplicateKey`/`UniqueViolation`).
3. Investigate the same gap for `insert_tx_many`/`insert_tx_many_bytes`/
   `update_tx` — the reviewer noted these are only covered in the
   POSITIVE direction (existing tests at `p1096_tx_aware_unique_check.rs:425`,
   `:660`, `:706`), meaning a `|_| true` mutation on any of their closures
   would also go unnoticed. Add the equivalent NEGATIVE-direction test for
   each, mirroring item 2's shape, unless your own investigation finds one
   already exists elsewhere under a name that didn't show up in the
   reviewer's grep — verify, don't assume the gap is real without
   checking yourself first.

## Verification — mutation-test every new test (this session's established discipline)

For EACH closure you add coverage for, temporarily mutate it to `|_| false`
and separately to `|_| true`, confirm your new test(s) catch BOTH
mutations (fails when mutated, passes when restored), then restore the
real closure. Report each mutation result in your final summary — this is
the whole point of the task; a new test that doesn't actually fail under
either mutation has zero regression value and doesn't close the gap.

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

All three must pass clean. Report the real pass/fail counts and the full
mutation-test results (which closures, which mutation direction, pass/fail
before and after) in your final summary.
