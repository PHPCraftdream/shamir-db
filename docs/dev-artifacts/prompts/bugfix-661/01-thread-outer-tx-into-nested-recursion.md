# #661 — thread the outer transaction into `Batch`/`ForEach` body recursion

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Decision (already made by the user — implement, do not re-litigate)

Fix the real atomicity gap. Do NOT just document the current (broken)
behavior — the user explicitly chose "fix it" over "document it as a
known limitation."

## The bug — root-caused precisely, and it affects BOTH `Batch` and `ForEach`

`crates/shamir-engine/src/query/batch/query_runner.rs`'s two recursive
seams — the `BatchOp::Batch(sub)` arm (~line 227-300) and the
`BatchOp::ForEach(fe)` arm (~line 332-458) — both recurse via the free
function `execute_batch_impl` (`crates/shamir-engine/src/query/batch/batch_execute.rs:62`),
which takes NO `tx` parameter at all. `execute_batch_impl` always
independently decides transactional-vs-not based purely on the INNER
body's own `request.transactional` flag — it can never participate in an
ALREADY-OPEN outer transaction. Both arms additionally guard against
`body.transactional && self.tx.is_some()` with a `"nested_tx_not_supported"`
error — so the ONLY combination that's actually reachable today (outer
transactional batch + inner body NOT itself marked transactional) silently
runs the body's writes via `execute_batch_impl`'s non-transactional path —
each write commits independently (an implicit per-op transaction), with
ZERO connection to the outer `TxContext`.

Consequence: contrary to the Epic04 ADR (`docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`
Decision 4) and the guide docs (`docs/guide-docs/guide/01-queries.md`),
"a `ForEach`/sub-batch iteration failure aborts the WHOLE containing
transactional batch, no partial writes survive" is NOT actually true —
already-succeeded iterations' writes are durably committed independently
and CANNOT be undone by the outer transaction's abort. **This bug is
identical for plain `BatchOp::Batch` sub-batches, not just `ForEach`** —
fix BOTH arms consistently; a ForEach-only fix would leave `Batch` with the
exact same silent gap.

## Why the fix is straightforward — the right machinery already exists

`crates/shamir-engine/src/query/batch/batch_execute.rs:367`,
`execute_plan_tx_impl(plan, queries, resolver, admin, invoker, actor,
db_name, tx: &mut shamir_tx::TxContext, depth, params, result_encoding)`
executes a plan's stages directly against an ALREADY-OPEN `TxContext`,
constructing `QueryRunner { tx: Some(&mut *tx), ... }` per stage exactly
like the top-level transactional path does
(`execute_transactional_impl`, `batch_execute.rs:424-499`, which calls
`execute_plan_tx_impl` then commits or — critically — drops `tx` without
committing on ANY `Err` from the plan, i.e. RAII rollback,
`batch_execute.rs:505-516`). If the nested body's writes flow through this
SAME `tx` object (instead of opening/committing their own independent
implicit transactions), an error from ANY iteration/sub-batch naturally
aborts the WHOLE outer transaction for free — no changes needed to
`execute_transactional_impl`'s own error handling.

## The fix

In BOTH the `BatchOp::Batch(sub)` arm and the `BatchOp::ForEach(fe)` arm of
`QueryRunner::run` (`query_runner.rs`):

1. **When `self.tx.is_some()`** (we are already inside an open outer
   transaction) **and the body itself is NOT marked transactional**
   (`sub.batch.transactional == false` / `fe.batch.transactional ==
   false` — the only reachable case today per the existing guard):
   instead of calling `execute_batch_impl`, do the SAME setup steps
   `execute_batch_impl` does internally (read it in full — plan via
   `BatchPlanner::plan`, `validate_tables`, `validate_filter_depth`) and
   then call `execute_plan_tx_impl` directly, reusing the outer
   transaction: `self.tx.as_deref_mut()` (a standard reborrow — `self.tx:
   Option<&'a mut TxContext>`, `as_deref_mut()` gives a shorter-lived
   `Option<&mut TxContext>` you can pass through). Wrap the resulting
   `TMap<String, QueryResult>` into the SAME `QueryResult`/`BatchResponse`-
   shaped value the current code builds from `inner_response.results`
   (read the existing wrapping code carefully and preserve its exact
   shape — do not change the wire format of what a sub-batch/for_each
   alias's value looks like to the OUTER batch, only how the WRITES are
   plumbed).
2. **When `self.tx.is_none()`** (no outer transaction — the top-level
   non-transactional case, or a nested call where somehow no tx is open):
   keep calling `execute_batch_impl` EXACTLY as today — zero behavior
   change for this path. This is the majority of existing test coverage
   and must stay byte-identical.
3. **The existing `nested_tx_not_supported` guard stays** for the
   `body.transactional == true && self.tx.is_some()` case — that
   combination remains genuinely unsupported (a body that ALSO wants its
   own independent transaction while being embedded in one already is a
   different, harder problem — two-phase commit across a shared
   `TxContext` — explicitly out of scope here).
4. **Check depth/params/actor threading carefully** — `execute_plan_tx_impl`
   takes `depth`, `params`, `actor`, `db_name` similarly to
   `execute_batch_impl`; make sure `self.depth + 1`, the resolved
   params map (built the same way the current code already builds it for
   the `execute_batch_impl` call), `self.actor.clone()`, and `self.db_name`
   are threaded through identically to how the existing code already
   assembles them for `execute_batch_impl` — this part of the existing
   code should mostly transplant unchanged.
5. **`ForEach` specifically**: this per-iteration recursion happens INSIDE
   the `for element in elements` loop (`query_runner.rs:429-458`) — each
   iteration must reuse the SAME outer `tx` (not open a new one per
   iteration). Since `execute_plan_tx_impl` takes `tx: &mut TxContext` by
   unique reference, you'll need to re-borrow it fresh each loop
   iteration (`self.tx.as_deref_mut()` inside the loop body, since a
   `&mut` reborrow can't be "reused" across iterations by value — this is
   standard Rust borrow-checker territory, not a design problem).

## Tests — the critical, previously-missing discriminating cases

Both new tests below MUST use a genuinely DATA-DEPENDENT failure (not a
table-existence/validation-time failure that fires before any iteration's
write happens) — read `for_each_iteration_error_aborts_whole_tx_batch`
(`crates/shamir-engine/src/query/batch/tests/executor_tests/...` or
wherever it lives) and `for_each_iteration_error_stops_at_first_in_non_tx_batch`
first; they show the "unique index to force a mid-loop failure" technique
already used elsewhere this session — reuse it.

1. **`for_each_partial_iterations_roll_back_on_later_failure`** (or
   similar name): a TRANSACTIONAL outer batch, `ForEach` over `[1, 1, 2]`
   (or similar) where iteration 0 succeeds (writes a row), iteration 1
   FAILS on a genuine data-dependent condition (e.g. a unique-index
   violation, same technique as the existing non-tx test), iteration 2
   never runs. Assert: after the batch call returns an error/aborted
   status, a FRESH read of the table shows ZERO rows from iteration 0 —
   proving the previously-committed-independently row is now genuinely
   rolled back. This is the test that would have caught #661 and must
   FAIL against the pre-fix code (verify this yourself: temporarily check
   out the pre-fix state mentally / reason about it — do not actually
   revert any code — and confirm the test's assertion is the one thing
   that was broken).
2. **`sub_batch_partial_writes_roll_back_on_later_top_level_failure`** (or
   similar): a TRANSACTIONAL outer batch with a plain `Batch(SubBatchOp)`
   whose body succeeds (writes a row), followed by a top-level sibling op
   that fails. Assert the sub-batch's row does NOT survive.
3. **A positive/happy-path test** confirming multi-iteration ForEach still
   COMMITS successfully and ALL rows are visible when every iteration
   succeeds (regression guard against the fix accidentally breaking the
   common case).
4. **Update/replace the existing (weaker) test** —
   `for_each_iteration_error_aborts_whole_tx_batch` currently makes every
   iteration fail at validation time before any write happens, so it never
   actually exercised the "roll back an already-succeeded iteration" case.
   Either strengthen it to also cover the data-dependent partial-failure
   case, or keep it as-is (it's still a valid "zero successful iterations"
   test) AND add the new discriminating tests above as the real coverage.
5. **e2e**: update `crates/shamir-client/tests/batch_for_each_e2e.rs`'s
   `for_each_iteration_error_mid_loop_rolls_back_whole_tx_over_real_wire`
   test if its current construction doesn't actually force a successful
   iteration 0 before the failure (read it — per the code review, it may
   currently fail at iteration 0 itself, making its "no partial rows"
   assertion pass for the wrong reason). Adjust the `over` values /
   failure trigger so iteration 0 provably succeeds before the failure,
   if it doesn't already.

## Docs — fix the misstatement, don't just leave it wrong

`docs/guide-docs/guide/01-queries.md`'s `for_each` section and
`docs/dev-artifacts/design/oql-05-while-loop-exploration.md`'s §4 both
currently describe the (pre-fix) mechanism incorrectly ("nested bodies
execute within the outer transaction" / "writes are visible because prior
iterations are committed" — the review found this describes independent
per-iteration commits, not real tx participation). Once the fix lands,
these descriptions become ACCURATE (nested bodies genuinely will execute
within the outer transaction) — update any wording that describes the
OLD, incorrect mechanism to match the NEW, correct one, but do not
overclaim beyond what the fix actually delivers.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green,
  including the new discriminating tests.
- `./scripts/test.sh -p shamir-client --full -- for_each` green (e2e).
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace — this exact class of fallout has bitten this session
  repeatedly).
- Report literal command output for all of the above, and explicitly
  confirm (in your own words, with reasoning) that the new discriminating
  tests genuinely would have failed before your fix — don't just assert
  this, walk through why.

## Out of scope

- Do NOT touch #663, #665, #666, #667 — separate tasks.
- Do NOT attempt to support `body.transactional == true` nested inside an
  outer transaction (two-phase commit) — that guard stays as an explicit
  rejection.
- Do NOT change `execute_transactional_impl`'s commit/abort logic itself —
  it already does the right thing once the nested body's writes flow
  through the same `tx`.
