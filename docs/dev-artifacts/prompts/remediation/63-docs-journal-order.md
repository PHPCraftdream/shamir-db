# Brief — S5b-5 docs: journal ordering and stale limitations

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

- `docs/guide-docs/KNOWN_LIMITATIONS.md` ONLY: (a) the replication journal paragraph in section 5 (~:545-565) and the changefeed/journal claims near it, (b) the stale B04b vector bullet in section 1 (~:28-30), (c) the stale literal-NULL aggregate bullet in section 1 (~:63-68), (d) the runtime byte enforcement bullet (~:48-56) ONLY if it still says enforcement is OFF in the server policy
- the module docs of `crates/shamir-tx/src/changefeed.rs` (the comment blocks near ~:29-40 and ~:80-85 that describe journal ordering) - doc comments ONLY, no code
- NOTHING else: other agents edit the slow-consumer bullet (~:508-513, N1L-2) and the gap-marker interval semantics (S5c-1) - do not describe those two (not merged yet)

## Task

S5b-5 + DOC-1 follow-ups (tasks #1294 and #1298). S5b-1..S5b-4 are MERGED on master (your base): `publish_journaled` enqueues the journal event immediately before the version is published (local commits and the follower's downstream feed), journal reads are bounded by the contiguous completion watermark W after a barrier (`RepoInstance::read_changelog_from`, `JOURNAL_CONFIRM_BUDGET` ~1 s), B04b, NULL-AGG (#112) and W4b (#120, server `enforce_result_size` default true) are merged. Read the current code before writing: `publish_journaled.rs`, `repo_instance.rs` (`read_changelog_from`, `changefeed()`), `changefeed.rs` (`confirm_through`, `read_from_bounded`, `mark_gap`), `bridge.rs` ~:411-415, `follower_loop.rs` (JournalGap handling), and `KNOWN_LIMITATIONS.md` as it stands.

Update ONLY the docs and doc comments named in the scope so that every statement is true for the code on master:
1. Replication journal paragraph (section 5): journal reads return only events `<=` the highest version at which every lower version is settled (W), after everything queued before the read was written; a version still committing pauses replication instead of being skipped (the old text said 'writes can journal version 11 before 10' - that is fixed for pulls); a full queue, a write failure, a corrupt record, a replayed orphan and a changefeed init failure are reported as `gap_at`; the gap marker survives restart (the old text says it is forgotten); STILL TRUE and must stay stated: a crash can lose the unflushed journal tail with NO gap marker (recovery does not check the journal; tracked as S5c); live subscriptions can still skip events because the broadcast follows commit-completion order and the bridge drops versions `<=` the last delivered (`bridge.rs` ~:411-415; tracked as N1-live) - use journal pulls for gap-free history; followers stop with `JournalGap` and `resync_required`, they do not reseed automatically.
2. The three stale claims named by S5b-5: persist failures mark a gap, corrupt records mark a gap, the gap marker survives restart (verify each against the code and cite file:line).
3. B04b bullet: vector similarity inside Snapshot/Serializable now ranks exactly over the pinned snapshot plus the staged overlay (O(N*d), no ANN there); `_id` keyset ordering works; reference `table/filtered_vector.rs`, `read_exec.rs` (verify names/lines).
4. Literal-NULL aggregate bullet: remove it or rewrite it to the true behaviour after NULL-AGG (#112): an explicit present-null aggregate input is distinguishable from absent input (verify in `select.rs` `deserialize_aggregate_input` and the engine's `build_aggregate_object`).
5. Runtime byte enforcement bullet: only if it still says OFF in the server policy - it is now ON by default (`security.query_limits.enforce_result_size`, default true; see `crates/shamir-server/src/db_handler/config.rs` and `docs/guide-docs/guide/07-operations.md`); keep numbers in sync.
6. The `changefeed.rs` module docs (doc comments only) must match the merged ordering design.
Every code reference must be re-verified against the final text (line numbers!). No code changes. No new files. Keep the existing document structure and style (English prose). Do NOT describe the slow-consumer behaviour change or the gap-marker interval (other open PRs).
