# Brief — B03 index2 read paths keep query semantics

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

- `crates/shamir-engine/src/table/read_exec.rs` — ONLY the index2 block (~:476-591), the filtered-vector block (~:1714-2091) and the gates around ~:1706-1712, :2079-2087, :2375; do NOT touch the context-aware projection sites (merged in B14)
- `crates/shamir-engine/src/table/read_planner.rs` (~:80-83, vector lease error)
- a NEW small file next to them for the eligibility function / `CandidateSet` if that keeps `read_exec.rs` readable (one primary export per file)
- tests: NEW topic files in `crates/shamir-engine/src/table/tests/` (+ one-line `pub mod`)
- NO tx/WAL/drainer/commit files, NO `query/batch/*`, NO shamir-query-types changes

## Task

B03 (GitHub issue #40, `gh issue view 40`; task #1256) — the index2 read paths must keep the full query semantics. B14 (resolved refs/params in every projection path) is merged on master, your base.

Observed on master (verify each line first): in `read_exec.rs` ~:476-591 the index2 shortcut returns candidate rows without applying the SELECT projection (~:531), and ignores COUNT/aggregates, DISTINCT and explicit ORDER BY; `read_planner.rs` ~:80-83 turns a vector-lease error into a FULL SCAN that returns every row; the filtered-vector path (~:1714-2091) has the same gaps plus pagination/`with_version` handling.

Design (follow it):
1. Split candidate retrieval from post-processing: `try_plan_index2` yields a `CandidateSet { rids, ranked }`; the post-processing (page -> `SelectProjection` -> corrupt-record accounting / versions) is shared with the normal read path.
2. An explicit `index2_shortcut_eligible(query)` that is true ONLY without aggregates, GROUP BY, DISTINCT and an explicit ORDER BY. When eligible: page the candidates, then project (with the B14 context), then corrupt/versions. Otherwise: stream the candidates into a new `read_collecting_from(stream)` (the normal collecting pipeline fed by the candidate stream) so COUNT/SUM/DISTINCT/ORDER BY/LIMIT/OFFSET behave exactly as on an unindexed table. Without an explicit ORDER BY keep the candidate rank order.
3. The same gate for the filtered-vector path: skip/take, `with_version` -> versions or a typed Validation error (pick the one that is consistent with the unindexed path; say which).
4. `read_planner.rs` ~:82: a vector-lease error returns a typed error, NEVER a full scan.
Red tests (DIFFERENTIAL: the same data in an indexed table and an unindexed table must give identical results; write them so they fail on the old code): `SELECT title` (projection applied); `COUNT(*) WHERE fts-match` (returns the count, not N rows); `SUM`; `DISTINCT`; `ORDER BY` + `LIMIT/OFFSET`; empty candidates with COUNT -> `{count: 0}`; a corrupt candidate -> `corrupt_records` equal on both tables; vector search + explicit ORDER BY; filtered-vector + COUNT / DISTINCT / pagination; vector-lease error -> typed error. Use the builder crate for queries.
Report the eligibility rule you implemented and any shape you could not make identical.
