# Brief — B04 tx-aware read shortcuts

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

- `crates/shamir-engine/src/table/read_exec.rs` — the shortcut gate in `read_impl` (counter, min/max, index2 / `index2_shortcut_eligible`, base-index, sorted, keyset, order-limit shortcuts: Grep each) and the transactional read entry
- `crates/shamir-engine/src/table/table_manager_streaming.rs` (`list_stream_tx` / `filter_stream_tx`, ~:140-143), `crates/shamir-engine/src/table/table_manager_crud.rs` (`get_many_bytes_tx`, ~:751-752)
- the MVCC read primitives you call (`crates/shamir-tx/src/mvcc_store/mod.rs` `snapshot_stream` ~:1354, `get_at_many` ~:1141): call them; edit only if a tiny additive helper is missing (list it)
- tests: NEW topic files in `crates/shamir-engine/src/table/tests/` (+ one-line `pub mod`)
- NO commit/WAL/drainer files, NO query/batch files, NO shamir-query-types

## Task

B04 (task #1257; GitHub issue #41, `gh issue view 41`) — read shortcuts must be transaction-aware. B03 (index2 paths keep full query semantics, `index2_shortcut_eligible`, `CandidateSet`, `read_collecting_from`) is merged on master, your base: read it first (`read_exec.rs`, `candidate_set.rs`).

Problem: no shortcut in `read_exec.rs` honours the transaction (counter shortcut ~:431-447, min/max ~:376-421, index2 ~:476-591, base index ~:595-679, sorted ~:685-831, keyset ~:836-893): inside a Snapshot/Serializable transaction they read latest committed state, ignoring the staged overlay and the snapshot; and the tx path itself does not pin the snapshot (`table_manager_streaming.rs` ~:140-143 `current_stream`, `table_manager_crud.rs` ~:751-752): a transaction that took its snapshot before another transaction committed still sees the later commit through COUNT, an index lookup or even a full scan.

Design (decided):
1. In `read_impl` with `tx.is_some()` and isolation Snapshot or Serializable: DISABLE ALL shortcuts (counter, min/max, index2, base index, sorted, keyset, order-limit) and use the transactional path. Pessimistic isolation stays latest-committed (the A4 design: locks, not snapshots). Non-transactional reads keep every shortcut.
2. Make the transactional path snapshotted: `list_stream_tx` / `filter_stream_tx` use `mvcc.snapshot_stream(batch, tx.snapshot_version)` and `get_many_bytes_tx` uses `get_at_many`, merged with the transaction's staged overlay (own staged inserts/updates/deletes win). Read your own writes must keep working (existing tests).
3. SSI predicate recording in `read_tx` is UNCHANGED (a Serializable read still records its predicate and conflicts are still caught).
4. Document the cost (tx reads become scans) in one comment at the gate.
Red tests: for EACH shortcut class x {staged insert, staged update, staged delete} x {Snapshot, Serializable}: `read_tx` equals the oracle "filter over snapshot + overlay" (build the oracle independently, e.g. by reading the same rows through the plain unindexed full-scan path); tx1 takes a snapshot, tx2 commits an insert/update/delete -> tx1 sees nothing of it via COUNT, an indexed lookup and a full scan (today it does); a Serializable predicate read still records its predicate and the conflicting commit is rejected; a Pessimistic transaction still takes its locks and reads latest committed. Keep the tests fast and deterministic; use the builder crate for queries.
Do NOT run benches (state that the read benches are owed: tx reads are now full scans by design).
