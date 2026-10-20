# Round 12 — fix the round-11 review findings F1–F4 (lease/lock interactions)

Worktree is CLEAN at b9ae90ae (merge committed; the vestigial admin_handlers.rs stub was
removed; mod.rs declares only admin_replication). NEVER run git add/commit/merge/rm/xargs.
No #[allow]; imports at top; scratch under .scratch/ (r12_*.log); one cargo process at a
time, CARGO_BUILD_JOBS=4; capture every command's own exit code; stop editing at 100 minutes;
never end a turn with a background job outstanding.

## The findings (fix all four)
F1 (P1 deadlock): follower_loop.rs holds `_apply_guard` (apply_gate.read(), ~:792–917) while
awaiting clear_stall / note_swallowed_failure (~:828–851, ~:905–913); both call
mark_subscription_terminal → mark_subscription_terminal_generation which takes the
replication CONTROL lock then quiesces the lease (apply_gate.write()) — self-deadlock and
lock-order inversion of the documented order (repl_binding/lease.rs:14–18).
F2 (P1 self-invalidation): quiescing sets lease.valid=false; the loop exits at the top-of-
iteration check (~:476–483). A retryable status report with state "active" must NOT quiesce
or invalidate the lease.
F3 (P1 fencing): follower-originated writes that use the generation-less
mark_subscription_terminal(..., None) can mark/clear a REPLACEMENT binding's row. Every row
write the loop performs must be fenced by the generation of ITS lease/config (stale
generation = logged no-op).
F4 (P2 tests): injected legacy rows without valid binding_version/binding_generation/
binding_destinations are rejected by the merged catalogue reader
(SubscriptionBinding::from_row, terminal.rs read_subscriptions) BEFORE backend admission —
supervisor fixture tests no longer reach the path they test.

## Required design (minimal)
1) NEW non-terminal, non-quiescing, generation-fenced status writer on ShamirDb
   (repo_admin/quarantine_helpers/ — sibling file admin_status.rs if admin_replication.rs
   would pass 1000 lines; mind the 7-entry folder limit):
   `set_subscription_status_message(&self, name: &str, message: &str,
   expected_binding_generation: u64) -> DbResult<()>`.
   Takes the replication control lock (via lock_replication_control), reads the row;
   missing row or binding_generation != expected → Ok(()) as a logged no-op; otherwise
   rewrites ONLY status_message (clears it when message is empty) via
   replace_subscription_row + persist_table_interner + flush_seed_catalogue. Must NOT touch
   state/quarantined/resync_from_receipt/last_terminal_reason, the quarantine cache, or the
   lease; must NOT call quiesce_subscription_lease_locked.
2) note_swallowed_failure / clear_stall (seed/terminal.rs) use ONLY that writer, taking the
   loop's generation (lease.generation; unsupervised loop with empty subscription name does
   nothing, as now). They must NOT take the control lock themselves beyond what the writer
   does internally.
3) LOCK ORDER: the loop never awaits the control lock while holding _apply_guard.
   Restructure: record the stall/clear DECISION (or the guard-held failure paths) in local
   state while the guard is held; do the PERSISTENCE after drop(_apply_guard) at the end of
   the iteration. One-line code comment stating why. Re-read and classify EVERY
   note_swallowed_failure/clear_stall call site in follower_loop.rs (~:499, :530, :545,
   :583, :676, :730, :758, :763, :828, :843, :905): guard-held ones defer persistence to
   after the guard drop; guard-free ones (repo-missing, bookmark-error, pull-error,
   decode-error, empty-backlog arms — all `continue`/idle paths before the guard exists)
   may call directly.
4) Re-verify EVERY other caller of mark_subscription_terminal / mark_subscription_resync_required
   in crates/shamir-server and crates/shamir-db. Follower-originated ones switch to the
   generation-aware variant with the loop's generation (pass it out of the loop result or
   thread it through the supervisor wrapper's existing `generation` variable). Admin/API
   callers may legitimately keep the generation-less variant — document who may call it in
   a doc comment on mark_subscription_terminal.
5) Tests (must be written now; CI executes):
   (a) leased follower reaches the stall threshold → status_message persisted AND the loop
       KEEPS RUNNING (lease still valid, no exit) — fails before the fix via F2;
   (b) a stale generation's stall report is a no-op on a replacement binding's row;
   (c) lock-order test for F1 using the existing apply_park hook pattern: with a simulated
       apply holding the read guard, a lifecycle mutation waiting for the write gate does
       not deadlock with stall persistence because persistence happens after the guard
       drops; deterministic gates, no sleeps/timeout raises.
6) F4: make every injected row valid under the PR8b-2 catalogue rules — use the production
   CREATE SUBSCRIPTION path or the same field set the #188 tests use
   (binding_version=1, binding_generation>=1, binding_destinations with concrete db/repo),
   then re-trace each affected test (supervisor fixtures and seed/tests/receipts/*) to
   confirm it reaches and asserts the intended admission/stall path. NOTE the receipts/
   tests run against rows injected for backend admission: the row must be valid enough for
   read_subscriptions (from_row) to parse, but must still exercise the intended
   MemBuffer/Hybrid refusal.

## Gates (after the LAST edit, sequential, one at a time; logs .scratch/r12_*.log; report
every inner exit code including failed attempts + fixes):
  cargo check -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets
  cargo clippy -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets -- -D warnings
  cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server
  cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server -- --check
  cargo check -p shamir-db -p shamir-server
  python -I scripts/ci/check_layout.py
Rerun an unchanged command ONCE after a 0xc0000409 crash and say so. Grep the worktree for
conflict markers after the last edit; wc -l every touched file (<=1000); list every folder
you added files to (<=7 entries).

## Report
Per finding F1–F4 what changed (file:line); the lock-order argument step by step; the new
tests and what each catches; per-command exit codes incl. failures; explicit NOT-done list;
residual risks.

## ADDENDUM — real CI evidence (PR #189 run 39326435557 on b9ae90ae, ubuntu lib)
C1 (confirms F1): supervisor_tests::receipt_io_pause_retry_convergence HANGS (SLOW >360s,
nextest kill) — the stall-persistence self-deadlock on the lease apply gate. The lock-order
test (5c) must cover exactly this scenario, and receipt_io_pause_retry_convergence must
pass after the fix (verify its shape; it runs a leased supervised follower).
C2 (5 deterministic shamir-db failures, same message `atomic_batch_unsupported` on
testdb/main): binding_scope_tests::{binding_scope_resume_marker_removal_survives_reopen
(:97), pause_and_set_profile_persist_exact_durable_row (:724),
set_profile_alone_preserves_complete_row_through_reopen (:626)} and
quarantine_reads_tests::{t14_bound_destination_stays_refused_after_profile_drop_and_reopen,
t15_replacement_rebind_survives_reopen_and_resume_requires_full_recovery} (:83): PR #188
fixtures bind subscriptions to MemBuffer-wrapped destinations that THIS branch's
CREATE/RESUME admission refuses. Fix EVERY test fixture in the merged tree that binds a
subscription to a destination repo (grep crates/shamir-db and crates/shamir-server tests
for the fixture helpers creating the destination, not only these five): raw in-memory repo
where the test does not restart, raw fjall where it reopens; keep assertions intact.
C3 (2 failures, receipts/atomic_batch_gate_tests hybrid/membuffer `left: Some("active"),
right: Some("resync_required")`): root cause is the injected row missing binding fields —
already fixed by the F4 `inject_legacy_row` change; re-trace and confirm.
