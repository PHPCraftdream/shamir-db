# Round 12 — recursion-gate boxing: move the quarantine gate OUT of the async state machines

## Problem

Windows full-tier stack overflows in
`shamir-server db_handler::tests::repo_audit::{nested_batch_alter_is_audited, foreach_execution_audits_each_alter_iteration}`;
ubuntu green, master green on both. The tests recurse through nested
`Batch`/`ForEach` (`run_nested_body_in_outer_tx`, `query_runner.rs:198-233`)
under `execute_with_policy_and_repo_observer` (single-batch path) and
`tx_execute_with_policy_and_budget_and_repo_observer` (interactive path).

## Root mechanism (file evidence)

The PR inlined a synchronous quarantine gate INTO the body of two async fns:

- `crates/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:76-94` —
  skip-seam check + `resync_quarantine.is_empty()` + inline
  `quarantined_read_hit` tree walk (`BatchTreeVisitor` loop, lines 139-154) +
  inline `BatchError::ResyncRequired` construction.
- `crates/shamir-db/src/shamir_db/execute/db_tx.rs:267-284` (pre-change) —
  seam + call to `tx_read_quarantine_refusal_for_request`.

Even code with no `.await`, once inlined into an async fn, becomes part of
that fn's generated `Future` state machine; under nested Batch/ForEach
recursion the future is held on the stack once per recursion level.
`git show origin/master:` of both files confirms neither gate block exists on
master. `git show origin/master:crates/shamir-query-types/src/batch/batch_error.rs`
has zero `Resync` occurrences — the variant is PR-added, already `Box`-ed
(`Box<ResyncRequiredDetail>`), fine as-is.

## Change

1. `db_tx.rs` — `tx_read_quarantine_refusal_for_request` is now the SINGLE
   shared implementation: made `pub(crate)`, and fully self-contained — the
   test-only skip seam (`skip_read_quarantine_for_test()`) moved inside it
   alongside the existing `is_empty` fast path. Doc comment records the
   stack-safety contract: keep this helper sync, never inline it.
2. `db_execute.rs` — the 19-line inline gate replaced by one
   `if let Some(err) = tx_read_quarantine_refusal_for_request(self, db_name, request)`
   call plus a why-comment. Now-unused imports
   (`ResyncRequiredDetail`, `skip_read_quarantine_for_test`) pruned;
   `quarantined_read_hit` (sync, non-async) stays in `db_execute.rs`, still
   called only from the helper's own plain stack frame.
3. `db_tx.rs` `tx_execute_*` call site — collapsed to the same one-line
   `if let`; seam check no longer duplicated at the call site.
   `tx_read_quarantine_refusal` (per-repo variant) untouched, already sync.

## Why this removes growth from the recursive path

The async fns' bodies now contain only a transient
`Option<BatchError>` (already-constructed, dropped before `execute_batch` /
`execute_in_open_tx`). The visitor/tree-walk locals can no longer become
fields of the recursive futures. No `.await` was added or removed on this
path vs master; `query_runner.rs` is untouched and byte-identical to master
(`git diff origin/master...HEAD -- .../query_runner.rs` is empty).

## Semantics preserved exactly

skip seam, `is_empty` fast path (one `Relaxed` load), whole-tree walk,
`Box<ResyncRequiredDetail>`, authorization-before-gate ordering. No test
edits, no dependency changes. No compile/test claims (builds banned by brief).
