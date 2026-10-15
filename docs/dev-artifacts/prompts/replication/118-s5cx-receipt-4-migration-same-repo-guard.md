# 118 / s5cx-receipt-4 — migration same-repo guard fix

Worktree: `worktrees/s5cx-follower-receipt-2` (dirty)
File: `crates/shamir-db/src/shamir_db/execute/admin_migration.rs`

## Problem

`handle_commit_migration` acquires src then dst `repo_ddl_guard` sequentially
(~:380/:382). If `cutover_state.dst_repo == cutover_state.src_repo`, the two
acquisitions hit the same non-reentrant tokio Mutex → self-deadlock.
`handle_start_migration` had no pre-check rejecting `op.dst_repo == op.repo`.

## Fix

1. `handle_start_migration`: early guard with other pre-checks (after the
   experimental-feature gate, before authorize/repo creation):
   `op.dst_repo == op.repo` → `err_code("migration_same_repo", ...)`,
   message "migration destination repo must differ from the source repo".
2. `handle_commit_migration`: defensive early check before the ddl guard
   acquisitions (covers persisted-state recovery paths); same err_code shape.

No git writes, no builds/tests (per instruction).

## Locations after edit

- start guard: crates/shamir-db/src/shamir_db/execute/admin_migration.rs:69-75
- commit guard: same file, immediately before the src/dst `repo_ddl_guard`
  acquisitions (after `let cutover_state = coord.state().await;`).
