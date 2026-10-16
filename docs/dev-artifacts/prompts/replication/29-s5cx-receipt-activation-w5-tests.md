# 29 — S5c-X PR6b-2c receipt activation, W5: receipt tests (findings 2, 4, 5)

Parent: `docs/dev-artifacts/prompts/remediation/119-s5cx-receipt-activation.md`. W1–W4 landed in the
working tree. The pipeline lives at
`crates/shamir-engine/src/tx/follower_durability/receipt/**`; its seams in
`crates/shamir-engine/src/tx/follower_durability/test_seam.rs`; the worker accessors used by the old
tests exist under the SAME names (`repo.receipt_worker_parked_guards() -> Option<usize>`,
`repo.receipt_worker_wait_idle()`). Read CLAUDE.md. Hard rules: ⛔ no git-mutating commands (read-only
git only); NO cargo/build/test/clippy; FILE SCOPE only; receipt/ lands at EXACTLY 7 entries with this
task's `tests/`; ≤ 1000 lines per file; imports at top; tests `#[cfg(test)]`-gated; deterministic —
NO sleeps, NO fixed yield budgets (finding 5); rustfmt-clean; verify every symbol against the tree.

## FILE SCOPE (all NEW unless noted)

- crates/shamir-engine/src/tx/follower_durability/receipt/tests/mod.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/receipt_apply_tests.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/checkpoint_uncertain_tests.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/cancellation_tests.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/crash_subprocess_tests.rs
- crates/shamir-engine/src/tx/follower_durability/receipt/tests/ddl_interlock_tests.rs  (REWRITE, not
  a port — the old file tests the REMOVED DdlGuard mechanism)

Sources: `git show feat/s5cx-follower-receipt-2:crates/shamir-engine/src/tx/follower_durability/receipt/tests/<file>`.
Port, then ADAPT to what actually shipped: admission is now the write-domain permit (no
`ddl_admission_lock`/`DdlGuard`/`ddl_hook` anywhere); the worker is the lifecycle foundation (no
`ReceiptWorker` type; lease-based park/retire); the Materialized fast path goes straight to checkpoint;
identity is the four-part `verify_identity`; failed 3-strike applies place a PAUSED RESERVATION
(`repo.write_domain().has_outstanding_reservation()` is true, ordinary `admit()` returns
`RepoWriteError::Reserved`, the next apply resumes through `admit_with_reservation`). Any old-branch
test asserting the absence of these semantics is updated to the NEW behavior and listed as a deviation.

## Work items

1. **mod.rs** — manifest only, port the old one.
2. **receipt_apply_tests.rs** — port the old file (helpers: `open`/`close`/`repo_name`/`journal`/
   `history_row`/`history_versions`/`persisted_count`/`outstanding_state`/`crashed_prepared`/
   `key_bytes`), adapting to the current codecs (receipt tuple version 2, `ops_per_event` in any
   hand-built fixture — prefer `encode_receipt` over hand-rolled bytes) and to fjall reopen-retry
   patterns already used in this repo. Then ADD the finding-2 test:
   `materialized_pause_hits_injected_failure_replays_recorded_counters`:
   - Arm `test_seam` with a STICKY `fail_after(ReceiptMaterialized)`; apply a two-key batch.
   - Assert the pipeline RETURNS the injected `DbError::Storage` (the failure was actually HIT, not
     silently swallowed) after the 3-strike park.
   - Assert the PAUSED state: `receipt_worker_parked_guards() > 0`;
     `repo.write_domain().has_outstanding_reservation()`; `outstanding_state` = phase Materialized;
     `!gate.requires_reopen()` (an ordinary pause is NOT a quarantine); spawn a task calling
     `repo.write_domain().admit()` and assert it resolves to `RepoWriteError::Reserved`.
   - Counter oracle that DISCRIMINATES: before resuming, perturb the table's LIVE counter to a value
     equal to neither the recorded `before` nor `after` (find the doctor's absolute-set method on
     `RecordCounter` in the current tree). Clear the sticky seam; apply again (the resume path runs
     `admit_with_reservation`). Assert: final live counter == the receipt's recorded `after` (NOT the
     perturbed value — a recomputation from the perturbed cache would surface it), persisted count row
     == after, history versions unchanged (no extra versions), journal bytes byte-identical, v3
     bookmark advanced + receipt retired + `has_outstanding_reservation()` cleared +
     `receipt_worker_parked_guards() == Some(0)`.
3. **checkpoint_uncertain_tests.rs** — port essentially as-is (SIMULATE_CHECKPOINT_UNCERTAIN,
   UNCERTAIN_WITHOUT_COMMIT, DISABLE_UNCERTAIN_RECONCILE, CHECKPOINT_RECONCILES negative controls are
   all landed in checkpoint.rs). Adapt only symbol paths.
4. **cancellation_tests.rs** — port, then apply finding 5: REMOVE the remaining bounded yield loop
   (old lines ~332-355 `for _ in 0..2000u32 { yield_now().await; }` waiting for the resumed worker):
   replace with `repo.receipt_worker_wait_idle().await` (arm-then-check Notify on the lease). Audit the
   whole file for any other fixed yield budget or sleep and replace with the park seams, the
   `commit_window_park_seam::wait_reached()` Notify wait, or `wait_idle`. Keep
   `drop_at_receipt_prepared...` / `drop_at_receipt_materialized_converges_on_reapply`, the
   commit-park and reconcile-park drop tests, and their assertions that the detached worker completes
   from the durable pause point.
5. **crash_subprocess_tests.rs** — port. The parent MUST require the child's
   `SHAMIR_RECEIPT_ABORT_FIRED {phase:?}` stderr marker for the exact crash point (finding 4 —
   present in the old branch; keep it), spawn via `std::env::current_exe()`, env-select the scenario,
   kill WITHOUT graceful shutdown at each persistence/publication boundary, and reopen TWICE + replay
   TWICE asserting exact allocation, history keys/timestamps, journal bytes, counters, tombstones and
   snapshot reads. Keep the NEGATIVE CONTROL (`SHAMIR_RECEIPT_CRASH_LOOKUP_DISABLED=1`) contrasting the
   legacy path. Adapt seam paths (`shamir_storage::storage_fjall::commit_window_park_seam` still works
   via re-export) and the receipt codec (version 2).
6. **ddl_interlock_tests.rs** — REWRITE against the write domain (the old file tested the removed
   DdlGuard):
   a. `parked_apply_blocks_ddl_via_the_write_domain`: park the pipeline at a seam phase
      (`park_at`), spawn `repo.rename_table_stores(..)`, install
      `repo.write_domain().install_contended_admission_for_test(Arc::new(Notify))` and await the
      contended notification (deterministic proof the DDL is blocked on the domain HELD BY THE
      PIPELINE — no second mutex), release the park, assert the DDL completes.
   b. `paused_reservation_rejects_ordinary_admission_resume_uses_it`: sticky `fail_after` 3-strike
      park → `has_outstanding_reservation()` + ordinary `admit()` → Reserved; the follow-up apply
      succeeds via the reservation path and clears the reservation.
   No sleeps, no yield budgets, no DdlGuard.
## Acceptance criteria

- receipt/ = exactly 7 entries (mod, stage, apply, checkpoint, publish, resume/, tests/). Each file
  ≤ 1000 lines. `grep -rn yield_now\|sleep` over the new tests: zero hits except `yield` inside
  comments (report the grep).
- Final message: files + line counts, per-test: what it proves and which review finding it realizes
  (file:line), every adaptation vs the old branch, anything unverified.
