# Brief — S5a changefeed persisted gap marker

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

- `crates/shamir-tx/src/changefeed.rs` (+ its tests under `crates/shamir-tx/src/tests/`)
- `crates/shamir-engine/src/repo/changelog_store.rs` — ONLY if the gap marker needs a key helper or `range_from` must return keys
- `crates/shamir-engine/src/repo/repo_instance.rs` — ONLY the place where the changefeed is created at init (load the persisted gap marker there)
- NO `drainer.rs` / `commit*.rs` / `materialize.rs` / `version_guard` files (another task owns the post-WAL path); NO server/replication follower files

## Task

S5a — replication journal holes that are invisible today (findings N2, N3, N4 of the replication design review). The follower trusts `first_gap_version`; a hole the leader does not signal makes the follower advance its bookmark past lost events.

Observed on master (verify the line numbers):
- N2: `persist_one` (`crates/shamir-tx/src/changefeed.rs` ~:637-650) only LOGS a serialize/put failure; the version is skipped silently and no gap is recorded.
- N3: a corrupt/undecodable journal record is skipped silently when reading (~:409-412); a reader then sees a contiguous-looking stream.
- N4: `first_gap_version` is RAM-only (~:246); after a leader restart it is forgotten, so a pre-restart hole is no longer reported.

Required result:
1. `RepoChangefeed::mark_gap(v)` (idempotent, keeps the minimum gap version, safe from any thread) — the single place that records a hole.
2. Call it on N2 (a failed serialize or put of version `v`) and on N3 (a corrupt record: conservatively mark the gap at `from_version` of the read, over-signalling is explicitly allowed by the contract documented at ~:415).
3. Persist the marker (a small dedicated key in the same changelog store/info store the journal already uses — pick the existing durable store of the changefeed, do not add a new backend) and load it when the changefeed is created at repo init, so the hole survives re-creating the feed over the same store.
4. A reader/Pull that crosses the persisted gap must see `first_gap_version == Some(gap)` exactly like for an in-RAM gap (reuse the existing path; do not invent a second signalling mechanism).

Red tests first: (a) a fake/failing store with exactly one failing put -> `gap_at` is `None` today, expected `Some(v)`; (b) a corrupt record in the journal -> gap reported; (c) the marker survives re-creating the feed over the same store, and a no-failure feed still reports no gap (control). Deterministic, no sleeps.

Out of scope (a separate task after the post-WAL PR lands): the drainer calling `mark_gap` for orphaned post-WAL versions. Do not touch it.
