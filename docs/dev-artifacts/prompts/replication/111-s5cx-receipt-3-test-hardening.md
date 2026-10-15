You are a Rust test-hardening worker in worktree D:/dev/rust/shamir-db/worktrees/s5cx-follower-receipt-2 (branch feat/s5cx-follower-receipt-2, DIRTY tree — round-3 phase-A changes from other workers are present; do not touch them). Task: S5c-X PR6b-2 round 3, review findings 7/8/9 — cancellation-test realism, crash/resume test semantics, oracle fixes.

⛔ NEVER run git reset / checkout / clean / stash / restore / rm, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
Do NOT run cargo/rustc/nextest or any build or test. Only the orchestrator runs `cargo fmt` at the very end.
Read and follow C:/Users/Computer/.agents/skills/rust-intel/SKILL.md if readable; CLAUDE.md conventions (imports at top, deterministic tests — waits on conditions/notifies, no wall-clock sleeps except the sanctioned bounded fjall reopen retry).
FIRST: save this brief verbatim to docs/dev-artifacts/prompts/replication/111-s5cx-receipt-3-test-hardening.md.

READ FIRST (real definitions): crates/shamir-storage/src/storage_fjall.rs (FjallAtomicBatch::commit, exec_atomic_commit, the existing cfg(test) `atomic_batch_test_seam` FAIL_AFTER_COMMIT pattern ~line 246); crates/shamir-engine/src/tx/follower_durability/receipt/checkpoint.rs (seams ~25-125, checkpoint() ~217-304 with the NEW signature including publication — phase-A worker changed it); crates/shamir-engine/src/tx/follower_durability/receipt/tests/{cancellation_tests.rs, checkpoint_uncertain_tests.rs, crash_subprocess_tests.rs}; crates/shamir-engine/src/tx/follower_durability/test_seam.rs (new fail_after fields from phase A); crates/shamir-engine/src/tx/follower_durability/receipt/{apply.rs,resume/worker.rs} (READ ONLY — phase-A repo-owned worker: submit/try_send, parked_guards; do not edit).

FILE SCOPE (everything else is off-limits):
- crates/shamir-storage/src/storage_fjall.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/checkpoint.rs (ONLY new cfg(test) seam(s))
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/cancellation_tests.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/checkpoint_uncertain_tests.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/crash_subprocess_tests.rs
- (only if needed) the seam module file where `atomic_batch_test_seam` lives (grep for it; it is cfg(test) infra in shamir-storage — if it is a separate file, extend it there; expose new fns with `pub` + `#[cfg(test)]`-visible naming consistent with the existing pattern).

REQUIRED:

1. TRUE commit-window park (finding 7a): add a cfg(test) seam in the fjall atomic-commit path that parks DURING `exec_atomic_commit` — before/around the physical `batch.commit()` call. This code runs inside `spawn_blocking` (storage_fjall.rs:292-297), so use a blocking primitive (std::sync::Condvar + Mutex, or the same style as the existing atomic_batch_test_seam) — NOT a tokio Notify. API: `arm_commit_window_park(bool)` / `wait_commit_window_park_reached()` / `release_commit_window_park()` mirroring the existing checkpoint seam naming, exported so the engine tests can call them. Note: this seam must be inert when not armed (one atomic load on the blocking path, zero cost unarmed; document the contention model in a comment).

2. Rewrite `drop_during_checkpoint_commit_still_completes_and_publishes` (cancellation_tests.rs ~355-385) to use the NEW commit-window seam instead of the pre-stage park: arm the seam, spawn the caller, wait until the worker is parked INSIDE the physical commit, `caller.abort()`, release, then assert full convergence INCLUDING publication: `gate.last_committed() == 2`, `gate.durable_watermark() == 2`, one history version per key, receipt retired. JOIN the aborted caller's handle afterwards (`let _ = caller.await;`) so the aborted task is reaped.

3. `spin_until_settled` (cancellation_tests.rs ~332-347): make the condition publication-aware — require `gate.last_committed() == 2 && gate.durable_watermark() == 2` in ADDITION to the durable bookmark/receipt conditions (those can land before publication). Raise the bound from 500 to a still-bounded loop (e.g. 2000 yields) and keep the panic on exhaustion. Also join the aborted caller in `drop_during_uncertain_readback_still_completes_and_publishes` (~line 409).

4. Commit-park tests must serialize: take `checkpoint::checkpoint_test_lock()` in the tests that arm global checkpoint/commit seams (the rewritten commit-window test currently does not). Verify the other tests that read `checkpoint_reconciles()` take the lock (they do) and that the commit-window seam tests cannot interleave with them.

5. TRUE reconciliation negative control (finding 9d): add a cfg(test) seam in checkpoint.rs — e.g. `disable_uncertain_reconcile(bool)` — that makes the Uncertain branch treat the outcome as settled WITHOUT performing the read-back or retry (the protection removed, the branch still entered so the counter still increments). Rewrite/extend `checkpoint_uncertain_tests.rs`: with the seam ON, the settled-state assertion (`assert_settled`, which checks bookmark/clean_through/receipt retirement) FAILS or the state is provably not reconciled — proving the tests discriminate real read-back protection, not just branch entry. Keep the existing healthy-path counter test. Document the seam as the negative-control hook in its comment.

6. Crash-harness oracles (finding 9a/9b):
   a. `Scenario::expected_counter` (crash_subprocess_tests.rs ~195-199) counts Put events — wrong for repeated-key (expects 2, true live-row count is 1) and Put+Delete (expects 1, true is 0). Fix to the actual absolute row count of DISTINCT keys present after the batch: tombstone → 0, repeated-key two puts → 1, single put → 1. Cross-check against `assert_converged`'s use and the receipt path's absolute counter semantics (history rows per key: latest op wins for existence).
   b. `assert_child_died` (~335-344) accepts ANY failure incl. the fallback panic `unreachable!("abort must preempt the apply; it returned {result:?}")` at ~line 302. Strengthen: assert the output does NOT contain `abort must preempt` (proves the abort hook fired inside the pipeline, not that the apply returned) in addition to the existing checks.
7. Keep every touched file within the 1000-line / 7-entry ratchets (check with a quick count before finishing; crash_subprocess_tests.rs is ~510 lines, cancellation_tests ~427 — no risk unless you add large blocks).

FINAL MESSAGE: per item: what changed with file:line, the seam API you added and where it lives, how the negative control fails when protection is removed, the new counter oracle values per scenario, and anything you could not verify without a build.
