# Brief — B08 ValidatorRegistry atomic lifecycle

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

- `crates/shamir-engine/src/validator/registry.rs`
- `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs`
- tests: `crates/shamir-engine/src/validator/tests/` (new topic file `registry_atomic_tests.rs` plus its one-line `pub mod` in `tests/mod.rs`; or extend the existing registry test file)

## Task

B08 (GitHub issue #42, readable with `gh issue view 42`) — `ValidatorRegistry` lifecycle must be atomic (name / id / artifact / bound_in).

Observed on master: `crates/shamir-engine/src/validator/registry.rs` :74-96 publishes the NAME before the artifact is in place; :126-144 and :150-161 are multi-step removals/renames; :156 `remove` wipes `bound_in`; "replace" in `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs` :144-153 and :302-313 is `remove` + `register`, so a concurrent reader can see a missing validator and a bound validator can lose its binding record.

Required result:
1. Replace the independent maps by one immutable state behind `arc_swap::ArcSwap<RegistryState>`, `RegistryState { by_id: TFxMap<RecordId, Entry{name, artifact, bound_in}>, name_to_id }` (use the workspace `THasher`-based map types, look at how neighbouring registries do it), with a writer serialization `std::sync::Mutex<()>` (sanctioned category 2 in CLAUDE.md: DDL-only, never held across `.await`, sync ops only; add the one-line "DDL op, contention is nil" comment). Every mutation = lock -> clone state -> modify -> publish. `get_by_id` and all reads stay lock-free.
2. New `replace(id, name, artifact)` that swaps the artifact atomically and KEEPS `bound_in`; `validator_management.rs` uses it instead of `remove` + `register`.
3. Red tests with deterministic parking (test hooks inside the operations plus `std::sync::Barrier` on real threads; no sleeps): (a) `register` parked after name publication (today: a reader resolves the name to nothing); (b) `rename` parked after removing `from` and registering `from`; (c) `remove` racing `rename`; (d) after quiescence every name in `list()` resolves to a live artifact and `name_for_id` returns that same name; (e) replacing a BOUND validator keeps it bound (cannot be dropped as unbound).
4. Public API of the registry stays source-compatible for existing callers except where a caller must move to `replace`.
