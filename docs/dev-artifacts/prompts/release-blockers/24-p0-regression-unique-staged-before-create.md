# Brief — P0 REGRESSION: unique staged-before-CREATE duplicate not validated

Task: #987 in the session TaskList. Found by an adversarial `@oh` review of
the just-completed #957-971 remediation wave (2026-08-04) — a fresh,
independent, empirically-reproduced finding, not part of the original
2026-08-03 review. **This is the single highest-priority item outstanding.**
Read this brief in full — it pins down the exact root cause, the exact fix,
and the exact regression test to add; do not re-derive the analysis.

## The bug — confirmed, reproduced, root-caused

`crates/shamir-engine/src/tx/pre_commit.rs`'s `pre_commit_prelock` function
runs, in order:

1. **Phase 2.5** (lines ~392-540): acquires each barriered table's
   `unique_write_lock` (`uwl_guards`), sorted+deduped for ABBA-freedom.
2. **Phase 2.6** (lines ~542-573): the ONLY place `TxError::UniqueViolation`
   is ever raised — loops `for g in &tx.unique_guards`, checking each
   recorded guard's `index_key` against the current `info_store` state.
3. **Phase 2.7** (line ~608): `rederive_index2_ops_post_stage(tx, repo)` —
   index2 (fts/functional/vector) re-derivation. Unaffected by this bug,
   confirmed sound by the review, do not touch.
4. **P0-2 base_index rederive** (lines ~610-618):
   `rederive_base_index_ops_post_stage(tx, repo).await?` — re-plans
   regular+unique posting ops for any base_index definition created/dropped
   AFTER this tx's stage-time snapshot, and — per its own doc comment —
   "records fresh `UniqueGuard`s so Phase 2.6's commit-time re-validation
   covers the new constraint."

**The bug: step 4 runs AFTER step 2.** The doc comment's claim is false
under the current ordering — `rederive_base_index_ops_post_stage`
(`crates/shamir-engine/src/tx/pre_commit.rs:1129`, specifically the
`tx.record_unique_guard(UniqueGuard {...})` calls inside its per-op loop,
e.g. ~line 1215) adds entries to `tx.unique_guards` — but Phase 2.6's
validation loop (`for g in &tx.unique_guards`, line ~558) has ALREADY
iterated over `tx.unique_guards` and found it empty (or missing the
relevant entry) by the time step 4 populates it. `tx.unique_guards` is read
NOWHERE else in the workspace (Phase 2.6's loop is the sole consumer) — so
every guard the rederive adds is provably a dead write under the current
ordering.

### Concrete failure scenario (empirically reproduced by the review, then reverted — reproduce it again yourself as the new regression test, see below)

1. `R0` is committed with `email = "dup@example.com"` (no unique index
   exists yet on `email`).
2. `T1` stages an `insert_tx` of the SAME value, BEFORE any unique index
   exists on `email`. Stage-time validation has nothing to check against
   (no unique def yet) — passes.
3. `CREATE UNIQUE INDEX` on `email` runs — backfills from the committed
   snapshot, which includes `R0`'s posting.
4. `T1` commits. **Expected: `TxError::UniqueViolation`. Actual: `Ok(())`.**
5. Worse than a missed constraint: `T1`'s `plan_record_created_unique` (the
   unique posting op derived inside `rederive_base_index_ops_post_stage`)
   is a `SetPosting` at the SAME deterministic key `R0`'s posting already
   occupies (the unique posting key is fully determined by
   `(index_id, value)`, not by which record owns it) — so `T1`'s commit
   **overwrites `R0`'s posting**, making `R0` invisible through the unique
   index. This is index corruption, not merely an unconstrained duplicate.

### Why the existing tests don't catch this

`p02_unique_insert_before_create_posting_and_guard_present` and
`p02_unique_concurrent_duplicate_one_wins`
(`crates/shamir-engine/src/table/tests/p02_base_index_rederive_tests.rs`)
both shape the SECOND conflicting write to be the one carrying the guard
(it stages AFTER the unique index exists, so ordinary stage-time validation
catches it) — neither exercises "the FIRST value already occupying the slot
was a pre-existing COMMITTED row, and the CONFLICTING tx is the one that
staged before CREATE and must be caught at commit-time by the rederive's
OWN guard."

## The fix

Move the call `rederive_base_index_ops_post_stage(tx, repo).await?;`
(currently at ~line 618, together with its explanatory comment block
starting ~line 610 — move the comment with the code, updating its wording
if the phase-ordering description needs it) to run:

- **AFTER** Phase 2.5 fully completes — specifically after the
  `unique_tokens`/`uwl_guards` acquisition loop closes (after line ~540,
  where the `for token in &unique_tokens { ... }` loop's closing brace is).
  This ordering constraint is REQUIRED and already safe to satisfy: a table
  gaining its first unique index mid-tx is picked up by Phase 2.5's
  `needs_write_barrier()` scan over `tx.write_set.keys()` (lines ~470-491)
  — `has_unique_indexes()` becomes true the moment `CREATE UNIQUE INDEX`
  registers the def, so that table's token IS added to `unique_tokens` and
  ITS `unique_write_lock` IS acquired in Phase 2.5, even though
  `tx.unique_guards` (built from STAGE-time guards only, line ~468) didn't
  know about it yet. Verify this yourself by reading lines 468-540 before
  making the change — do not just trust this brief, confirm it.
- **BEFORE** Phase 2.6's validation loop (before the `// Phase 2.6` comment,
  ~line 542) — this is the actual fix: Phase 2.6 must see the freshly
  recorded guards.

Do NOT move `rederive_index2_ops_post_stage` (Phase 2.7, line ~608) — leave
its position exactly where it is. It has no dependency on base_index
rederive's position (independent generation counters, independent op
families) and the review confirmed it's sound; moving it is out of scope
and adds needless risk to this fix.

After moving, the new order inside `pre_commit_prelock` is: Phase 2.5 (lock
acquisition) → **base_index rederive (records fresh UniqueGuards)** →
Phase 2.6 (unique re-validation, now sees them) → Phase 2.7 (index2
rederive, unchanged position).

Update the comment block you move (currently ~line 610-617) so it
accurately describes WHY it now runs before Phase 2.6 rather than after —
the existing wording ("records fresh `UniqueGuard`s so Phase 2.6's
commit-time re-validation covers the new constraint") becomes TRUE once
moved; just make sure the surrounding prose doesn't still describe the old
(broken) phase-numbering/ordering. Also correct the misleading impression
this bug leaves for a reader — if there's any comment elsewhere in this
file or `p02_base_index_rederive_tests.rs`'s existing test docstrings that
describes this scenario as already handled, it should be reconciled (do not
leave a stale "this works" comment next to code you just proved didn't).

## Required new test

Add to `crates/shamir-engine/src/table/tests/p02_base_index_rederive_tests.rs`
(reuse the existing file's helpers — `make_repo()`, `key_id()`,
`record_with_str()` — and follow the exact style of the neighboring
`p02_unique_*` tests, e.g. `p02_unique_concurrent_duplicate_one_wins` for
the multi-tx sequencing pattern):

```
p02_unique_staged_before_create_conflicts_with_preexisting_committed_row
```

Reproduce the exact scenario above:
1. Insert (non-tx, committed) `R0` with `email = "dup@example.com"` —
   BEFORE any unique index exists.
2. Begin `T1`, stage an `insert_tx` of the SAME value via `T1`.
3. `CREATE UNIQUE INDEX` on `email`.
4. `T1.commit()` — assert it returns `Err` and is specifically
   `TxError::UniqueViolation` (match the variant, not just `is_err()` —
   the existing neighboring tests use `result.is_err()` with a message;
   for THIS test, since the exact bug was "silently succeeds", assert the
   variant precisely to make a future regression impossible to miss).
5. **Critical additional assertion, not just "commit failed":** confirm
   `R0` is STILL present and STILL correctly found via the unique index
   after the aborted commit — e.g. re-fetch `R0` by id and/or attempt a
   fresh insert of the same value from a THIRD tx and confirm IT is
   rejected too (proving `R0`'s posting was never overwritten/corrupted by
   `T1`'s aborted attempt). Before your fix, run this test manually and
   confirm it FAILS (reproducing the bug) — after the fix, confirm it
   passes. Report both results.

## Regression check — do not let this fix silently break the other rederive tests

After making the change, run the FULL `p02_base_index_rederive_tests.rs`
file (not just your new test) — every existing test in it
(`p02_regular_insert_before_create_posting_present`,
`p02_unique_insert_before_create_posting_and_guard_present`,
`p02_unique_update_before_create_enforced`,
`p02_unique_concurrent_duplicate_one_wins`,
`p02_regular_drop_index_before_commit_no_orphan`,
`p02_unique_drop_index_before_commit_no_orphan`,
`p02_no_indexes_at_stage_rederive_fires`,
`p02_unique_multi_field_before_create_enforced`) must still pass — the
reordering should be a strict improvement (Phase 2.6 now sees a superset of
guards it saw before), so nothing should newly fail. If anything DOES
newly fail, STOP and report it — do not paper over a real regression by
weakening an assertion.

## Scope discipline

- Do NOT touch `rederive_index2_ops_post_stage` or its position — confirmed
  sound, out of scope.
- Do NOT touch the generation-gate mechanism itself
  (`base_index_stage_gens`, `IndexRegistry`/`IndexManager` generation
  counters) — confirmed sound, out of scope. (A separate, lower-priority
  finding about `IndexRegistry::insert`'s generation-tagging linearizability
  is tracked as its own task, #992 — do not attempt it here.)
- Do NOT touch sub-bug 2c (the DROP-between-stage-and-commit retraction
  logic inside the same function) — confirmed sound by the review, out of
  scope. (Its key-length-heuristic fragility is tracked separately as #993
  — do not attempt it here.)
- This is a pure ordering fix (move one function call + its comment block
  earlier) plus one new test. Do not refactor the surrounding function
  beyond what's needed to relocate the call correctly.

## Gate (MANDATORY)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p02_
```
Also run the BROADER engine suite once before finishing, since this touches
a core commit-path function used by every transaction:
```
./scripts/test.sh -p shamir-engine --full
```

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit files and run read-only/test/gate
commands.

## What to report back

Show the exact before/after ordering of the 4 phases inside
`pre_commit_prelock` (a short diff excerpt showing the moved call is
enough — do not paste the whole function). Confirm you manually verified
the new test FAILS against the old ordering and PASSES against the new one
(this is the single most important confirmation — do not skip it). Confirm
every existing test in `p02_base_index_rederive_tests.rs` still passes.
Give exact gate command output for both the scoped `p02_` filter run and
the full `shamir-engine` suite run.
