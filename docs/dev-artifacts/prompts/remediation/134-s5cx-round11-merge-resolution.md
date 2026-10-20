# Round 11 — semantic merge-conflict resolution (PR #189, mid-merge worktree)

NOTE: the working tree is inside an uncommitted `git merge origin/master`. NEVER run any
git command that mutates the index, the merge state, or the working tree
(add/commit/merge/checkout/restore/stash/clean/rm). Read-only git inspection is allowed
(`git show origin/master:<path>`, `git diff`, `git log`, `git merge-base`).
The orchestrator commits the merge at the end.

## A. crates/shamir-db/src/shamir_db/execute/admin_replication.rs (4 conflict hunks)
Rules: master (#188) is authoritative for lease/generation/quarantine/pending-claim/replace
semantics; this branch is authoritative for receipts-only apply, FollowerStateRefused, stall
visibility and the repo-wide atomic-batch admission.
1. Hunks 1/2 (imports + `batch_err` helper): keep what the merged file really uses;
   master's `batch_err` is needed by master's code.
2. Hunk 3 (CREATE SUBSCRIPTION admission): KEEP BOTH blocks in a sensible order — first
   master's `ensure_subscription_absent` refusal (same-name overwrite), then this branch's
   profile-stream `validate_repo_wide_atomic_batch` loop (or the reverse if a data
   dependency requires it; explain the order in your report).
3. Hunk 4 is the whole "Follower-side gap visibility" `impl ShamirDb` block: this branch
   MOVED it into crates/shamir-db/src/shamir_db/execute/repo_admin/quarantine_helpers/
   admin_replication.rs while master modified those functions IN PLACE. Take master's final
   version of every function in that block (git show origin/master:crates/shamir-db/src/
   shamir_db/execute/admin_replication.rs, from 'Follower-side gap visibility' to the end:
   has_resync_required_subscription_covering, quarantined_read_reason, read_quarantine_reason,
   mark_subscription_resync_required, mark_subscription_resync_required_generation,
   mark_subscription_terminal, mark_subscription_terminal_generation incl. the lease quiesce,
   owner claims, row_set_fault_armed seam, replace_subscription_row) and make the MOVED file
   contain exactly that, applying this branch's receipts-only changes on top where the old
   text touched legacy bookmarks/activation. The resolved admin_replication.rs must NOT
   contain that block any more (it lives in the helper file). Compare the helper file against
   the merge-base version of the block (git merge-base HEAD origin/master) so no other master
   change in that block is lost, and re-check every other function of the helper module
   against master's current text for the same loss.
4. Keep admin_replication.rs <= 1000 lines AND every helper file <= 1000 lines (wc -l); if
   the merge pushes one over, move whole functions into a sibling file under
   repo_admin/quarantine_helpers/ (mind the 7-entry folder limit; the layout checker only
   sees TRACKED files, so count entries with ls).

## B. crates/shamir-server/src/replication/follower_loop.rs (4 hunks)
Master's FollowerLoopConfig has subscription: String (empty = unsupervised loop, no lease),
lease: Option<Arc<SubscriptionLease>>, #[cfg(test)] apply_park, #[cfg(test)] exit_hook, a
manual Debug impl; this branch has subscription: Option<String> for PR6b-3 stall visibility.
DECISION: adopt MASTER's field shapes and adapt this branch's stall visibility to String:
empty name never marks (same as old None), non-empty name marks. Fix every constructor/
consumer of subscription (supervisor.rs, follower-loop tests, receipts/stall_visibility_tests,
receipts/atomic_batch_gate_tests, loop_tests) and the Debug impl. Hunk 3 (destructuring):
keep master's lease, apply_park, exit_hook: _ bindings. Hunk 4: keep master's Round-4 lease
apply guard (_apply_guard held across the engine apply, supersession exit) and the apply_park
hook, DROP master's comment about legacy activation ('repositories with a stored legacy
v1/v2 bookmark keep the legacy path') since the legacy path no longer exists, keep this
branch's 'receipt pipeline is the ONLY apply path' comment. Re-read the whole apply_one/loop
body afterwards: the lease guard must wrap the receipt apply exactly as it wrapped the old
apply, no legacy branch may survive, and after_apply park hooks must still fire at the same
point.

## C. Non-conflicted but semantically coupled files (auto-merged, can silently break)
Check every file master touched in crates/shamir-server/src/replication/ (supervisor.rs,
lease/startup code, tests lease_gate_tests, multi_target_exit_tests, startup_lease_tests,
terminal_retry_tests) against this branch's removal of the legacy path: any master-added code
or test calling a removed legacy function/activation or asserting legacy-only behaviour must
be adapted to receipts-only (state each such change). terminal_retry_tests::
reopen_follower_with_lock_retry and the persistent follower fixtures must keep working with
this branch's fjall_raw catalogue kind.

## D. Gates (after your LAST edit, sequential, one cargo at a time, CARGO_BUILD_JOBS=4;
capture each command's own exit code: cmd > .scratch/r11_x.log 2>&1; echo exit=$?; report
every real inner exit code including failed attempts and fixes):
  cargo check -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets
  cargo clippy -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets -- -D warnings
  cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server
  cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server -- --check
  cargo check -p shamir-db -p shamir-server   (NO --all-targets: production must not call *_for_test/test-gated API)
  python -I scripts/ci/check_layout.py
Rerun an unchanged command ONCE after a 0xc0000409/allocation crash and say so.
Also: grep the whole worktree for '^<<<<<<<' / '^=======' / '^>>>>>>>' after your last edit
(must be zero hits in tracked sources), and wc -l every file you touched (<= 1000).

## Report
For A/B/C: what was merged and how (file:line); the list of master behaviours ported into the
moved helper functions; every legacy-dependent master test adapted; per-command exit codes;
explicit list of what you did NOT do. No git mutation, no tests/builds beyond the listed
gates, no #[allow], imports at top, no raw json!()/from_value for ops.
