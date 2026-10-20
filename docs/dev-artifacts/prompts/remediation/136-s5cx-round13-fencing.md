# Round 13 — finish the round-12 review findings G1–G3

Worktree CLEAN at 4dad76b3 (round 12 committed as de11ec9a, merged with master). NEVER run
git add/commit/merge/rm/xargs. No #[allow]; imports at top; scratch under .scratch/
(r13_*.log); one cargo process at a time, CARGO_BUILD_JOBS=4; capture every command's own
exit code; stop editing at 100 minutes; never end a turn with a background job outstanding.

## G1 (P1) — startup refusal paths must carry a generation fence
crates/shamir-server/src/replication/supervisor.rs: the start-admission refusals
(~:571-576 atomic-batch, ~:620-625 follower-state, ~:650-655 receipt-pointer reopen, and
the refuse_gate_init helper at ~:279) call the generation-less mark AFTER the startup
control-lock scope ended (~:480) and after further awaits (~:523, :583, :608-616, :638).
A concurrent DROP/re-CREATE or rebind can supersede the startup's snapshot in between (or
while the marking call waits for the control lock): the old start then marks the
REPLACEMENT row and quiesces its lease. Neither validity recheck (:520 before, :673 after
the marking branches) closes the window.
FIX: `start_subscription` already has `sub.binding_generation` (and the re-validated lease
generation) — pass `Some(generation)` on EVERY mark made by a start attempt (including
refuse_gate_init — thread the generation in as a parameter). A stale generation is a
logged no-op inside ShamirDb: row, cache and lease untouched.
Then re-grep ALL callers of mark_subscription_terminal / mark_subscription_resync_required /
mark_subscription_terminal_generation(..., None) across crates/shamir-server and
crates/shamir-db: the generation-less variant must have NO follower- or
supervisor-originated caller left. If only admin/test fixture callers remain, narrow the
generation-less method accordingly (e.g. keep it but rename its doc contract to
"admin/test-seam only" and audit every remaining call site is genuinely admin- or
test-initiated; if none in production code remains, consider removing it — choose the
minimal-diff option and justify). Fix the doc wording on mark_subscription_terminal (and
the start-admission comments at supervisor.rs ~:546-549/:593-606) that claims the start
marks cannot go stale — state the real fence (the generation passed to the call).

## G2 (P1) — generation 0 silently disables existing stall tests
follower_loop.rs ~:461: `lease.as_ref().map_or(0, |l| l.generation)` — an unsupervised
loop (lease: None) reports generation 0, but real rows carry binding_generation >= 1
(write_new_subscription stamps it), so the fenced status writer rejects generation 0 as a
stale no-op. Existing stall-visibility tests (repeated_failures…, progress_overwrites…,
empty_pull_with_advertised_backlog…, and any other test/fixture driving the loop WITHOUT a
lease) therefore never land their expected status_message.
DECIDE ONE RULE and document it in a one-line comment at the generation derivation:
- an unsupervised loop (empty subscription name) NEVER writes (unchanged);
- a supervised loop writes with its lease generation;
- tests that drive a SUPERVISED loop must give it the row's real generation — do NOT
  special-case 0 in production.
Update every affected test so it drives the loop with the row's real binding_generation
(take a lease via take_subscription_lease under the control lock like the round-12
leased-follower test does, or attach cfg.lease with the row's generation) AND asserts the
persisted status_message value (exact or expected). Name every changed test in the final
report. Audit for OTHER fixtures reaching the status writer with generation 0 (grep
with_subscription / FollowerLoopConfig::new in tests) and fix each.

## G3 (P2) — test quality
(a) guarded_stall_persistence_does_not_deadlock_against_write_gate
    (seed/tests/receipts/stall_visibility_tests.rs ~:621-737): the test must ACKNOWLEDGE
    that the lifecycle mutation actually OWNS the control lock and is parked on the
    apply_gate.write() BEFORE the follower is released. Deterministic handshake, NOT an
    assumption: e.g. after `arrived.notified()` spawn the mark task, then poll
    `lease.apply_gate.try_write()` in a bounded yield loop until it returns Err (a pending
    writer holds the write lock) — that proves the mutation is parked on the write gate —
    then `release.notify_one()`. No sleeps; bounded yield/try loops only. Keep the rest of
    the assertions.
(b) stale_generation_status_report_is_a_no_op_on_the_row (~:578-605): build a REAL
    REPLACEMENT binding — write a second row state (new generation = generation + 1,
    DIFFERENT destinations) through the production write path or replace_subscription_row —
    then submit the stale report with the OLD generation and compare the COMPLETE row
    (read the whole QueryValue row, not the (state,message) tuple) before/after. The fresh
    generation's report on the replacement row must still land. Deterministic, no sleeps.

## Gates (after the LAST edit, sequential, one at a time; logs .scratch/r13_*.log; report
every inner exit code including failed attempts + fixes):
  cargo check -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets
  cargo clippy -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets -- -D warnings
  cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server
  cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server -- --check
  cargo check -p shamir-db -p shamir-server
  python -I scripts/ci/check_layout.py
Rerun an unchanged command ONCE after a 0xc0000409 crash and say so. After the last edit:
grep the worktree for conflict markers (zero in sources); wc -l every touched file (<=1000);
ls every touched folder (<=7 entries).

## Report
Per G1–G3 what changed (file:line); the list of every remaining caller of the generation-
less marking API (or its removal); each changed test and what it now proves; per-command
exit codes incl. failures; explicit NOT-done list; residual risks.
