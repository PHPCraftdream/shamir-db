# Brief — B02-G WalQueued state

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

- `crates/shamir-tx/src/version_guard.rs`, `crates/shamir-tx/src/repo_tx_gate.rs`, `crates/shamir-tx/src/repo_wal_manager.rs` (`begin_grouped_ticketed`), and their tests: `crates/shamir-tx/src/tests/version_guard_post_wal_tests.rs` + NEW topic files (+ one-line `pub mod`)
- `shamir-wal` ONLY if a tiny additive accessor on `WalTicket` is missing (list it)
- NO `shamir-engine` files (the drainer resolution and the commit-path wiring are a later task: `commit.rs`, `pre_commit.rs`, `drainer.rs` are NOT yours; the engine must keep compiling, so every new API you add must be additive and unused by the engine for now)

## Task

B02-G (task #1284) — the pre-WAL counterpart of the post-WAL orphan: a version whose WAL frame was QUEUED but whose committer vanished must be resolved from the frame's real fate instead of being aborted blindly. Design by a read-only review (2026-10-11). Your worktree contains: PR #92 (`WalTicket`: states PENDING / WRITTEN / ACKED / FAILED / UNKNOWN, `append_ticketed`, fail-stop `LeaderGuard`) and PR #89 (the post-WAL membership sets are `scc::HashMap`s in `VersionGuard` / `RepoTxGate`). Read `wal_ticket.rs`, `version_guard.rs`, `repo_tx_gate.rs`, `repo_wal_manager.rs` first.

Problem: a follower parks after queueing its frame; the leader writes the frame anyway while the follower's `VersionGuard` is still pre-WAL; if the follower is then cancelled the guard aborts although the frame IS in the WAL: lost in this session (version Aborted, never replayed), resurrected after a crash (recovery replays the WAL).

Required result (additive; nothing here may change the behaviour of the current commit path, which does not call the new APIs yet):
1. `RepoWalManager::begin_grouped_ticketed(entry, durability)` returning the `WalTicket` together with the completion future (wrapping `append_ticketed`); the existing `begin_grouped` keeps its signature.
2. `VersionGuard::wal_queued(ticket, entry)`: called right BEFORE awaiting the frame; it moves the insert into the active set from `wal_accepted()` to here (the version is "possibly in the WAL" from the moment it is queued) and keeps the entry for hand-over. `wal_accepted()` stays valid (idempotent) for callers that do not use tickets. `record_commit_writes` ordering: document (do not move it; the engine does that later) that it may move before queueing — an abort then leaves only footprints that can cause spurious SSI aborts, never a missed conflict.
3. `Drop` of a guard that is `wal_queued` and not committed resolves by the ticket's state: WRITTEN or ACKED -> register as an orphan exactly like a post-WAL guard AND hand the entry to the gate (new `RepoTxGate::orphan_entry` / the existing orphan map, whichever fits; the drainer needs the entry because the frame may not be in its window); FAILED -> mark the version Aborted in BOTH trackers (like a pre-WAL abort); PENDING or UNKNOWN -> insert into a new `wal_unresolved: scc::HashMap<u64, Unresolved, THasher>` (version -> ticket + entry) on the gate, leave the version neither aborted nor orphaned.
4. Gate API for the (later) drainer: `unresolved_wal_versions()` (ascending snapshot), `take_unresolved(v)`, `abort_unwritten(v)` (marks Aborted in both trackers and advances like a pre-WAL abort), and a `requires_reopen` flag (`AtomicBool`, `set_requires_reopen()`, `requires_reopen()`) for the UNKNOWN fail-stop; `RepoTxGate::assign_next_version_guarded` callers will check it later — do not wire it into anything now except exposing it.
5. Keep every counter/high-water behaviour of the existing post-WAL code (`orphan_max`, `active_post_wal`, `orphaned_post_wal`) unchanged; use `scc::HashMap<_, _, THasher>`; no `len()` (clippy.toml bans it: keep atomic mirrors if you need counts).
Red tests (deterministic; a `WalTicket` can be built directly in tests through the Mem sink knobs from PR #92 or a test constructor you add to `wal_ticket.rs`): drop after WRITTEN -> orphaned + entry handed over; drop after ACKED -> same; drop after FAILED -> Aborted in both trackers and the watermark advances; drop while PENDING -> in `wal_unresolved`, not aborted, not orphaned, watermark blocked; drop after UNKNOWN -> in `wal_unresolved`; `take_unresolved` / `abort_unwritten` behave; a guard that is committed normally is unaffected; the legacy post-WAL path (`wal_accepted` without a ticket) is unchanged.
