# Brief — B11 bound pagination and Pull preallocations

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

- engine: `crates/shamir-engine/src/query/read/order.rs`, `crates/shamir-engine/src/table/read_index_scan.rs`, `read_asof_seek.rs`, `read_temporal.rs`, `crates/shamir-engine/src/query/batch/batch_validate.rs` (pagination ceiling check only), `crates/shamir-engine/src/repo/changelog_store.rs`
- `crates/shamir-query-types/src/read/limit.rs`, and `crates/shamir-query-types/src/batch/batch_error.rs` (ONLY the new `PaginationTooLarge` variant + its wire/Display plumbing)
- `crates/shamir-tx/src/changefeed.rs`
- server: `crates/shamir-server/src/db_handler/repl_handler.rs`, `crates/shamir-server/src/replication/in_process.rs`, and the server limits config (`crates/shamir-server/src/config.rs` / `db_handler/config.rs`: ONLY the new pagination ceilings next to the existing query limits)
- tests: new topic files in the matching `tests/` directories (+ one-line `pub mod` each); a new integration test file `crates/shamir-engine/tests/prealloc_caps.rs` with a counting `#[global_allocator]`

## Task

B11 (GitHub issue #44, readable with `gh issue view 44`) — bound pagination and replication Pull preallocations.

Observed on master: client-controlled LIMIT / OFFSET / page values are used to size allocations or are multiplied unchecked: `order.rs` ~:230 `with_capacity(k+1)`; `read_index_scan.rs` ~:436 and ~:527; `read_asof_seek.rs` ~:209; `read_temporal.rs` ~:184; `limit.rs` ~:180 unchecked `*`; `repl_handler.rs` ~:165 and `in_process.rs` ~:96 take the Pull `limit` without a cap; `changelog_store.rs` ~:53 `range_from`.

Required result:
1. Server-owned ceilings `max_page_size` and `max_offset` next to the existing query limits (look at how `BatchLimits` / the server `query_limits` are defined and wired; follow that style, keep defaults generous but finite). `batch_validate` enforces them for EVERY Read, including those inside nested `Batch` / `ForEach` bodies (iterate the tree in a non-recursive way or reuse the existing iterator there; another task is introducing a shared `BatchTreeVisitor` in `shamir-query-types` concurrently — do NOT depend on it and do not touch the planner). New typed error `BatchError::PaginationTooLarge`.
2. Engine: `TopKHeap::new` uses `checked_add` and `with_capacity(k.min(PREALLOC_CAP))`; same cap in `read_index_scan.rs` (~:436, ~:527) and `read_asof_seek.rs` (~:209) and `read_temporal.rs`; `limit.rs` ~:180 gets a `try_resolve()` with `checked_mul` returning a typed error instead of overflowing.
3. Pull: `limit.clamp(1, MAX_PULL_EVENTS)` in `repl_handler.rs` and `in_process.rs`; `ChangelogStore::range_from` gets a `max_bytes` bound and `with_capacity(limit.min(1024))`; clamp `wait_ms` (~:190-193).
4. Red tests, deterministic: (a) on EMPTY data, `LIMIT`/`OFFSET = u64::MAX` and `Page{u64::MAX, u64::MAX}` on full-scan ORDER BY, sorted order-limit, keyset, base-index scan and AsOf -> a typed error or a bounded response, never an allocation failure/abort; (b) `crates/shamir-engine/tests/prealloc_caps.rs` with a counting `#[global_allocator]` (single-threaded test binary section or a guard so other tests do not skew it): the largest SINGLE allocation stays below 64 MiB; (c) `range_from(usize::MAX)` -> `Ok(empty)`; (d) Pull with `u32::MAX` -> bounded events and bytes.
