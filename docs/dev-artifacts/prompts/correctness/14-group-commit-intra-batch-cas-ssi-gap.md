# Brief 14 — group-commit intra-batch lost-update for keyed SSI read-set + Phase CAS (RI-13 gate blocker, CRITICAL)

## ⛔ Mandatory constraints

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND.

Tests are run ONLY through `./scripts/test.sh` (never raw `cargo test`,
blocked by a perimeter guard). Use
`./scripts/test.sh -p shamir-engine -- <name>` for a single test.

## Context

We are running the RI-13 frozen-commit gate for v0.1.0-alpha.1 — the
whole workspace test suite must be reliably green, with zero tolerance
for a demonstrated correctness bug in transaction commit. A full
`./scripts/test.sh --full` run surfaced:

```
thread 'table::tests::version_cas_tests::concurrent_cas_exactly_one_wins' panicked:
expected exactly one commit success and one conflict abort, got:
a_ok=true b_ok=true a_conflict=false b_conflict=false
```

**This is a real lost update, not test flakiness.** Confirmed via 20
back-to-back ISOLATED re-runs (no other test binaries running
concurrently) of just this one test — it failed ~1 in 16 runs even
alone. This rules out CPU-contention-induced flakiness (the class that
explains two OTHER flaky tests fixed earlier this session — a
`slow-timeout` override for `vr5_cofilter_sees_staged_and_filters_residual`
and a recall-tolerance fix for `filtered_ann_low_selectivity_finds_rare`;
neither of those is relevant here, don't touch them). This is a genuine,
reproducible-by-chance race in the commit path itself.

The test (`crates/shamir-engine/src/table/tests/version_cas_tests.rs`,
`concurrent_cas_exactly_one_wins`, ~line 274-365) spawns two concurrent
`tokio::spawn` tasks, both under `IsolationLevel::Serializable`, both
issuing an `update` with `.expected_version(v1)` (the SAME expected
version, read once before either transaction begins) against the SAME
row. Exactly one must succeed and the other must abort with a conflict
(`SsiConflict`, `PhantomConflict`, or — since FG-7 — `CasConflict`).
Instead, both committed.

## Root cause (already diagnosed — do not re-investigate from scratch)

`crates/shamir-engine/src/tx/group_commit.rs`'s `run_leader` (starts
~line 61) batches multiple queued transactions (one leader + N
followers that lost the `try_commit_lock` race) under a SINGLE
`commit_lock` acquisition. It:

1. Validates each batch member SEQUENTIALLY in a `for mut entry in
   entries` loop (~line 153-288), calling `pre_commit_locked_validate`
   (from `crates/shamir-engine/src/tx/pre_commit.rs`) per entry.
2. Only AFTER the whole loop finishes does it WAL-begin and
   **materialize/publish** every validated survivor, as a separate
   batch (Step 3/4, ~line 296 onward — `materialize(&mut work.tx, ...)`
   runs in a loop starting ~line 396, entirely AFTER the validation
   loop).

The code ALREADY has explicit intra-batch conflict protection — but
**only for phantom/predicate conflicts**. Read the "P3a: batch-local
footprint accumulator" comment at group_commit.rs lines ~148-151 and
the `phantom_conflict` block at lines ~169-187 carefully: as each
survivor passes `pre_commit_locked_validate`, its write-footprint is
pushed onto `batch_footprints` (a `Vec<CommitWriteRecord>`), and each
SUBSEQUENT survivor's `predicate_set` is checked against
`batch_footprints` via `shamir_tx::record_conflicts` — explicitly
closing "the gap for intra-batch phantoms" (the comment's own words).

**There is no equivalent for:**

1. **Ordinary keyed SSI read-set validation** — Phase 2 inside
   `pre_commit_locked_validate` (`crates/shamir-engine/src/tx/pre_commit.rs`
   ~lines 438-450) validates `tx.read_set` entries via
   `tx.validate_read_set(|t, k| provider.version_of(t, k))` —
   `provider.version_of` reflects only ALREADY-PUBLISHED committed
   state. A batch survivor validated earlier in the SAME loop iteration
   has not been materialized/published yet (that happens later, in the
   separate Step 4 loop), so its write is invisible to `provider` when a
   LATER survivor in the same batch validates its own read-set.
2. **Phase CAS** (FG-7, same file, ~lines 469-513) — same
   `provider.version_of` blind spot, for `tx.cas_set` instead of
   `tx.read_set`.

This means: if task_a (leader) and task_b (follower) — both with
`expected_version(v1)` against the same row — land in the SAME
group-commit batch, task_a validates first (sees v1 via `provider`,
still unpublished elsewhere too, passes), is NOT yet published. Task_b
validates second in the SAME loop and ALSO sees v1 (task_a's write
genuinely isn't visible to `provider` yet) — both pass, both commit.
This reproduces the exact observed failure.

**This is very likely a pre-existing architectural gap, not something
introduced this session.** FG-7's Phase CAS simply inherited the same
blind spot plain keyed SSI reads already had before FG-7 touched
anything; P3a only ever closed the predicate/phantom half of this class
of intra-batch race. Do not scope the fix narrowly to "just CAS" — Phase
2's ordinary keyed read-set has the identical gap and must be fixed
too, in the same pass, using the same mechanism.

## The fix

Mirror the existing P3a `batch_footprints` pattern
(`group_commit.rs` ~lines 148-227) but for **exact key comparison**
instead of predicate matching:

1. Maintain a batch-local accumulator of write-set keys from
   EARLIER-VALIDATED-BUT-NOT-YET-PUBLISHED survivors in this batch —
   `compute_write_set_keys` (already defined at `group_commit.rs:29`,
   returns `TFxSet<(u64, Bytes)>`) is the exact primitive already used
   elsewhere in this file for computing a tx's write-set keys; accumulate
   these into a running batch-local set as each survivor is accepted
   (parallel to how `batch_footprints` already accumulates per survivor
   at line ~223-226).
2. For each NEW entry being validated (after `pre_commit_locked_validate`
   returns `Ok(Some(vpc))`, alongside the existing `phantom_conflict`
   check at ~line 169), additionally check whether ANY of this entry's
   OWN `read_set` keys OR `cas_set` keys collide with the accumulated
   batch-local write-key set from earlier survivors. `TxContext::read_set`
   and `TxContext::cas_set` are both `scc::HashMap<(u64, Bytes), u64,
   THasher>` (`crates/shamir-tx/src/tx_context.rs` lines ~160, ~167) —
   iterate with `.iter_sync(...)` (see `validate_read_set` at
   `tx_context.rs:544` and the existing Phase CAS loop at
   `pre_commit.rs:492` for the exact iteration idiom already used in this
   codebase for these two maps).
3. On a collision: abort this survivor with the appropriate conflict
   error — `TxError::SsiConflict` for a `read_set` collision,
   `TxError::CasConflict` for a `cas_set` collision (construct the same
   way the existing single-tx Phase 2 / Phase CAS blocks in
   `pre_commit.rs` do — reuse those error shapes exactly, e.g.
   `TxError::CasConflict { key, expected, found }` where `found` should
   be the commit_version of the earlier batch survivor whose write
   collided, since that's the value this survivor WOULD have observed
   had it validated after that survivor's publish instead of before it).
4. Wire the abort through the exact same follower/leader handling
   already present for the `phantom_conflict` block immediately above it
   (~lines 189-219) — leader failure aborts the whole batch (drains
   `validated`, notifies followers of failure, returns the error);
   follower failure notifies just that follower's oneshot and
   `continue`s the loop for the rest of the batch. Copy this control-flow
   shape, don't invent a new one.
5. Decide implementation placement: either (a) add an optional
   extra parameter to `pre_commit_locked_validate` carrying the
   batch-local write-key accumulator so the check lives inside
   `pre_commit.rs` alongside Phase 2/Phase CAS (parallel structure, but
   touches the single-tx call sites in `commit.rs` too — they'd pass an
   empty/absent accumulator), or (b) keep it entirely inside
   `group_commit.rs`'s loop, checking the survivor's `read_set`/`cas_set`
   against the batch accumulator right after `pre_commit_locked_validate`
   returns `Ok(Some(vpc))`, structured exactly like the existing
   `phantom_conflict` block. **Prefer (b)** — it mirrors the existing P3a
   placement exactly, keeps `pre_commit_locked_validate`'s signature
   unchanged (no ripple into the single-tx call sites in
   `commit.rs`/`pre_commit.rs`'s own single-tx `pre_commit_locked`), and
   the gap is specifically a group-commit-batching problem, not a
   single-tx-path problem (see next paragraph).

**Do NOT touch the single-tx path** (`commit_tx_lockfree` /
`commit_tx_inner_legacy_async` in `crates/shamir-engine/src/tx/commit.rs`,
or the non-batched `run_single_tx` in `group_commit.rs`). That path
processes exactly ONE transaction per `commit_lock` acquisition with no
batch peers to race against within its own critical section — Phase
2/Phase CAS already correctly see all prior commits via `provider`
there, since every prior commit fully published (materialize completed)
before releasing the lock (confirmed: `commit_tx_lockfree` drops
`_serializable_guard` only AFTER `materialize()` returns, `commit.rs`
~line 789). Verify this understanding holds during your investigation;
if you find a reason it's NOT actually safe, stop and report rather than
silently expanding scope.

## Verification (do this yourself before reporting done)

1. **Primary reproducer**: run
   `./scripts/test.sh -p shamir-engine -- concurrent_cas_exactly_one_wins`
   **30 times back-to-back** (a bash loop). The pre-fix reproduction
   rate was ~1/16 in isolation — 30 clean runs is strong evidence the
   race is closed. Report the pass count (must be 30/30).
2. Run the whole `version_cas_tests` module and the whole
   `group_commit`-related test modules (search for test files
   referencing `group_commit`/`run_leader`/batch-commit scenarios) —
   all must pass.
3. Run `./scripts/test.sh -p shamir-engine` (whole crate) — must be
   fully green, no new failures introduced by this change.
4. `cargo fmt --all -- --check` and
   `cargo clippy --workspace --all-targets -- -D warnings` — both clean.
5. Report the full diff and all command outputs in your final summary.
   If you conclude the single-tx path also needs a change (contrary to
   this brief's expectation), explain exactly why with file:line
   evidence before making that change — don't silently expand scope.
