# Brief — B06 reverse-FK cache snapshot

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

- `crates/shamir-engine/src/repo/fk_reverse_cache.rs`
- `crates/shamir-engine/src/query/batch/fk_restrict.rs`, `fk_actions.rs`, `fk_on_update.rs` (only the reverse-FK lookups)
- `crates/shamir-engine/src/query/batch/query_runner.rs` (ONLY the FK warm/role lookups: `implicit_tx_isolation_for_fk_parent`, `require_footprint_if_fk_child` and their direct helpers)
- tests: `crates/shamir-engine/src/query/batch/tests/fk_reverse_cache_race_tests.rs`, `fk_reverse_cache_tests.rs` (or a new topic file in that directory plus its one-line `pub mod` in `tests/mod.rs`)

## Task

B06 (GitHub issue #37, readable with `gh issue view 37`) — the reverse-FK cache must return ONE consistent snapshot.

Observed on master:
- `crates/shamir-engine/src/repo/fk_reverse_cache.rs` around :376-377: after a successful publish a second lookup plus `unwrap_or_default()` can read a cache that was invalidated in between and return an EMPTY set; around :397-447 the "peek" flags answer role questions from a different read than the one that built the data.
- `crates/shamir-engine/src/query/batch/query_runner.rs` around :331-358 and :431-456: the warm-up and the role lookup are two separate reads.

Required result:
1. `try_publish` returns the published `Arc<VersionedState>` on CAS success. A new `get_or_build_snapshot(build) -> Result<FkSnapshot, E>` has a fast path `load_full()` when the cache is populated; `by_parent`, `is_fk_child`, `is_parent_with_{delete,update}_action` all derive from that ONE snapshot. Remove the cold "peek" flags.
2. Migrate the callers: `query_runner.rs` (the two spots above), `fk_restrict.rs` (~:194), `fk_actions.rs` (~:1072), `fk_on_update.rs` (~:788).
3. Keep single-flight and the generation CAS exactly as they are; never hold a sync guard across `.await` (an arc-swap `Guard` must not live across an await: use `load_full()`).
4. Red tests, deterministic via the existing test-hook style of this module (look at how `fk_reverse_cache_race_tests.rs` parks a thread/task): (a) a hook `TEST_POST_PUBLISH_PRE_RETURN_HOOK` that stops after a successful publish, lets the test call `invalidate()`, then continues -> the returned result must equal the built set (today it is empty); (b) a hook between warm-up and role lookup in `implicit_tx_isolation_for_fk_parent` (the result must be Serializable) and in `require_footprint_if_fk_child` (the footprint is required).
5. B08 (a separate task, another agent) is logically related (`collect_fk_refs` skips a missing artifact); do not touch the validator registry.
