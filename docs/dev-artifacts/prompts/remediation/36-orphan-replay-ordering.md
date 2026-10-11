# Brief — B02-F orphan replay ordering

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

- `crates/shamir-engine/src/tx/drainer.rs` — the orphan-completion code (`complete_orphaned_post_wal`, its index replay and its table lookups, ~:540-560, ~:650-660, ~:800-810, ~:960-1010); the `index_applied` bookkeeping
- tests: `crates/shamir-engine/src/tx/tests/post_wal_orphan_ordering_tests.rs` (un-ignore the test marked `#[ignore = "#1287"]`) and NEW topic files next to it (+ one-line `pub mod`)
- NO commit.rs / materialize.rs / version_guard.rs / repo_tx_gate.rs changes beyond a one-line accessor if truly needed (list it)

## Task

B02-F (task #1287) — two real defects in the drainer's ORPHAN completion, found by the part-A hardening agent (PR #89; your worktree is built on it, so `active_post_wal` / `orphaned_post_wal` / `counter_applied` are already `scc::HashMap`s). Read the part-A test file `post_wal_orphan_ordering_tests.rs` and `drainer.rs` first. Context: PR #69 made a commit cancelled after the WAL accepted it recoverable by letting the drainer replay the WAL entry (`complete_orphaned_post_wal`); a later change makes ordinary cancellations finish their own tail, so this path will be rare (panics / dead runtime) but it must still be correct.

1. STALE-POSTING CLOBBER. An OLDER orphan's `IndexPut` replay (drainer ~:541-548, :802-804) overwrites the posting that a LATER transaction already applied inline (e.g. the later tx updated/deleted the same record), and the later drain pass skips the repair because `index_applied` records only the later version. Test `post_wal_cancel_same_record_later_writer_posting_survives_drain` (currently `#[ignore = "#1287"]`) is the oracle — un-ignore it. Fix with last-writer-wins BY VERSION: keep, per (table token, posting key), the highest version whose posting state has been applied (in memory, monotone; a lock-free map in the style of the other membership sets: `scc::HashMap<_, _, THasher>`), and make the orphan replay skip a posting op whose version is lower than the recorded high-water; the inline apply path (materialize) must record its version in the same map. Decide the exact granularity, document it in a comment (short), and make sure memory cannot grow without bound (prune entries at or below the durable watermark, when no orphan below it can exist).
2. MISSING TABLE. Phase B resolves the table through `per_table_mvcc` while Phase C uses `table_by_token`; when a table is gone (concurrent DROP) the orphan's history write is skipped but finalisation still proceeds to durability (~:654-658, :963-980). Resolve the table through ONE lookup; a missing table must STOP the pass at that entry (the orphan stays not-durable and is retried next pass; do not loop forever inside one pass), exactly like a failed table lookup in B3 of PR #69. Add a test: orphan whose table is dropped between the cancel and the drain -> `durable_watermark` does not pass it, no panic; after the table is recreated/restored or the entry is explicitly discarded (decide: if a dropped table can never come back, completing the orphan as 'table dropped' is correct — then write that rule down and test it instead; pick the one consistent with how recovery treats WAL entries of dropped tables, read `recovery.rs`).
3. Comment (one short paragraph) at the legacy `begin_grouped` call site is out of your scope; skip it.
