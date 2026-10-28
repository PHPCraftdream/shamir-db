# 232 — flake-crash-recovery-bounded-child (FLAKE #1537)

`crates/shamir-engine/tests/crash_recovery.rs::crash_at_phase6_5_recovers_full_tx` hit nextest's 600 s TIMEOUT once on ubuntu CI (siblings pass in ~0.2 s; the rerun passed). Root-cause analysis (read-only, @oh):
- the parent waits for the crash child with `std::process::Command::output()` and NO bound (`tests/crash_recovery.rs` `spawn_child`, ~L310), on the parent's tokio thread, so any child that does not exit hangs the test until nextest kills it and the child's captured output is lost;
- no product deadlock was found between Phase 6 publish and the `phase6_5` abort seam (`src/tx/commit/pipeline/phase/materialize.rs` ~L382 → `src/tx/commit/pipeline/phase/commit/entry.rs` `maybe_crash`); candidates are a fjall persist/set stall in the widest seam window, a kernel core-dump stall on abort, or a skipped seam with the child stuck in the unbounded `drain_all` / runtime-drop fallback;
- the parent's reopen/recover path also has no bound.
The fix makes every crash-child wait bounded and self-diagnosing so the next occurrence fails fast (~60 s) with the evidence that names the cause. Do NOT raise any nextest timeout.

All paths relative to `crates/shamir-engine/`.

## Changes
1. New file `tests/crash_recovery/child_proc.rs` (wired from `tests/crash_recovery.rs` with `#[path = "crash_recovery/child_proc.rs"] mod child_proc;`):
   - `pub(crate) const CHILD_STALL_EXIT: i32 = 86;`
   - `const CHILD_DEADLINE: Duration = Duration::from_secs(60);`
   - `pub(crate) async fn run_crash_child(label: &str, cmd: tokio::process::Command) -> (ExitStatus, String)`: spawn with stdin null, stdout/stderr piped, `kill_on_drop(true)`; drain both pipes in spawned tasks; `tokio::time::timeout(CHILD_DEADLINE, child.wait())`; on expiry snapshot per-thread kernel state (Linux: `/proc/sys/kernel/core_pattern`, `/proc/<pid>/status` State/CoreDumping/Threads lines, and per `/proc/<pid>/task/<tid>`: comm, state field from stat, wchan, syscall; other OS: a short "not collected" string), kill the child, then panic with label, deadline, thread snapshot and the captured stdout/stderr. On success return the status and a formatted "--- child stdout --- / --- child stderr ---" string.
2. `tests/crash_recovery.rs`:
   - `spawn_child` becomes `async fn spawn_child(phase, repo_path) -> (ExitStatus, String)` built on `tokio::process::Command` + `run_crash_child`.
   - `crash_then_recover`: assert the status is not `CHILD_STALL_EXIT` (message includes child output); append child output to the existing "child must die abnormally" assertion; wrap reopen → `recover_v2_inflight` → `get_table` → counts in `tokio::time::timeout(PARENT_DEADLINE = 60 s, ..)` with a `Cell<&'static str>` stage marker so a stall panics as "parent stalled in <stage> after <phase> crash" plus child output.
   - `run_child_scenario`: `eprintln!` breadcrumbs ("crash-child: open", "create_index", "insert", "commit_tx", "commit_tx returned {..}"); bound `drainer().drain_all(..)` with `tokio::time::timeout(CHILD_DRAIN_DEADLINE = 10 s, ..)` and on expiry `eprintln!` + `std::process::exit(CHILD_STALL_EXIT)`.
   - `child_entrypoint`: after `rt.block_on(..)` call `rt.shutdown_timeout(Duration::from_secs(5))` so blocking-pool teardown cannot hold the exit hostage.
   - Keep all existing comments; imports at file top.
3. `src/tx/commit/pipeline/phase/commit/entry.rs` `maybe_crash` (test-only crash seam, active only when the crash env var is set): `eprintln!("maybe_crash: flushing before abort at {phase}")` before the flush and `eprintln!("maybe_crash: abort at {phase}")` right before `std::process::abort()`. Nothing else in product code.
4. The same unbounded `.output()` hazard in the sibling crash harnesses must use `run_crash_child` too: grep `tests/crash_recovery/` (and any other `tests/**` crash harness that re-execs the test binary) for `.output()` — at least `history_seed.rs`, `multi_table_drain.rs` (2 sites), `vector_delta.rs`, `write_to_data.rs`. Convert each to `tokio::process::Command` + `super::child_proc::run_crash_child(label, cmd).await` (make the enclosing helpers async as needed, keep their assertions and add the child output to their failure messages). If one of those files is wired from a different test root where `child_proc` is not reachable, reach it with a `#[path]` module there as well — exactly one copy of the code.
5. Check `tokio` features available to shamir-engine tests (`process`, `io-util`, `time`, `rt`); if `process` is missing from the dev-dependency features, add it to `[dev-dependencies]` tokio features only (no new crates, no version bumps).

## Layout / rules
- `tests/crash_recovery/` gets one new entry; keep every folder <= 7 entries and every file <= 1000 lines.
- shamir-engine denies `unwrap_used`/`expect_used` in non-test code — the only product edit is two `eprintln!`s.
- No `#[allow]`, no local `use`, no visibility widening beyond `pub(crate)` inside the test crate.

## Local gates (only these; one crate at a time; never --all-features)
- `cargo fmt -p shamir-engine -- --check`
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`
- `cargo clippy -p shamir-engine --no-default-features --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py` (tracked files only: report every NEW file path)
Note: `thread_states` for Linux is `#[cfg(target_os = "linux")]`; this Windows host compiles only the non-Linux stub — keep the Linux body simple and obviously correct (std::fs only).
