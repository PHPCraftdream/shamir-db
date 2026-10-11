# Brief — B02-W WalTicket and LeaderGuard

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.

## FILE SCOPE (only these paths may be edited)

- `crates/shamir-wal/src/wal_group_commit.rs` (Grep the real path), NEW `crates/shamir-wal/src/wal_ticket.rs` (one primary export: `WalTicket`), `crates/shamir-wal/src/lib.rs` (re-export), the in-memory WAL sink file (test-only Mem pause / sync-fail knobs, find it with Grep for the Mem sink used by the group-commit tests)
- tests: `crates/shamir-wal/src/tests/wal_group_commit_tests.rs` (or the existing group-commit test file) + NEW topic file for the ticket (+ one-line `pub mod`)
- NO `shamir-tx` / `shamir-engine` files (a later task wires the ticket into `VersionGuard` and the commit path); the public API of the existing `append`/`begin_grouped` entry points must keep compiling for their current callers

## Task

B02-W — make the WAL group-commit outcome observable and make a dropped leader harmless (design by a read-only review of the post-WAL work, 2026-10-11; PR #69 is merged on master). Read `wal_group_commit.rs` (module doc: the measured +22% cost of a per-append cross-task hop — do NOT add a spawn or an extra cross-task wake on the hot path), the private `Waiter`, `lead_until_drained`, and the follower park (~:175-197).

Problems: (1) a follower parks after queueing its frame; the leader writes the frame anyway; the follower cannot tell whether its frame reached the log. (2) If the caller elected as leader is dropped inside `lead_until_drained`, `flushing` stays true and the waiters it already took are never completed: a PERMANENT WAL hang (finding RV-WAL-81969DDB).

Required result:
1. A public `WalTicket` (new file) replaces the private `Waiter` — SAME allocation, no extra one. State machine (atomic): PENDING -> WRITTEN (the frame write succeeded; set for both durability tiers BEFORE any fsync) -> one of ACKED (the requested durability is reached), FAILED (the write failed and was rolled back, the frame is NOT in the log), UNKNOWN (the leader was dropped while the frame was in its window; the frame may or may not be in the log). Accessors: `state()`, and a way for the appender to obtain the ticket of its own frame (a new entry point such as `append_ticketed` that returns the ticket together with the existing completion future; the old entry points stay and wrap it).
2. A `LeaderGuard` in `lead_until_drained`: if it is dropped with a window in flight it completes every waiter of that window as UNKNOWN, marks the group INDETERMINATE (every later append fails fast with a typed error until the WAL is reopened — fail-stop on purpose: the `spawn_blocking` write may still land, so continuing could reorder or duplicate), and clears `flushing` so nothing hangs.
3. Under `Synced` durability: a write that succeeds followed by a failed fsync leaves the ticket in WRITTEN (not FAILED) and the error to the appender says so (a typed distinction, e.g. `NotWritten` vs `WrittenNotDurable`, additive on the error type; do not change how current callers match errors).
4. Test-only knobs on the Mem sink (pause before write completes, inject a sync failure) so the tests are deterministic with `Notify`/oneshot handshakes, no sleeps.
Red tests first: abort the leader mid-window -> the follower's ticket is UNKNOWN and its future resolves with the typed error (it does NOT hang); the next append fails fast; a follower of a normal group ends ACKED; an fsync failure leaves WRITTEN; a failed write leaves FAILED; the hot path (single appender, no contention) allocates nothing new (assert via the existing allocation/IO counters if the file has them, else skip and say so).
Do NOT run benches; say in the final message that the group-commit bench run is owed.
