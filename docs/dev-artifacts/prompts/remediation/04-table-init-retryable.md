# Brief — B09_table_init_retryable

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

- `crates/shamir-engine/src/repo/repo_instance.rs`
- tests under `crates/shamir-engine/src/repo/tests/` (a new topic file such as `table_init_retry_tests.rs` plus its one-line `pub mod` in that directory's `mod.rs`)

## Task

B09 (GitHub issue #39, `gh issue view 39`) — failed or cancelled table initialization must be retryable.

Observed on master: `RepoInstance::create_table_context` (`repo_instance.rs`, around :382-450) publishes the `per_table_mvcc` registration (around :411-429) BEFORE later fallible `.await` steps (around :440, :442, :447). A failure or a cancellation at those points leaves the OnceCell empty but the MVCC registration present, so the retry fails with an "attachment collision". Only `remove_table` (around :540) rolls the registration back.

Required result:
- Make table-context publication atomic from the callers' point of view. The narrowest fix: reorder `create_table_context` so that everything fallible and async (stores, gate, `fully_unwrap_store`, `TableManager::create`, repo interner, `MvccStore::new`) happens first and the `per_table_mvcc.insert_sync(...)` is the LAST fallible step, followed only by synchronous builder calls (`with_mvcc_store`, `with_interner`, `with_changefeed`, ...). After the insert re-check that the table config still exists; if the table was dropped meanwhile, remove the registration with an ownership check (`remove_if_sync(token, |v| Arc::ptr_eq(v, &mine))` or equivalent) and return NotFound. NEVER remove a registration owned by a racing successful initializer. `remove_table` must only remove the mvcc of its own initialized cell (ptr_eq). The A13 split-brain protection (never two MvccStores for one table) must stay intact. Do not hold an scc entry/bucket guard across `.await` (project rule).
- Review every publication point and the remove/rename races touching `per_table_mvcc`.
- Tests (deterministic): (1) one-shot injected failure after the old registration point (use a test-only seam such as a `OnceLock<AtomicBool>` hook in the style of the existing `TEST_READ_ONE_TX_BYTES_FAILURE`, or a failing store wrapper for the `__info__` store): the first `get_table` returns Err; a second `get_table` succeeds (today: Internal attach collision) and `per_table_mvcc[token]` is `Arc::ptr_eq` to the returned table's `mvcc_store`; (2) cancellation: park the init on a Notify/Barrier (e.g. in the interner store), abort the task, then retry succeeds; (3) race: init1 parked, `remove_table` + re-add + init2 completes, init1 resumes -> init2's registration is untouched and exactly one coherent TableManager/MvccStore exists.
