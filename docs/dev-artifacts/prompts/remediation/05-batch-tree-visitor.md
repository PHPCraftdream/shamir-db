# Brief — #1206_BatchTreeVisitor

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

- `crates/shamir-query-types/src/batch/` : a NEW file `tree_visitor.rs` (one primary export: the iterative visitor and its small helper types), the one-line `mod`/`pub use` additions in `crates/shamir-query-types/src/batch/mod.rs`, and edits to `batch_op.rs`, `query_entry.rs` (`distinct_repos`, `collect_required_access`) and `planner.rs` (ONLY `max_nesting_depth_of_ops` and its callers, to run on the visitor)
- tests for the visitor in `crates/shamir-query-types/src/batch/tests/` (new topic file + one-line `pub mod` in that directory's `mod.rs`)
- `crates/shamir-server/src/db_handler/handler.rs` (the coarse admin gate and any other whole-batch classification)
- `crates/shamir-server/src/db_handler/tx_handlers.rs` (the same coarse admin gate for TxExecute; do not change its deadline / activity logic, another task owns that)
- `crates/shamir-server/src/db_handler/admin.rs` (`check_destructive_hmacs` and the nested-op expansion it needs)
- server-side tests: `crates/shamir-server/src/db_handler/tests/` or `crates/shamir-server/tests/` (new topic file, wired through the existing manifest/pattern of that directory)

## Task

#1206 — one iterative `BatchTreeVisitor` used by EVERY policy check on the OQL operation tree (P0, security).

Observed on master:
- `BatchOp::is_admin()` (`crates/shamir-query-types/src/batch/batch_op.rs`, around :577) does not include `ForEach`, so an admin or destructive operation wrapped in a `ForEach` passes the coarse admin gate in `crates/shamir-server/src/db_handler/handler.rs` (around :512-517) and in `tx_handlers.rs` (around :107-120).
- The destructive-HMAC check (`check_destructive_hmacs`, `crates/shamir-server/src/db_handler/admin.rs`, around :637-758) also does not look into nested `Batch` / `ForEach` bodies.
- The asymmetry goes both ways: a nested `Batch` that contains only data operations is wrongly treated as admin as a whole.
- `collect_required_access` and `distinct_repos` (`query_entry.rs`, around :93-151) and `BatchPlanner::max_nesting_depth_of_ops` (`planner.rs`, around :758-790) each walk the tree on their own, recursively, with no shared definition.

Required result:
1. One shared, ITERATIVE (explicit stack / worklist, no recursion) visitor over the operation tree in `shamir-query-types`, visiting every query entry at every nesting level of `Batch` and `ForEach` bodies with enough context for the callers (at least: the entry, its alias path or a stable alias, nesting depth). Container operations (`ForEach`, nested `Batch`) are NOT admin by themselves — policy is decided on the leaf operations found by the visitor; this removes the false positive for data-only nested batches and closes the bypass.
2. Rebase ALL of these on the visitor: the coarse admin gate (handler.rs and tx_handlers.rs), the destructive-HMAC check (admin.rs, every nesting level), read-only/write classification of a whole batch (find existing whole-batch classifications with Grep, e.g. users of `is_write`), ACL collection (`collect_required_access`), repo collection (`distinct_repos`), nesting-depth computation. Keep their public signatures and results for flat batches unchanged.
3. The visitor itself adds NO new limits (another task adds server-owned hard limits on top of it); it must however not recurse, so a deeply nested tree cannot overflow the stack.
4. Tests, all deterministic:
   - visitor unit tests (query-types): visits all levels in a mixed Batch/ForEach tree, correct depth, iterative on a deep chain (use a depth that would overflow a recursive walker under the default 2 MiB test-thread stack but is still cheap to build; keep construction and drop of the test tree itself non-recursive-safe, e.g. pick a depth the std recursive Drop can handle, or build and `std::mem::forget` the tree; explain the choice in a comment);
   - server tests through the real handler path: a non-superuser sending `DropTable` (or another admin op) wrapped in one `ForEach`, in three nested `ForEach`/`Batch` levels, and via the TxExecute path -> `permission_denied`; a superuser sending a destructive op nested in `ForEach` WITHOUT the required HMAC -> the HMAC error, and WITH a valid HMAC -> accepted; a non-superuser sending a nested `Batch` that contains only data operations -> NOT rejected by the coarse admin gate.
   - If executing such a batch hits a stack overflow on the default test thread stack, run that test on a `std::thread::Builder` thread with a larger stack (do not change global runtime configuration in this task).
