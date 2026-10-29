# 253 — audit-fix-proc-tree (P1 audit follow-up: G5d bounded startup cleanup + reap)

Source: post-merge audit of PR #347 (P1 G5d, new crate `shamir-proc-tree`), two verified abnormal-path gaps on master:
1. **Unbounded wait on startup errors.** Unix: if the waiter thread cannot be spawned, cleanup ignores both kill results and calls an unbounded `child.wait()` (`crates/shamir-proc-tree/src/process_tree_unix.rs` ~29-49). Windows: job assignment / resume failure has the same unchecked-kill then unbounded-wait sequence (`process_tree_windows.rs` ~23-28). If termination fails while the leader stays alive these paths can block forever - the pattern the card requires removing. Existing injected tests run inside `supervise`, so they never reach these branches.
2. **Reap gap on waiter error.** A Unix waiter error is consumed by `wait_exit`; `reap` returns without trying to reap while `exit_seen` stays false (~62-75, 91-98). Killing on Drop does not reap the leader.
The Darwin zombie-only-group EPERM handling landed in #373 - keep it.

## Change
1. Make both startup-error cleanups bounded and checked: the kill result is inspected and recorded in the returned diagnostic, the wait is bounded (reuse the crate's existing bounded-drain/timeout constant and polling helper; no new sleeps in production beyond that helper), and on timeout the error says the leader could not be reaped. No unbounded `wait()` may remain anywhere in the crate (grep for `.wait()`).
2. `reap`: when the waiter reported an error, still attempt a bounded non-blocking reap (`try_wait` loop with the same bound) so the leader is not left as a zombie.
3. Tests: add injection seams in the same style as the existing ones (no sleeps for ordering; readiness handshake + watchdog) covering (a) waiter-spawn failure with a kill that fails / does not terminate -> returns within the bound with the diagnostic, (b) waiter-error then reap path. Windows branch: add a unit-testable seam if the code is `cfg(windows)` only; if impossible, say so explicitly in the report.

## Rules
Only `shamir-proc-tree`. No new dependencies, no `#[allow]`, layout limits, imports at top (move the block-local Windows import in `tests/teardown_tests.rs` ~32 to the file header while there).

## Local gates (only these)
`cargo fmt -p shamir-proc-tree -- --check`; `cargo clippy -p shamir-proc-tree --all-targets -- -D warnings`; `cargo check -p shamir-wasm-host --all-targets`; `python -I scripts/ci/check_layout.py`. Real exit codes.
