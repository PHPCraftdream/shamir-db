# Brief — B02-R wire ticketed WAL append

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

- `crates/shamir-engine/src/tx/commit.rs` (the two `begin_grouped` call sites of the lock-free path and the AsyncIndex path inside the CommitDriver's future), `crates/shamir-engine/src/tx/pre_commit.rs` (the legacy `begin_grouped` site in `pre_commit_locked`), `crates/shamir-engine/src/tx/drainer.rs` (resolution of `wal_unresolved`, `mark_gap` for orphans, the `requires_reopen` halt)
- `crates/shamir-engine/src/tx/commit.rs` `TxError` (a typed variant for the fail-stop WAL state) and its mapping where `TxError` is mapped to wire codes (one-line arms)
- tests: `crates/shamir-engine/src/tx/tests/drainer_tests.rs` (or a NEW topic file) + `post_wal_cancel_tests.rs` only for mechanical updates (+ one-line `pub mod`)
- shamir-wal / shamir-tx: ONLY tiny additive accessors if something you need is missing (list them)

## Task

B02-R (task #1286) — wire the ticketed WAL append into the commit path and let the drainer resolve versions whose frame fate is unknown. On master (your base): B02-W (`WalTicket` PENDING / WRITTEN / ACKED / FAILED / UNKNOWN, `append_ticketed`, fail-stop `LeaderGuard`: a dropped leader marks the group indeterminate so later appends fail fast until reopen), B02-G (`VersionGuard::wal_queued(ticket, entry)`, `RepoTxGate::{wal_unresolved, unresolved_wal_versions, take_unresolved, abort_unwritten, requires_reopen}`, `RepoWalManager::begin_grouped_ticketed`), B02-D (CommitDriver: the commit tail survives its caller, so these paths are the fallback for a continuation that dies — panic, runtime teardown), B02-F (drainer orphan replay ordering). Read `wal_ticket.rs`, `version_guard.rs`, `repo_tx_gate.rs`, `commit.rs`, `drainer.rs` first.

Required result:
1. Commit path: replace `wal.begin_grouped(..)` + `version_guard.wal_accepted()` by `begin_grouped_ticketed` + `version_guard.wal_queued(ticket, entry)` called BEFORE awaiting the frame, in the lock-free path, the AsyncIndex path and the legacy `pre_commit_locked` site. `gate.record_commit_writes(footprint)` moves BEFORE queueing (an abort then leaves only footprints that can cause spurious SSI aborts, never a missed conflict — keep the comment from B02-G). A typed error split in shamir-wal's append result (`NotWritten` vs `WrittenNotDurable`, if PR #92 exposed it; otherwise map from the ticket state): `NotWritten` aborts the commit as today; `WrittenNotDurable` (written, fsync failed under Synced) must NOT abort: the frame is in a replayable log, so run the tail and report the durability failure to the caller as a typed error after the version is published/orphan-safe (never leave a version that recovery would resurrect while this session aborted it). A group in the fail-stop (indeterminate) state makes `commit_tx` return a typed error (`TxError` variant, wire code mapping) BEFORE allocating further work, and `RepoTxGate::requires_reopen()` is checked once at the admission point of a commit.
2. Drainer: at the start of each pass resolve `wal_unresolved` in ascending version order: WRITTEN -> `offer` the stored entry and complete it as an orphan (existing path); FAILED -> `gate.abort_unwritten(v)`; PENDING -> blocked by an in-flight version (stop the pass cleanly, NO full `wal.recover()` reseed — keep `recover_calls` unchanged); UNKNOWN -> fail-stop: set `requires_reopen`, halt the drain at v-1, log loudly (never guess Aborted: the data would come back after a restart; never guess Written: it would be lost after a restart — cold recovery decides on reopen).
3. N5 (from the replication design, PR #84 landed `RepoChangefeed::mark_gap`): `complete_orphaned_post_wal(v)` calls the repo changefeed's `mark_gap(v)` so a follower stops honestly on an orphan whose changefeed event was never emitted (reachable through `RepoInstance`; if the feed is not yet initialised, initialise it — it is lazy).
Red tests (deterministic; use the Mem sink knobs from PR #92 and the existing post-WAL hook): FAILED resolves to an abort and the watermark advances; PENDING blocks the pass with `recover_calls` unchanged; WRITTEN replays as an orphan with its data; UNKNOWN fail-stops (`requires_reopen`, later commits get the typed error, drain halts); an fsync failure after a successful write does not abort the commit and reports the typed durability error; orphan completion marks the changefeed gap (a Pull across it reports `gap_at`).
Do NOT run benches (state that `tx_pipeline` and the group-commit bench are owed: the ticketed append sits on the hot path).
