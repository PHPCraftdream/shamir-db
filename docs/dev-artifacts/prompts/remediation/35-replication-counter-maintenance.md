# Brief — S4 follower counter maintenance

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

- `crates/shamir-engine/src/tx/apply_replicated.rs` (preflight rejection of a table with indexes; per-table pre-image counting; counter increment) and the engine apply tests (`crates/shamir-engine/src/tx/tests/apply_replicated*_tests.rs`, NEW topic file for the counter, + one-line `pub mod`)
- `crates/shamir-server/src/replication/tests/read_after_replication_tests.rs` — only to un-ignore the rows marked `#[ignore = "#1212"]`
- docs: the replication limitation text that says follower indexes/counters are not maintained (README / `KNOWN_LIMITATIONS` / the replication guide — Grep for it) updated to the new behaviour
- NO changefeed / WAL / drainer / commit files

## Task

S4 (task #1212, issue #48) — the follower maintains the table counter on replicated apply and refuses tables with indexes. Your worktree is built on S3 (PR #90: replicated records are translated by name; `apply_replicated` takes field bindings) and contains the read-after-replication matrix; read `apply_replicated.rs` as it is in your tree first.

Broken: the leader derives indexes and the counter from `tx.index_write_set` / `tx.counter_deltas` (`materialize.rs` ~:107-137, :155-178), the changelog event carries neither (`changefeed.rs` ~:440-442); the follower writes only data via `apply_committed_ops`. So index lookups miss replicated rows and `SELECT COUNT(*)` (`read_exec.rs` ~:431-432 reads the counter) returns 0.

Design (decided):
(a) INDEXES: fail closed in the preflight — a follower table that has ANY index makes the event Rejected (typed, with the table and index named) + documentation. Full index maintenance is a later project (through the commit pipeline, after B02 step 2). Do not try to maintain indexes here.
(b) COUNTER, best effort like the leader: per table take pre-images with `mvcc.get_current_many` (`mvcc_store/mod.rs` ~:1536); delta +1 for a Put of a record that did not exist, -1 for a Delete of an existing record, else 0 (last op per key in the event); apply the data; then `tbl.counter().increment(delta)` before moving to the next table. A retry counts from the ACTUAL state, so an already applied event has delta 0 (issue #48 converges). Do NOT use `apply_counter_phase` (deleted by PR #69).
Red tests: leader 3 inserts + 1 delete -> follower `COUNT(*)` must be 2 (today 0); a failure injected on the second table of a multi-table event, then retry -> exact counts on both tables; replaying the same event twice leaves the counter unchanged; a table with an index is Rejected with nothing applied; the five matrix rows ignored with `#1212` are UN-IGNORED and must pass (report any that does not).
