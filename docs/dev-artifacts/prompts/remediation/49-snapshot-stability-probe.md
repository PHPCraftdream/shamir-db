# Brief — INVESTIGATE snapshot stability across out-of-order publish

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

- tests only: NEW `crates/shamir-engine/src/tx/tests/snapshot_visibility_park_tests.rs` (+ one-line `pub mod` in `crates/shamir-engine/src/tx/tests/mod.rs`), reusing the existing post-WAL/materialize test hooks (`PostWalPrePublishHook`, the hook at `materialize.rs` ~:105, `post_wal_orphan_ordering_tests` fixture)
- a tiny test-only seam in `crates/shamir-engine/src/tx/materialize.rs` / `finalize.rs` ONLY if the existing hooks cannot park a committer at the needed point (LIST it)
- NO production behaviour change in this task: this is an investigation + red test. If the test is red, STOP and report; do not fix.

## Task

INVESTIGATE (task #1297; found by the ox S5b review, UNPROVEN). Evidence first, no production edit.

Claim to test: `last_committed` is raised early and out of order on master (Phase 5a: `mvcc_history.rs` ~:522 `publish_committed_max`, also ~:407, ~:633 and `recovery.rs` ~:677; see the comment at `pre_commit.rs` ~:1990-2003). A transaction that begins while `last_committed` is 11 gets `snapshot_version = 11`. If tx A holds v10 (allocated earlier) and is parked before `materialize`/publish while B (v11) completes, can C (snapshot 11) later SEE A's writes after A is released - i.e. does a repeated read inside one snapshot change?

Read first: how `begin_tx` picks `snapshot_version` (gate / `last_committed` / completion watermark), how the MVCC read filters by `snapshot_version` (`get_at`, `snapshot_stream` in `crates/shamir-tx/src/mvcc_store/`), and the Phase 5a publish. Then write ONE deterministic test (disjoint tables are NOT needed here: use two different keys of the same table or two tables, whichever your parking hook supports): park A (v10) before materialize, commit B (v11) fully, begin C with Snapshot isolation (snapshot 11), read the key A wrote (expect: not visible), also run a full scan and a COUNT inside C; release A and wait until A's commit completes; repeat the same reads in C - they MUST return exactly what they returned before (snapshot stability), for point read, scan and COUNT. Also assert a NEW transaction D begun after A completed sees A's write.
Report: (1) green -> state precisely WHY the snapshot is stable (which mechanism guarantees versions > snapshot_version or still-uncommitted versions are filtered), with file:line; (2) red -> the exact failing assertion, the mechanism with file:line, and 2-3 candidate fixes with their hot-path cost (candidate: snapshot version from the contiguous completion watermark instead of `last_committed`; candidate: filter reads by completion). Do NOT implement a fix: the owner decides after the report.
Do NOT run benches.
