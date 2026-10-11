# Brief — #1208 idle reaper lease

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

- `crates/shamir-server/src/tx_registry.rs`
- `crates/shamir-server/src/db_handler/tx_handlers.rs` (ONLY the activity/lease handling around TxExecute; do not touch the admin gate that another task just rebuilt on `BatchTreeVisitor`, and do not add a deadline here: task B13 owns the TxExecute deadline later)
- tests: `crates/shamir-server/src/tests/tx_registry_tests.rs` and/or a new topic file in `crates/shamir-server/src/tests/` (+ its one-line `pub mod` in `tests/mod.rs`)

## Task

#1208 — idle-reaper vs a running interactive transaction (P0).

Observed on master: `InteractiveTx::bump_activity` is called only AFTER a successful TxExecute (`crates/shamir-server/src/db_handler/tx_handlers.rs`, around :165-170). The idle reaper (`crates/shamir-server/src/tx_registry.rs`, around :254-321) decides expiry by timestamp and removes the registry entry without knowing that a request is currently executing. If a TxExecute runs longer than the idle TTL the reaper can remove the entry while the request still holds its own Arc / tx context: the request returns a successful TxBatch, but the following Commit gets `tx_not_found` and the transaction is rolled back on drop.

Required result:
1. A cancellation-safe in-flight lease: mark activity at the START of an operation and hold a lease (RAII guard, released on every exit path including cancellation/panic via Drop) for its whole duration; the reaper must SKIP entries that have an active lease (and a lease acquisition must refresh the activity timestamp, a lease release must refresh it again so the idle clock restarts from the end of the request). Use atomics (an `AtomicUsize` lease counter plus the existing timestamp atomics); no mutex on the request path. Race: the reaper's "check lease == 0 then remove" must not remove an entry whose lease was acquired in between — use the registry's existing removal primitive with a conditional/ptr-equality removal and re-check the lease inside it (look at how `remove_if`-style removal is done for other registries in this repo, e.g. `scc::HashMap::remove_if_sync`), or an equivalent CAS on a state word.
2. Max-tx-lifetime reaping stays as it is: a lease protects against IDLE expiry only, not against the absolute lifetime cap.
3. Red tests, deterministic with paused tokio time (`#[tokio::test(start_paused = true)]`, `tokio::time::advance`) and no real sleeps: (a) a TxExecute-like operation holds a lease for longer than the idle TTL while the reaper runs repeatedly -> the entry is NOT reaped, and a Commit afterwards succeeds; (b) after the lease is dropped the idle clock restarts: the entry survives until a full idle TTL after the lease release, and is then reaped; (c) a lease dropped by task cancellation (abort the task) releases correctly; (d) the lifetime cap still reaps a leased entry. Drive them against the registry API directly (and, if a small handler-level test is feasible with the existing fixtures, once through the real handler).
