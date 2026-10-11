# Brief — B02-S quiesce commits at shutdown

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs` (`ShamirDb::flush_all`, used at shutdown) and the system-store flush site (Grep `flush_buffers` in `crates/shamir-db/src` to find every shutdown flush: the system store, per-repo flushes)
- `crates/shamir-server/src/server/server_launcher.rs` ONLY if the shutdown sequence needs one call added; tests: a NEW shamir-server shutdown test file (+ one-line `pub mod`) and/or a shamir-db test
- NO `shamir-engine` files except a one-line accessor if `quiesce_commits` is not reachable from shamir-db (list it)

## Task

B02-S (task #1285) — quiesce in-flight commit tails before flushing at shutdown. PR #98 (B02-D) is merged on master: `RepoInstance::quiesce_commits(timeout)` waits for the `CommitTailBarrier` (inline, detached and AsyncIndex background tails) to reach zero; after the CommitDriver a connection future dropped by `run_until_cancelled` (`server_launcher.rs` ~:1079) leaves a commit tail running detached, so a shutdown that flushes and releases the stores right away can race that tail.

Required result:
1. `ShamirDb::flush_all` (`db_management.rs` ~:632) and the system-store shutdown flush call `repo.quiesce_commits(T)` for every repo BEFORE `flush_buffers` (T = a sane constant, e.g. 30 s, defined next to the call; on timeout log a loud warning naming the repo and continue — never hang the shutdown). It is NOT added to `flush_buffers` itself (`synced_flush` calls that while commits run; waiting for zero there would starve).
2. This makes the existing doc on `Drainer::drain_all` ("Shutdown callers must quiesce commits first") true — update that sentence's reference if it points at nothing.
3. The server shutdown path (`ServerHandle::shutdown`, which already waits for the stores to be released, PR #62) must call into the new quiesce through `flush_all` — verify the call order: connections stop admitting -> connection futures dropped -> quiesce -> flush -> release stores.
Red test (shamir-server or shamir-db, deterministic, `TEST_POST_WAL_PRE_PUBLISH_HOOK`-style parking is engine-private, so use what is reachable: e.g. a commit whose tail is parked by the engine test hook if the hook is `pub(crate)` only, then add a minimal `#[cfg(any(test, feature = "test-util"))]` public seam; say what you used): a commit parked after WAL accept during shutdown completes (the row is present after reopen), the stores are released (reopen works), and shutdown does not return before the tail finished; a wedged tail makes `flush_all` return after T with the warning (use a tiny T through a test override).
