# Brief — B04b snapshot-consistent vector reads and _id keyset in transactions

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

- `crates/shamir-engine/src/table/read_exec.rs`: ONLY the snapshot gate at the top of `read_impl` (the Snapshot/Serializable branch and its vector exemption), `read_snapshot_keyset`, and the vector read paths (`read_filtered_vector_scan` and the bare VectorSimilarity / index2 vector branch)
- `crates/shamir-engine/src/table/filtered_vector.rs`
- `crates/shamir-tx/src/mvcc_store/` : only a tiny ADDITIVE accessor if a per-table "last committed version" is missing (LIST it in your report)
- tests: NEW topic files in `crates/shamir-engine/src/table/tests/` (+ one-line `pub mod`)
- NO commit/WAL/drainer files, NO query/batch files, NO other read_exec.rs regions (another agent edits the row sinks `read_streaming` / `read_collecting` / `read_counting` in parallel - do not touch them)

## Task

B04b (task #1288; follows B04 / PR #106 merged on master = your base; read B04 `read_impl` snapshot gate, `read_snapshot_keyset` and `crates/shamir-engine/src/table/tests/tx_read_shortcut_tests.rs` first).

State today: inside a Snapshot or Serializable transaction all latest-state shortcuts are disabled and reads scan at the tx snapshot (`list_stream_tx_after_predicate` -> `mvcc.snapshot_stream(.., tx.snapshot_version)` + staged overlay). Two gaps remain:

1. Vector similarity (bare `VectorSimilarity` and the filtered-ANN shape `And([VectorSimilarity, ..residual])`) is deliberately EXEMPTED from the gate (`contains_vector_similarity`): it still runs ANN on the LATEST committed index and merges the staged vectors of the tx, so its candidates are not snapshot-consistent (a vector committed after the snapshot can appear; one deleted after it can be missing; an updated one is ranked by its new vector).
   Required: make vector reads inside Snapshot/Serializable snapshot-consistent. Design (decided; deviate only if you find it unsound and say why): ANN on the committed index is valid for the snapshot iff no commit touched the table after `tx.snapshot_version`. Add or locate a cheap per-table "last committed version" (MVCC store / table gate; if it does not exist add the smallest additive accessor and LIST it). If the table is unchanged since the snapshot -> keep today ANN + staged-overlay path. If it changed -> exact fallback: scan the snapshot (`snapshot_stream` at `tx.snapshot_version`), merge the staged overlay, compute the exact distance per row with the SAME metric and normalisation the index uses, apply the residual filter, top-k (document the O(N*d) cost in one comment). The staged-overlay semantics of the existing tests (`filtered_ann_tests.rs`: tx_staged_vector_visible_and_filtered, vr5_*) MUST keep passing unchanged.
2. Synthetic `_id` keyset ordering inside Snapshot/Serializable is rejected by `read_snapshot_keyset` (`resolve_field` only walks stored maps). Support `_id` as an order key there by comparing the RecordId directly (same total order the sorted index uses for the id tie-breaker); remove the validation error and its test expectation, add positive tests (ASC and DESC, with and without `after_id`).
Red tests (deterministic, small fixtures, builder crate for queries): tx1 takes its snapshot, tx2 commits (a) a new nearest vector, (b) a delete of the nearest vector, (c) an update that moves a vector -> the vector read of tx1 equals the oracle "exact top-k over the snapshot + staged overlay" in every case, for Snapshot and Serializable; the unchanged-table case still reports the ANN path (assert via `stats.index_used` or explain); a Pessimistic transaction keeps latest-committed behaviour; non-tx vector reads are unchanged.
Do NOT run benches (state that vector read benches are not affected on the non-tx path).
