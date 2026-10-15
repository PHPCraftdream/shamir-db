# 118 — Round 14: nested-batch stack overflow diagnostic + boxing fix

Symptom: `shamir-server` lib test `nested_batch_alter_is_audited`
(`crates/shamir-server/src/db_handler/tests/receipts/repo_audit/tests.rs:189`)
stack-overflows on Windows lib and SIGABRTs on ubuntu lib; master is green.
Hypotheses already disproven: (1) `#[inline(never)]` sync quarantine gate
helper — still overflows; (2) error-enum growth — the recursion futures are
boxed on both sides. Both boxings kept.

## Call chain (file:line)

```
send() (tests.rs:104)
→ ShamirDbHandler::handle            handler.rs:358   (boxed closure → DbResponse)
→ ShamirDbHandler::execute           handler.rs:533   (unboxed, pub(super))
→ ShamirDb::execute_as               db_execute.rs:40
→ execute_with_policy_and_repo_observer  db_execute.rs:64 (unboxed; quarantine gate = sync helper call)
→ engine execute_batch               batch_execute.rs:93
→ execute_batch_impl (Box::pin per nested Batch level)  batch_execute.rs:288
→ execute_admin (async_trait = boxed at trait boundary)  admin_dispatch.rs:20
→ handle_alter_repo                  admin_repo_durability.rs:59
→ get_table (PR 2-line quarantine gate)  table_management.rs:373
→ observer
```

## (1) Diagnostic tests

`#[cfg(test)] mod zz_stack_diag` appended to
`crates/shamir-server/src/db_handler/tests/receipts/repo_audit/tests.rs`
(runs, not `#[ignore]`; all output prefixed `zz_stack_diag`, asserts nothing).

- `zz_stack_diag_execute_as` — measures concrete (unboxed) future sizes of
  `ShamirDb::execute_as` and `execute_with_policy_and_repo_observer` via
  `zz_size_of_future` (generic `size_of::<F>()`, no polling); prints
  `size_of` of `BatchError` / `DbError` / `QueryResult` / `BatchResponse`.
- `zz_stack_diag_execute_batch` — engine `execute_batch` measured through a
  thin unboxed wrapper `zz_measure_execute_batch` with
  `Authorized::unchecked` + a stub `ZzResolver` (never polled).
- `zz_stack_diag_alter_repo` — `ShamirDb::get_table` future size; notes
  `execute_admin` is boxed (Pin<Box> = 16).
- `zz_stack_diag_dispatch` — `ShamirDbHandler::execute` future size inside
  the borrowing scope (fixture from the file's own `fixture()` helper).

### Expected numbers (master → this PR)

| future | master (expected) | PR (expected) |
|---|---|---|
| `execute_with_policy_and_repo_observer` | base | identical or +8–16 B (gate adds one `Option<BatchError>` temp; helper is external) |
| `get_table` | small (few dozen B) | modest growth (`Option<String>` reason / `DbError::ResyncRequired` temp) |
| `execute_batch` wrapper | unchanged (byte-identical file) | unchanged |
| `BatchError` | context value | context value |
| `shamir_db::DbError` | context value | context value |

`execute_admin` boxed on both sides ⇒ 16 either way; `handler.handle` boxed
⇒ 16; the unboxed `handler.execute` future is the interesting dispatcher
measure.

## (2) Fixes (both applied, user-mandated)

a. `crates/shamir-db/src/shamir_db/execute/admin_replication.rs` —
   `Box::pin` on the newly-heavy inline awaits:
   - `Box::pin(self.shamir.refresh_resync_quarantine_cache())` in
     `handle_drop_subscription` (~:396) and `handle_alter_subscription`
     (~:592),
   - `Box::pin(self.revoke_quarantined_receivers())` in
     `mark_subscription_terminal` (~:907),
   - `Box::pin(self.refresh_resync_quarantine_cache())` in
     `mark_subscription_terminal` (~:952).
   Comment lines condensed in the same functions; file is **998 ≤ 1000**
   lines (was exactly 1000; must not grow).

b. `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs` — the 9
   replication arms (CreateReplicationProfile … ReplicationStatus) removed
   from `execute_admin`'s match and routed through a match-guard arm
   `op if matches!(…)` → `Self::dispatch_repl(self, op).await`, where
   `dispatch_repl` is a `#[inline(never)]` fn returning
   `Pin<Box<dyn Future<Output = Result<QueryResult, BatchError>> + Send +
   '_>>` containing those arms (each `.await` inside its own boxed body).
   Keeps the dispatcher's future free of the replication arms' inline state.

## (3) Non-touch verification

- `crates/shamir-engine/src/query/batch/batch_execute.rs` — untouched
  (byte-identical to master; 959 lines).
- `crates/shamir-db/src/shamir_db/execute/repo_admin/admin_repo_durability.rs`
  — untouched (285 lines, identical to master).
- `git status --porcelain` shows exactly 3 modified files:
  admin_dispatch.rs, admin_replication.rs, repo_audit/tests.rs.
- No builds/tests/clippy/fmt run (parent runs them); no git writes.
