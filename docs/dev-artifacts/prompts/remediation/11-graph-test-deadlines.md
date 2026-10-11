# Brief — #1263 graph tests fail fast

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

- `crates/shamir-index/src/vector/tests/quantized_graph_tests.rs` (and, if cleaner, a new helper file in `crates/shamir-index/src/vector/tests/` plus its one-line `pub mod` in that directory's `mod.rs`)
- `crates/shamir-index/src/vector/hnsw_adapter.rs` — COMMENTS ONLY (module doc lines ~3-5 and the RNG comment at ~49-52); no code change in this file

## Task

#1263 — graph-insert async tests: a bounded timeout must FAIL the test fast instead of hanging until the runner's 180 s kill.

Observed (CI probe run 38128366691, ubuntu): `quantized_graph_tests::concurrent_upsert_with_tombstone_across_fit_does_not_hang` has a `tokio::time::timeout(60 s)` that fires and asserts, but dropping the `#[tokio::test]` runtime then calls `BlockingPool::shutdown`, which waits forever for the stuck `spawn_blocking` graph insert (the hung thread is visible in the dump as the test thread parked in `tokio::runtime::blocking::shutdown::Receiver::wait`). The result is a 180 s nextest TIMEOUT instead of a 60 s failure. The test file already has `*_body` async fns, `phase()` stderr markers and a `within_deadline` helper (120 s) from an earlier diagnostic change; read the file first.

Required result:
1. For every test in that file that runs graph inserts through `spawn_blocking` (the concurrent upsert tests, the recall tests, anything using `upsert` / `upsert_batch` under a deadline): build the runtime BY HAND (`tokio::runtime::Builder::new_multi_thread()` with the same worker count the test used, `enable_all()`), `block_on` the body under the deadline, and on every exit path call `runtime.shutdown_timeout(Duration::from_secs(5))` so a stuck blocking task cannot hold the process. Keep the tests deterministic and keep their assertions; the deadline panic message must name the test/phase.
2. One small shared helper (no macro soup) so each test is a short wrapper over its existing `*_body`.
3. Comments only in `hnsw_adapter.rs`: the module doc (~lines 3-5) claims the graph is "internally thread-safe" for concurrent `insert`; rewrite it to say that concurrent inserts are safe only because the workspace builds the vendored `hnsw_rs` (`vendor/hnsw_rs`, lock-order fix, see its VENDORED.md); and the comment at ~49-52 says hnsw_rs has an unseedable RNG, which is stale (0.3.5 seeds its layer generator with a fixed seed 397): correct it. Do not change any code in that file.
4. Do not change what the tests assert about recall or counts.
