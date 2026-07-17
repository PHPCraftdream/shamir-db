# #666 follow-up — redesign `max_execution_time_secs` enforcement to be genuinely cancel-safe

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug this fixes (confirmed by two independent readings — the review, then the orchestrator)

Commit `e1ae30e3` (#666) wrapped the top-level
`crates/shamir-engine/src/query/batch/batch_execute.rs::execute_batch`
entry point in `tokio::time::timeout(budget,
execute_batch_impl(...)).await`, mapping a timeout to
`BatchError::ExecutionTimedOut`. This is **unsafe** for two independently
confirmed reasons:

1. **`commit_tx` explicitly forbids being raced under a timeout.** Its own
   doc comment (`crates/shamir-engine/src/tx/commit.rs:238-249`):

   > cancel-safe: partial — the commit point is a *successful* Phase 4
   > `wal.begin`. Before that, cancellation is a CLEAN ABORT... AFTER
   > that, the tx is COMMITTED: the WAL entry is durable and is the
   > single source of truth. Cancellation between Phase 4 and Phase 7
   > leaves an inflight WAL marker that recovery replays idempotently on
   > the next open... the caller simply does not observe the `Ok`
   > outcome, but the data is not lost and the tx is not half-aborted.
   > **Treat as non-cancel-safe at the API boundary: do not race under
   > `tokio::select!` / `tokio::time::timeout` if you need to observe the
   > outcome.**

   #666's timeout wraps `execute_batch_impl` end to end, which — for a
   transactional batch — calls `execute_transactional_impl`
   (`batch_execute.rs:452`), which calls `repo.commit_tx(tx).await`
   (`batch_execute.rs:549`) INSIDE the timeout's scope. If the timeout
   fires while `commit_tx` is between Phase 4 and Phase 7, the client gets
   `Err(ExecutionTimedOut)` — and the doc comment #666 wrote for that
   variant falsely claims "the TxContext is dropped without commit (RAII
   rollback)" — while the transaction is ACTUALLY durably committed and
   will be replayed on the next open. This is a genuine WAL/in-memory
   state divergence bug, not a theoretical concern.

2. **`TxContext` has no `Drop` impl, so pessimistic locks leak on
   cancellation.** `crate::tx::release_pessimistic_locks(&tx, &repo)` is
   called EXPLICITLY, only on the normal `Err` arm of
   `execute_transactional_impl` (`batch_execute.rs:534-540`) — NEVER via
   `Drop` (confirmed: `grep -rn "impl Drop" crates/shamir-tx/src` finds
   `CellReservationGuard`/`SnapshotGuard`/`VersionGuard`, but no
   `TxContext`). If `tokio::time::timeout` drops the
   `execute_batch_impl` future while it is anywhere INSIDE
   `execute_plan_tx_impl`'s execution (i.e. the far more common case than
   landing exactly inside `commit_tx` — this is simply "the batch was
   still running an op when the budget expired") for a `Pessimistic`-
   isolation batch, the `match plan_result { Err(..) => release_pessimistic_locks(...), ... }`
   arm in `execute_transactional_impl` never runs at all (the future is
   dropped mid-`.await`, never resuming to reach that `match`) — so any
   Level-3 locks already acquired by ops that had already executed are
   NEVER released. Per `crates/shamir-tx/src/tests/mvcc_store_tests/lock_tests.rs`'s
   `lock_key_younger_waits_for_older`, under wound-wait a younger tx
   waits UNBOUNDEDLY for an older lock holder — so this is a PERMANENT
   leak (until process restart), and the DoS gate meant to protect the
   server becomes itself a DoS vector: repeatedly submitting
   pessimistic batches that time out poisons more and more keys.

3. **The test written to prove the timeout rolls back a transactional
   batch's writes is vacuous.** `dos_gate_tests.rs`'s
   `execute_batch_timeout_during_transactional_batch_rolls_back_partial_writes`
   uses a `SlowResolver` that delays EVERY `resolve()` call — including the
   ones `validate_tables` makes for BOTH tables BEFORE `begin_tx` is ever
   reached. With the test's 1s budget and 1.3s delay, the timeout fires
   during that up-front validation — `begin_tx` never runs, the "good"
   insert never executes, and the "no partial rows survive" assertion
   passes trivially (it would pass even with the bug fully intact). This
   needs to be fixed as part of this task too (see Tests below).

## Your task — design AND implement a genuinely cancel-safe replacement

You have full latitude to choose the mechanism, but any solution MUST
satisfy ALL of these:

- **`commit_tx` must be allowed to run to completion, uncancelled, every
  time it is called.** No `tokio::time::timeout`/`tokio::select!` may
  ever race the future that contains a `commit_tx(...).await` call.
- **A batch's pessimistic locks must always be released through the
  EXISTING `release_pessimistic_locks` call sites** (the `Err` arm of
  `execute_transactional_impl`, and wherever else it's already called) —
  i.e. any timeout-detected condition must be surfaced as an ordinary
  `Err` that flows through the EXISTING error-handling machinery, not as
  an external `Future::drop`-based cancellation that bypasses it.
- **The DoS protection must remain real** — a batch that keeps doing work
  (many stages, many `ForEach` iterations, a pathologically large single
  stage) past its budget must actually be stopped from continuing to
  consume server resources, not merely "detected after the fact and
  reported as failed after running to completion anyway" (that would
  defeat the entire purpose of #666 and is NOT an acceptable outcome).

### A promising direction worth evaluating (not mandated — use your judgement)

Replace PREEMPTIVE cancellation (`tokio::time::timeout` wrapping the whole
execution) with **COOPERATIVE deadline checks at existing safe
checkpoints** — points where control already returns to a loop between
discrete units of work, so inserting a check there costs nothing in
correctness and doesn't touch anything mid-flight:

- The per-alias loop inside `execute_plan_impl` and `execute_plan_tx_impl`
  (`batch_execute.rs:320-354` and `:410-...`) — once per stage-alias,
  BEFORE calling `execute_single_impl`/dispatching the op.
- The per-iteration loop inside `ForEach`'s handling in
  `crates/shamir-engine/src/query/batch/query_runner.rs` (~line 530,
  `for element in elements`) — once per iteration, before recursing into
  the loop body.
- Possibly the per-sub-batch recursion in `BatchOp::Batch`'s handling too.

At each checkpoint, compare `Instant::now()` against a deadline computed
once at the top of `execute_batch` (or threaded down as a parameter/field
— your choice of plumbing) and threaded through the SAME call chain
`depth`/`params` already travel through (mutual recursion between
`QueryRunner::run`/`execute_batch_impl`/`execute_plan_tx_impl` already
exists — this is one more piece of context to carry, not a new
recursion shape). If the deadline has passed, return
`Err(BatchError::ExecutionTimedOut { budget_secs })` THROUGH THE NORMAL
RETURN PATH — exactly like any other op failure — so it flows through
`execute_transactional_impl`'s EXISTING `Err` arm (which already calls
`release_pessimistic_locks` and never calls `commit_tx`), with ZERO new
cancel-safety surface.

This means `execute_batch` no longer needs `tokio::time::timeout` at all
— `commit_tx` is never raced against anything, and pessimistic lock
release is never bypassed, because nothing is ever externally cancelled;
the deadline is simply data every checkpoint consults and reacts to like
any other precondition. Consider whether this genuinely closes the DoS
concern for the realistic threat model (many cheap ops/iterations
accumulating time — which is what a `ForEach`/large batch DoS actually
looks like) — a SINGLE op that itself hangs forever inside one `.await`
(e.g. a pathological I/O stall) is a DIFFERENT problem this checkpoint
approach does NOT solve, and you should explicitly say in your summary
whether that gap is: (a) already out of scope (a single op hanging is a
different failure class, arguably a resource/IO-layer concern, not what
`max_execution_time_secs` was ever meant to bound), or (b) something you
found a way to also cover, or (c) a residual gap worth flagging as a
separate, smaller follow-up.

**If you find a genuinely better design, use it instead** — the
cooperative-checkpoint idea above is a starting hypothesis from the
orchestrator's own investigation, not a mandate. In particular, if you
determine that an `Arc`-ification of `resolver`/`admin`/`invoker`
(currently `&'a dyn Trait` borrowed references, not `'static` — which is
why `tokio::spawn` + "race only the `JoinHandle`" isn't directly
available today) combined with a spawn+detach pattern is actually a
cleaner or more robust fix and is achievable without destabilizing the
rest of the codebase, you may propose/attempt that instead — but weigh
its much larger blast radius (every caller of `execute_batch`/
`TableResolver`/`AdminExecutor`/`FunctionInvoker` across the workspace)
against the cooperative-checkpoint approach's smaller footprint before
committing to it. State your reasoning for whichever direction you pick.

## What to do with the existing (buggy) #666 timeout code

Remove the `tokio::time::timeout` wrapper from `execute_batch`
(`batch_execute.rs`) entirely and replace it with your chosen mechanism.
Keep the `BatchError::ExecutionTimedOut { budget_secs: u64 }` variant (it
is still the right error shape/message for a deadline-exceeded outcome)
— just change WHERE and HOW it gets raised. Keep `#666`'s OTHER fix (the
`max_iterations` hard ceiling / `effective_max_iterations` in
`query_runner.rs`) completely untouched — it has no cancel-safety issue
and is unrelated to this bug.

## Tests

1. **Fix the vacuous test**:
   `execute_batch_timeout_during_transactional_batch_rolls_back_partial_writes`
   in `crates/shamir-engine/src/query/batch/tests/dos_gate_tests.rs` must
   be adjusted so the "good" insert PROVABLY completes (and, under your
   new mechanism, is provably staged in the tx) before the deadline
   check/detection fires on a LATER stage/iteration — e.g. delay only the
   SECOND table's resolve (match on table name in `SlowResolver`), or
   switch the harness entirely to something that naturally consumes
   several cooperative-checkpoint boundaries (e.g. a `ForEach` with many
   iterations and a deadline that expires partway through) rather than
   simulating slowness via `resolve()`. Whichever you choose, the test
   must demonstrably fail (partial write survives, no error returned) if
   you mentally revert your fix — walk through why in your summary.
2. **New test proving `commit_tx` is never raced**: construct a scenario
   where the deadline is ALREADY expired by the time the last stage
   finishes and `execute_transactional_impl` would be about to call
   `commit_tx` — assert the batch resolves to `Err(ExecutionTimedOut)`
   WITHOUT ever having committed (fresh read shows nothing), AND (if
   feasible to observe/assert directly, e.g. via a counter/flag your
   checkpoint mechanism can expose in a test-only way) that `commit_tx`
   was never even entered. If directly observing "commit_tx was never
   called" isn't practical without invasive instrumentation, a strong
   proxy is fine: assert the timeout fires and NO write survives, for a
   batch shaped so that — under the OLD, buggy code — the timeout would
   plausibly have landed during/after commit (e.g. a single-stage,
   single-op transactional batch, which under the checkpoint model can
   only be checked BEFORE that one op runs, never mid-commit).
3. **Non-regression**: re-run all of #666's existing tests
   (`for_each_max_iterations_*`, `execute_batch_within_time_budget_succeeds_unaffected`)
   — must still pass unchanged.
4. **Pessimistic-isolation-specific test** (the concrete finding-2
   scenario): a `Pessimistic`/`level3`-isolation transactional batch with
   multiple stages/ops, whose LATER stage's deadline check fires — assert
   (a) the batch errors with `ExecutionTimedOut`, and (b) a
   SUBSEQUENT, otherwise-unrelated transaction that would need the SAME
   lock the timed-out batch had already acquired can proceed promptly
   (does not hang) — proving locks were genuinely released via the normal
   `Err`-arm cleanup path, not leaked. If constructing a true multi-tx
   lock-contention test is disproportionate effort, an acceptable
   alternative is a MORE targeted unit test directly asserting
   `release_pessimistic_locks` (or equivalent internal state) ran — use
   your judgement, but do not skip verifying this finding is closed.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green.
- `./scripts/test.sh -p shamir-client --full -- for_each` green (e2e,
  unaffected by this change but worth re-confirming).
- `cargo fmt -p shamir-engine -p shamir-query-types -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Explicitly re-state, in your own words, how your final design satisfies
  ALL THREE bullet points in "Your task" above (commit_tx never raced,
  pessimistic locks never bypassed, DoS protection remains real) — don't
  just assert it, walk through the control flow.

## Out of scope

- Do NOT touch #661/#662/#663/#665/#667 — separate, already-completed
  tasks.
- Do NOT change `BatchLimits`'s wire shape or defaults.
- Do NOT remove the `max_iterations` ceiling fix from #666 — only the
  wall-clock-timeout mechanism is being redesigned here.
