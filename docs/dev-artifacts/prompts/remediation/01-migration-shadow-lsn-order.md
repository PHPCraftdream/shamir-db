# Brief — #1218_shadow_log_LSN_order

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

- `crates/shamir-engine/src/migration/coordinator.rs`
- `crates/shamir-engine/src/migration/tests/coordinator_tests.rs` (and, only if a new test file is cleaner, a new file in `crates/shamir-engine/src/migration/tests/` plus its one-line `pub mod` in that directory's `mod.rs`)

## Task

#1218 — preserve LSN order in `MigrationCoordinator::apply_shadow_entries`.

Observed: `crates/shamir-engine/src/migration/coordinator.rs` (around lines 226-249) splits a shadow-log drain batch into a `puts[]` array and a `deletes[]` array and then runs ALL `set_many` followed by ALL `remove_many`. That is not equivalent to a sequential replay by LSN. For one key, `Delete -> Put` becomes `Put` then `Delete`, leaving the record absent although it must exist; `Put -> Delete -> Put` breaks the same way.

The existing mixed-drain test uses DIFFERENT record ids for put and delete, so it never checks same-key ordering, and its oracle ("exactly one set_many and one remove_many") pins the wrong behaviour. Revise that oracle instead of preserving it as a contract.

Required result:
1. Apply the entries in LSN order. Adjacent operations of the same kind may be coalesced into one `set_many` / `remove_many` batch ONLY when this is provably equivalent to the sequential replay (consecutive puts of a run, consecutive deletes of a run; a run boundary is any change of kind). Two puts to the same key inside one run must keep last-writer-wins by LSN.
2. Tests (deterministic, no wall clock): (a) explicit same-key sequences `Delete->Put`, `Put->Delete`, `Put->Delete->Put`, `Delete->Put->Delete`; (b) a model/property test: random interleavings of put/delete over ONE key and over several keys, compared against a trivial sequential model (a map updated entry by entry in LSN order) — use a small deterministic LCG with fixed seeds (no external crates), many iterations; (c) the revised mixed-drain oracle.
3. Keep the public behaviour otherwise unchanged (error handling, shadow-log clearing, state transitions).
