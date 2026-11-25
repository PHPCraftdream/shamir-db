# Brief: REL-R2d-P5 (task #1978), round 3 - lifecycle ownership of the schedule state and the remaining oracle gaps (findings re-verified by the orchestrator)

You continue session `p5-1` in the same worktree. Round 2 is kept except where this brief changes it. The independent review of round 2 returned FAIL; the orchestrator read the code and accepts the findings below. Paths: `backup_online/` = `crates/server/shamir-server/src/boot/config/diagnostics/backup_online/`. Re-open every path and line before relying on it.

## Compile / build / test policy - MANDATORY, highest priority

Compilation, builds and tests run on GitHub CI, not on this machine. Banned: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest builds and runs, WASM builds, e2e and integration harnesses, and any other command whose purpose is to build the project or run its tests. Allowed locally, scoped to the touched crates: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. Every cargo call goes through the machine-wide queue wrapper exactly as in rounds 1 and 2 (`-j 1` before the `--`). A child that believes it needs a build or test run says so in its final message.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

## Findings to fix (in this order)

### 1. The schedule cells have no owner (P1)
The three cells (info, last run, next run) are process-global with no scheduler identity: a schedule-less launch clears another live scheduler's state, an older scheduler's exit can clear a newer installation, an installation publishes the new info before it resets the old last/next values, and the clear at the end of `run_with_anchor` is skipped when the future is dropped or unwinds. Fix with a GENERATION owner (no new dependency):
- `install_schedule_info(info) -> u64`: take a new generation from one global `AtomicU64`, RESET last-run and next-run first, then publish the info together with its generation (one `ArcSwapOption` value, so a reader sees info and generation together).
- `note_schedule_last_run`, `note_next_run_unix_ns` and the registry's `note_scheduled_run_outcome` write only when the installed generation is the caller's (the registry does not know the generation: give `OnlineBackupRegistry` no new field if `registry.rs` would pass 1000 lines (it has 998); instead publish the last run unconditionally for a scheduled run, which is correct because only the live scheduler starts scheduled runs, and keep the generation check for the next-run cell and the clear).
- `clear_schedule_state_if(generation)`: clears all three cells only if the installed generation is still that one.
- The clear moves from the end of `run_with_anchor` into a DROP GUARD created in `spawn_with_observer` and moved into the spawned future, so it runs on normal exit, unwind and future drop (task abort). `run` and `run_with_anchor` then have no global side effect except the next-run publication: make that publication a parameter (a closure `FnMut(Option<u64>)`, production passes the generation-checked `note_next_run_unix_ns`, the `#[cfg(test)] run` wrapper passes a no-op) so `schedule/tests/decisions.rs` (not serial) can no longer touch the global cells. The wrapper also keeps taking its own anchor only for the unit tests.
- A schedule-less launch needs NO clear any more (a scheduler clears its own state when its task ends): remove the `launch.rs` clear and leave that file as it is on the parent.

### 2. The lifecycle tests must exercise the production paths (P1)
Replace `schedule_key_absent_until_configured_and_after_clear` (it calls the clear function itself). Required, through the REAL reply (real adapter, superuser dispatch):
1. Spawn a REAL scheduler through `spawn_for_test`/`spawn_with_observer` with a short interval and cancel its shutdown token; join the task; the reply has NO `schedule` key afterwards (kills removing the drop-guard clear). While it runs the reply HAS the section.
2. Two generations: install A, install B (a second scheduler), end A: the section of B is still present (kills an unconditional clear); end B: absent.
3. Abort the task handle of a running scheduler (not a graceful stop): the section disappears (kills a clear that only runs at the end of the future).
All three `#[serial]`; every wait is a completion handshake (a channel or a join, no `park_timeout` loop, no sleep) bounded by at least 120 s.

### 3. The failed-run test (P2)
`failed_scheduled_run_publishes_stable_last_run_and_survives_a_manual_run`: replace the `park_timeout` polling loop with a handshake (the `on_terminal` callback and the run's `finished` signal), compare the WHOLE `last_run` map before and after the manual run (not only some keys), assert `started_at_unix_ns` equals the scheduled run's own registry timestamp (read it from the registry status right after the scheduled run is admitted, before the manual run), and assert the manual run actually succeeded.

### 4. Smaller test fixes
- The read-only stamp test must also assert that the reply contained a `schedule` section (otherwise skipping the read path stays green).
- `non_superuser_is_refused_with_a_configured_schedule`: either count adapter calls with a small counting wrapper around the real adapter and assert zero, or drop the claim that the adapter is never called from the doc and the report (the gate already refuses before dispatch).

### 5. Terminal paths that never reach the final publication (P2, document, do not code)
A thread-spawn failure of a scheduled run and the runner-thread unwind guard finalize the record without a last-run update (`registry.rs` has no room: 998 lines). State it in the guide subsection: `last_run` is the final record of the last scheduled run that reached the normal final publication; a run lost to a runner-thread failure still counts in `runs.*` and keeps the previous `last_run`. Do not touch `registry.rs` beyond what findings 1 needs (nothing is expected).

## Gates and report
After your LAST edit: `cargo fmt -p shamir-server -p shamir-server-security`, `cargo check -p shamir-server -p shamir-server-security --all-targets -j 1`, `cargo clippy -p shamir-server -p shamir-server-security --all-targets -j 1 -- -D warnings`: print every exit code from the invoking shell as `name_exit=$?` captured right after each command (never behind a trailing `echo`), then `python -I scripts/ci/check_layout.py` (hand-count the entries of any folder that gets an untracked new file) and `python -I <orch>/vis_widen.py <worktree>` and READ its output. No new `#[allow]`, no widening of an existing item, no new dependency, no wire tag or stable code change. Final report: ONE complete standalone message with the gates and exit codes, per finding what changed and where, the test list with the real mutant each one kills (never claim a mutant a test does not exercise), the final line counts of `registry.rs`, `metrics.rs`, `runner.rs`, and every deliberate deviation.
