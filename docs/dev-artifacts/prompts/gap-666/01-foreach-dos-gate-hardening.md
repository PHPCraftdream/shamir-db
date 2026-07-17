# #666 — harden ForEach's DoS gates: clamp `max_iterations`, enforce `max_execution_time_secs`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — three findings from the final-session `@fh` review (F6),
## independently confirmed by the orchestrator via direct code reading

`BatchLimits` (`crates/shamir-query-types/src/batch/batch_limits.rs`) is
entirely **client-supplied wire data** — every field, including
`max_iterations` and `max_execution_time_secs`, comes straight from the
`BatchRequest` the client sends (nested `Batch`/`ForEach` bodies each
carry their OWN independent `limits`, per #661's fix). This is fine for
`max_queries`/`max_dependency_depth`/`max_nesting_depth` (all enforced via
`BatchPlanner::plan`'s validation at plan time — a client requesting a
too-generous value just gets rejected against a fixed structural check).
But two of the SIX `BatchLimits` fields have no independent server-side
ceiling at all:

### Confirmed finding 1 — `max_iterations` is fully client-controlled, unclamped

`crates/shamir-engine/src/query/batch/query_runner.rs:520-521`:

```rust
let max_iterations = fe.batch.limits.max_iterations;
if elements.len() > max_iterations {
    return Err(BatchError::TooManyIterations { .. });
}
```

`max_iterations` here is `fe.batch.limits.max_iterations` — a value the
CLIENT supplies in the wire request, with **no independent hard ceiling**.
For a `ForEach` whose `over` is a literal array, `BatchPlanner::plan`'s
`virtual_units = iterations × body_len` check (planner.rs, ~line 137)
gives SOME protection via `max_queries` — but for a DYNAMIC `over` (a
`$query`-column-ref against a real table, resolved only at runtime), the
planner cannot know the iteration count ahead of time and this static
check is skipped entirely. The ONLY runtime protection is the
`elements.len() > max_iterations` gate above — and a client can simply set
`max_iterations` to `usize::MAX` (or any value ≥ the size of whatever
table it's iterating) in the SAME nested body whose iteration count it
wants unbounded, completely defeating the gate that's supposed to protect
the SERVER, not the client's own request.

### Confirmed finding 2 — `max_execution_time_secs` is a dead config value, enforced nowhere

Confirmed by grep: `grep -rn "max_execution_time_secs" crates/shamir-engine/src` (excluding tests)
returns **zero matches** outside `batch_limits.rs`'s own definition and
its `BatchError`/planner doc comments. `execute_batch_impl`
(`crates/shamir-engine/src/query/batch/batch_execute.rs:74-166`) DOES
track wall-clock time via `let start = Instant::now();` (line 75) /
`let elapsed = start.elapsed();` (line 166) — but this is used ONLY to
populate `BatchResponse.execution_time_us` for **telemetry**, never
compared against `request.limits.max_execution_time_secs` to actually cut
anything off. A client can set an arbitrarily large (or the default 30s,
which is still not enforced) budget and a batch that takes minutes/hours
(e.g. a huge `ForEach` over a dynamic `over`, or a pathological filter)
runs to completion (or forever) regardless of what the client claimed as
its own time budget.

### Finding 3 — cross-level `max_queries` is NOT a cumulative/global budget (documented here, NOT fixed in this task — see "Out of scope" below)

Each nested `Batch`/`ForEach` body is independently re-planned at
EXECUTION time via `BatchPlanner::plan(&body.queries, &body.limits)`
(`query_runner.rs:189`'s `run_nested_body_in_outer_tx`, and
`batch_execute.rs:89`'s `execute_batch_impl`) — each call checks
`queries.len() > limits.max_queries` against ONLY that level's OWN direct
query count and ONLY that level's OWN (independently client-supplied)
`limits`. There is no GLOBAL, cumulative "total ops executed across the
whole recursive tree" counter. Since `max_nesting_depth` IS correctly
composed globally (`BatchPlanner::plan`'s initial `max_nesting_depth_of_queries`
call walks the ENTIRE tree recursively in one pass, bounding total
STRUCTURAL depth against the top-level request's own limit), the
worst-case total op count is bounded by (roughly)
`max_queries_per_level ^ max_nesting_depth` — large, but FINITE — rather
than literally unbounded. This task does not attempt to fix this (seeAlex
"Out of scope"); items 1+2 below (iteration ceiling + wall-clock timeout)
already substantially bound the PRACTICAL damage a too-large total op
count can do, since even a large-but-finite amount of work is now capped
in wall-clock time.

## The fix

### Fix 1 — hard server-side ceiling on `max_iterations`

In `crates/shamir-engine/src/query/batch/query_runner.rs`, define a
crate-visible constant near the `ForEach` handling (or in a sensible
shared location — check if `crates/shamir-query-types/src/batch/batch_limits.rs`
already has a natural home for gate constants; if not, a `const` right
above the `ForEach` arm in `query_runner.rs` is fine):

```rust
/// Absolute, server-enforced ceiling on `ForEach` iteration count —
/// independent of whatever a client-supplied `BatchLimits.max_iterations`
/// claims. Closes a DoS gap: `max_iterations` is entirely client-supplied
/// wire data (#666), so a client could otherwise set it to `usize::MAX`
/// and defeat the gate for a `ForEach` with a dynamic (non-literal-array)
/// `over` source, whose iteration count the plan-time `virtual_units`
/// check (`BatchPlanner::plan`) cannot see ahead of time.
const ABSOLUTE_MAX_FOR_EACH_ITERATIONS: usize = 100_000;
```

Change the runtime gate (~line 520-521) to:

```rust
let max_iterations = fe
    .batch
    .limits
    .max_iterations
    .min(ABSOLUTE_MAX_FOR_EACH_ITERATIONS);
if elements.len() > max_iterations {
    return Err(BatchError::TooManyIterations {
        alias: alias.to_string(),
        actual: elements.len(),
        max: max_iterations,
    });
}
```

(Adjust the exact `BatchError::TooManyIterations` field names to match
whatever the existing construction already uses — do not change the
error TYPE, just the value fed into `max`.) Pick `100_000` unless you find
a more established convention elsewhere in this codebase for a similar
"absolute ceiling regardless of client request" pattern — if so, prefer
consistency with that existing convention and note it in your summary.

### Fix 2 — enforce `max_execution_time_secs` via a wall-clock timeout at the top-level `execute_batch` entry point

Scope this to the SINGLE top-level public entry point,
`crates/shamir-engine/src/query/batch/batch_execute.rs`'s `execute_batch`
(line 26) — NOT the recursive `execute_batch_impl` (nested-body
re-entrant calls already run within the outer call's own timeout budget
transitively, so wrapping only the outermost call is sufficient and
avoids nested-timeout complexity) and NOT
`crates/shamir-engine/src/query/batch/interactive_tx.rs`'s
`execute_in_open_tx` (an already-open interactive tx has a DIFFERENT
lifecycle contract — the caller owns `tx: &mut TxContext` across
POTENTIALLY MANY separate `execute_in_open_tx` calls before an eventual
`commit_interactive_tx`/rollback, so a single-call wall-clock timeout
here is a different, out-of-scope problem — see "Out of scope" below).

```rust
pub async fn execute_batch(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: Actor,
    db_name: &str,
) -> Result<BatchResponse, BatchError> {
    let budget = std::time::Duration::from_secs(request.limits.max_execution_time_secs.max(1));
    match tokio::time::timeout(
        budget,
        execute_batch_impl(request, resolver, admin, invoker, actor, db_name, 0, &new_map()),
    )
    .await
    {
        Ok(result) => result,
        Err(_elapsed) => Err(BatchError::ExecutionTimedOut {
            budget_secs: request.limits.max_execution_time_secs,
        }),
    }
}
```

(`.max(1)` guards against a client-supplied `0` producing a
zero-duration timeout that fires before any work happens at all — decide
whether that's actually the RIGHT behavior for `0`, or whether `0` should
mean "no explicit budget beyond the default" — read `BatchLimits::default()`'s
`max_execution_time_secs: 30` and pick whichever interpretation is more
defensible; document your choice in a doc comment either way.)

Add a new `BatchError` variant in
`crates/shamir-query-types/src/batch/batch_error.rs`:

```rust
/// The batch's total execution time exceeded `BatchLimits.max_execution_time_secs`
/// (#666). The in-flight `execute_batch_impl` future was cancelled by
/// `tokio::time::timeout` — if the batch was transactional, its `TxContext`
/// is owned by that cancelled future and is dropped without commit (RAII
/// rollback, the SAME mechanism `execute_transactional_impl` already uses
/// for any other `Err` — no new rollback logic needed).
ExecutionTimedOut { budget_secs: u64 },
```

Plus its `Display` arm (mirror the existing style, e.g. `"batch execution
exceeded its {budget_secs}s time budget"`), and add the required match
arm in `crates/shamir-server/src/db_handler/handler.rs`'s `error_code()`
exhaustive match (the same mechanical update #663's fix needed for its
own new `BatchError` variant — check that diff/commit for the pattern;
classify it in whatever bucket makes sense, e.g. alongside
`TooManyIterations`/`TooManyQueries`, NOT the same bucket as
`InvalidWhenFilter`/`InvalidCondCondition` since this is a runtime
resource-limit error, not a validation error).

**Critical correctness point to verify yourself, not just assert**:
confirm that when `tokio::time::timeout` fires, the in-flight
`execute_batch_impl` future (and, transitively, any `TxContext` it owns
via `execute_transactional_impl`'s local `tx` variable) is genuinely
DROPPED (not leaked, not left in some half-alive state) — `tokio::time::timeout`
drops the wrapped future on timeout, and Rust's normal drop-on-scope-exit
semantics apply to whatever local state that future's stack held,
including an owned `TxContext` — so this should give correct RAII
rollback "for free", identical in spirit to how #661's fix relies on
`execute_transactional_impl`'s existing drop-without-commit semantics.
Write a test that PROVES this (see Tests below) rather than taking it on
faith.

### Fix 3 — one-line doc note on `execute_in_open_tx` (no code change)

Add a doc comment to `execute_in_open_tx`
(`crates/shamir-engine/src/query/batch/interactive_tx.rs`) noting that
`max_execution_time_secs` wall-clock enforcement (#666) applies only to
the single-call `execute_batch` entry point — an interactive tx spanning
multiple `execute_in_open_tx` calls before commit/rollback is a different
lifecycle contract and is explicitly out of scope for this fix (a
per-session idle/total-duration timeout for interactive transactions, if
ever needed, is a separate, larger feature).

## Tests

Add to `crates/shamir-engine/src/query/batch/tests/` (find or create a
sensible home — check for an existing `batch_limits_tests.rs`/similar,
otherwise a new `dos_gate_tests.rs` following the repo's test-org
convention: a `tests/mod.rs` manifest entry, no inline `#[cfg(test)] mod
tests`):

1. **`for_each_max_iterations_clamped_to_absolute_ceiling_even_when_client_requests_more`**:
   a `ForEach` (dynamic `over` — e.g. a `$query`-column-ref against a
   seeded table with more rows than a SMALL test ceiling) whose body's
   `limits.max_iterations` is set to something absurd (e.g. `usize::MAX`)
   — assert the runtime gate STILL rejects with `TooManyIterations` once
   the actual element count exceeds `ABSOLUTE_MAX_FOR_EACH_ITERATIONS`.
   Since `100_000` is large, either temporarily use a SMALLER test-only
   ceiling by making the constant `pub(crate)` and testing directly
   against it (seed exactly `ABSOLUTE_MAX_FOR_EACH_ITERATIONS + 1` rows —
   expensive but doable in-memory) OR — PREFERRED, cheaper — write the
   test to directly call whatever internal clamping logic you factor out
   (e.g. if you extract `effective_max_iterations(limits) -> usize` as a
   small testable helper function instead of inlining the `.min(..)` call,
   unit-test THAT function directly with a small handful of
   `(client_value, expected_effective_value)` cases including one that
   exceeds `ABSOLUTE_MAX_FOR_EACH_ITERATIONS`). Prefer the helper-function
   approach — it's cheaper to test and just as decisive; use your
   judgment on whichever reads cleaner in the final diff.
2. **`for_each_max_iterations_below_ceiling_is_respected_unchanged`**: a
   `ForEach` whose body's `limits.max_iterations` is small (e.g. `2`) and
   `over` resolves to more elements (e.g. `3`) — assert
   `BatchError::TooManyIterations { max: 2, .. }` (NOT the absolute
   ceiling) — proving the clamp is a `min`, not a replacement, and doesn't
   regress the existing #653 behavior (`for_each_max_iterations_exceeded_errors_before_first_iteration`
   in `for_each_tests.rs` already covers a similar shape for the
   STATIC/literal-array case at PLAN time — this test is the RUNTIME/
   dynamic-`over` counterpart; don't duplicate, complement).
3. **`execute_batch_exceeding_time_budget_returns_execution_timed_out`**: a
   batch whose `limits.max_execution_time_secs` is set to something the
   test can reliably blow past QUICKLY (do not sleep for real seconds in
   a unit test — instead, either (a) if `max_execution_time_secs` accepts
   sub-second granularity conceptually you could special-case a test
   constant, or (b) more robustly, construct a batch/resolver that takes
   observably-real (but still fast, e.g. tens of milliseconds) time via a
   deliberately slow mock `TableResolver`/`AdminExecutor` implementation
   whose `resolve`/`execute_admin` sleeps briefly before returning, paired
   with `request.limits.max_execution_time_secs` effectively meaning "a
   tiny fraction of a real second" — since the field is typed `u64`
   seconds, you likely need the mock's artificial delay to be at least
   ~1-2 real seconds with `max_execution_time_secs: 1`, OR (PREFERRED,
   faster tests) factor the timeout duration computation into a small
   testable function (e.g. `fn execution_budget(limits: &BatchLimits) ->
   Duration`) and separately unit-test `tokio::time::timeout`'s interaction
   with a artificially-slow future using `tokio::time::pause()` /
   `tokio::time::advance()` (Tokio's test-time control, needs the `test-util`
   feature — check if `shamir-engine`'s dev-dependencies already enable it;
   if not and adding it is disproportionate, fall back to a real (small,
   e.g. 100-300ms) sleep-based mock — use your judgment for whichever is
   cleaner and faster in CI). Assert the result is
   `Err(BatchError::ExecutionTimedOut { .. })`.
4. **`execute_batch_within_time_budget_succeeds_unaffected`**: the SAME
   harness as test 3 but with a generous budget (or no artificial delay)
   — assert the batch succeeds normally, proving the timeout wrapper adds
   no overhead/false-positive for the common case.
5. **Decisive rollback-on-timeout test**:
   `execute_batch_timeout_during_transactional_batch_rolls_back_partial_writes`
   — a TRANSACTIONAL batch (mirrors #661's own discriminating-test
   technique: a real write that succeeds, then a deliberately slow
   step/op that blows the time budget) — assert (a) the call returns
   `Err(BatchError::ExecutionTimedOut { .. })`, and (b) a FRESH read
   afterward shows the earlier write did NOT survive — proving the
   timeout's future-drop genuinely triggers the transaction's existing
   RAII rollback, exactly like #661's own tests prove for a plain `Err`.
   This is the single most important test in this task — walk through,
   in your summary, why you're confident the timeout-drop path and the
   error-return path give IDENTICAL rollback behavior (both end in the
   `TxContext` being dropped without a `commit()` call ever having run).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green,
  including all new tests.
- `cargo fmt -p shamir-engine -p shamir-query-types -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Explicitly confirm, with reasoning (not just assertion), that test 5's
  rollback assertion would have failed before your fix (there was no
  timeout at all before, so the slow op would just run to completion —
  describe what WOULD have happened without the fix, to show the test is
  decisive).

## Out of scope

- Do NOT attempt to fix finding 3 (cross-level cumulative `max_queries`
  budget) — this is documented above as an accepted, deferred risk. If
  you want to leave a breadcrumb, a short code comment near
  `BatchPlanner::plan`'s `max_queries` check noting the composition gap
  and pointing at this brief's finding 3 discussion is welcome, but do
  NOT implement a global counter/budget-threading fix — that is a
  separate, larger task.
- Do NOT add timeout enforcement to `execute_in_open_tx`/interactive
  transactions — see Fix 3 (doc-only).
- Do NOT touch #667 — separate task.
- Do NOT change `BatchLimits::default()`'s actual default values.
