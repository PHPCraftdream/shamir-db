# Brief — B05_FK_final_staged_state

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

- `crates/shamir-engine/src/validator/validator_db.rs`
- `crates/shamir-engine/src/validator/tests/validator_db_tests.rs` (or a new topic file in `crates/shamir-engine/src/validator/tests/` plus its one-line `pub mod` in that directory's `mod.rs`)

## Task

B05 (GitHub issue #36, readable with `gh issue view 36`) — foreign-key existence checks must observe the transaction's FINAL staged state.

Observed on master: `ValidatorDb::exists_in_table` (`validator_db.rs`, around :228-361) accepts a committed index hit immediately (:245-252 returns true), the full-scan fallback (:296-318) ignores staged deletes/updates, and the staged probe (:326-330, :344-361) only looks at staged `Set` operations. One transaction can therefore delete a parent row and insert a child referencing it, or change a parent key and reference the old value, and commit-time RI conflict detection does not reject the transaction's own final-state violation.

Required result:
- FK existence evaluates the committed snapshot PLUS this transaction's net staged overlay.
- Indexed path: for every id returned by `lookup_by_index` consult `staging.staged_op(id)` (`staging_store.rs:163`, O(1)): staged `Removed` -> skip that candidate; staged `Set` -> re-check the referenced field on the STAGED bytes; no staged op -> the candidate is valid. For FK parents on a unique key this stays O(1); do not introduce an O(M*N) regression.
- Scan fallback: skip rids that have a staged op, or read through the tx-aware stream that already applies the masks; staged inserts/sets still count as existing.
- The staged probe resolves field names through the layered interner exactly like `exists_in_self` (around :462-469).
- Errors stay fail-closed (an undecodable row still fails the check; the existing test `exists_in_table_fails_closed_on_decode_corrupt_row` must keep passing).
- Tests: for {table with an index on the referenced field, table without} x {Snapshot, Serializable}: (1) delete parent `uid=1` then insert a child referencing 1 in the SAME tx -> the FK check refuses; (2) update parent key 1 -> 2: a child referencing 1 is refused, a child referencing 2 is accepted; (3) regression: inserting parent and child in one tx (the earlier FG-3 case) still passes. Use the existing fixtures of `validator_db_tests.rs` / the nearby FK tests as the template for building the tx and tables.
