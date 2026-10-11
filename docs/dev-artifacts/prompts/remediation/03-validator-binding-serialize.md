# Brief — B07_validator_binding_serialization

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

- `crates/shamir-engine/src/table/table_manager.rs`
- `crates/shamir-engine/src/table/table_manager_validators.rs`
- tests under `crates/shamir-engine/src/table/tests/` (a new topic file such as `validator_binding_serialization_tests.rs` plus its one-line `pub mod` in `tests/mod.rs`)
- If, and only if, call sites outside these files must change to keep compiling after removing `bindings_len` (search the whole workspace with Grep), make the minimal call-site edit there and list it in the final message.

## Task

B07 (GitHub issue #38, `gh issue view 38`) — validator binding read -> modify -> persist -> publish must be serialized per table.

Observed on master: `table_manager_validators.rs` (around :40-60 and :69-85) loads the `validator_bindings` ArcSwap snapshot, clones it, awaits the persist to the info store and then stores the new snapshot, with no lock. Two concurrent bind/bind or bind/unbind operations clone the same snapshot, persist competing lists and then overwrite each other while both report success. `bindings_len` is published separately (lines :58 and :82), so the fast-path metadata and the snapshot can disagree. The bind path in `crates/shamir-server/src/db_handler/admin_validator.rs` (around :210, :263) does not take the schema lock either (out of your scope, do not edit it).

Required result:
- `TableManager` gets a per-table `bindings_lock: Arc<tokio::sync::Mutex<()>>` shared by all clones of the TableManager (same sharing as `bindings_len`). This is the sanctioned exception "guard held across .await, DDL only" — put a one-line comment naming the contention model. Serialize the full load -> modify -> persist -> publish lifecycle under it. Do not hold unrelated map guards across awaits.
- Publish coherently: remove the separate `bindings_len` atomic and derive "no bindings" from the same `validator_bindings` snapshot (`validator_bindings.load()` is a guard, no Arc clone on the hot path). Update every user of `bindings_len`.
- Tests (deterministic, pausing store, no sleeps): wrap the table's info store with a store that parks the persist call on a barrier/Notify. Two concurrent `add_validator_binding(a)` and `add_validator_binding(b)` on clones of the same TableManager: while A is parked, B must not finish (poll once / `now_or_never`). After A is released both complete; the LIVE binding set is `{a, b}`, the PERSISTED set (reload via `load_validators_metadata`) is `{a, b}`, and a write to the table actually invokes BOTH validators (count invocations with two counting native validators). Variant: bind/unbind interleaving.
