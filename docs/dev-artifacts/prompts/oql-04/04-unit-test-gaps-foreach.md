# Epic04/Phase D — unit test gap-closure for loops (#655)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`BatchOp::ForEach` (Epic04) is fully implemented: engine (Phase B, commit
`6ff521d5`) and Rust+TS builders (Phase C, commit `79510a13`). Existing test
coverage:

- `crates/shamir-query-types/src/batch/tests/for_each_op_tests.rs` — serde
  round-trip + dispatch-by-wire-key (4 tests).
- `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` — 8 executor
  tests: zero iterations, one iteration binds row, N-iteration accumulation,
  max_iterations exceeded (dynamic `over`), tx-abort-on-error, `over` as a
  `$query` column ref, `over` as an `$fn` call, and the "resolves exactly
  once, not per-iteration" guarantee.
- `crates/shamir-query-builder/src/batch/tests/for_each_tests.rs` — 5 builder
  tests (literal/`$query`/`$fn` `over`, wire-shape regression guard, escape
  hatch parity).

This phase closes the remaining gaps, which are the parts of
`docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`'s 5 decisions that
are NOT yet covered by a test. Read the ADR first — it is short and precise
about the exact semantics each of these tests must lock in.

## Gaps to close

### 1. Planner-level tests (`crates/shamir-query-types/src/batch/planner.rs`)

New test file `crates/shamir-query-types/src/batch/tests/for_each_planner_tests.rs`
(wire into `crates/shamir-query-types/src/batch/tests/mod.rs`). Cover:

- **Dependency extraction from `over`.** A `ForEach` whose `over` is a
  `$query` ref to alias `A` must show up in the DAG as depending on `A` (same
  mechanism the ADR's Decision 1 describes — "outer deps come exclusively
  from `over`", see `planner.rs` around line 288). Assert via
  `BatchPlanner::plan`'s resulting `dependencies`/`stages` (check
  `after_tests.rs` or `sub_batch_tests.rs`-equivalent planner tests for the
  right assertion shape — read `crates/shamir-query-types/src/batch/tests/`
  for the existing convention before writing new assertions).
- **Black-box non-unfolding.** The `ForEach` body's own internal aliases must
  NOT appear as top-level nodes in the parent `BatchPlanner`'s stages/DAG —
  only the `ForEach` node itself does. (Read `planner.rs` around line 556 —
  `fe.batch` is treated like `SubBatchOp`'s nested body for whatever
  "does this alias have a nested batch" check exists; find the analogous
  existing `SubBatchOp` test and mirror it for `ForEach`.)
- **Static DoS gate.** A `ForEach` with a LITERAL `over` array of length N and
  a body of M queries must fold `N * M` into the same budget `max_queries`
  already enforces (read `planner.rs` around line 126-138 for the exact
  fold-in logic) — assert both an under-budget case (plan succeeds) and an
  over-budget case (plan fails with the expected `BatchError` variant, check
  which one `planner.rs` actually returns there).
- **Nesting-depth accounting.** Confirm a `ForEach` node's body counts toward
  `max_nesting_depth` the same way a `SubBatchOp`'s body does (find the
  existing `SubBatchOp` nesting-depth test as a template).

### 2. Non-transactional stop-at-first semantics (engine)

Add to `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs`:

- **`for_each_iteration_error_stops_at_first_in_non_tx_batch`** — mirrors the
  existing `for_each_iteration_error_aborts_whole_tx_batch` test but with
  `transactional: false` on the OUTER batch. Per ADR Decision 4: the
  non-transactional case is `stop-at-first`, NOT `collect-errors` — i.e. once
  iteration `i` fails, iteration `i+1..K` never runs, but iterations
  `0..i-1`'s already-applied writes are NOT rolled back (there's no
  transaction to roll back). Assert: (a) the batch response reports the
  error at the failing iteration, (b) records from iterations before the
  failure ARE visible in the table afterward (query the table directly to
  confirm), (c) records from iterations after the failure are NOT present.

### 3. Pessimistic authorization (engine or wherever `SubBatchOp`'s
   equivalent authorization test lives — check
   `crates/shamir-engine/src/query/auth/` tests, or `session.rs`'s own
   test module)

- **A `ForEach` whose body contains a write op must be classified `is_write =
  true` and require the corresponding write-access grant, even when the
  runtime `over` resolves to ZERO iterations.** This is exactly
  `for_each_is_write_reflects_body` from
  `crates/shamir-query-types/src/batch/tests/for_each_op_tests.rs` at the
  `BatchOp::is_write()` unit level already — for THIS phase, write the
  equivalent test one layer up: an actual authorization/session-permissions
  check (mirroring however `SubBatchOp`'s or `when`'s pessimistic
  authorization is tested elsewhere in the session/auth test suite) that
  confirms a caller WITHOUT write access is REJECTED even when `over` is a
  literal empty array (zero iterations at runtime, but still requires the
  write grant per Decision 5's "iteration count never affects the
  classification").

### 4. Interaction with `when` (Epic03) on the `ForEach` entry itself

- **A `ForEach` entry can itself carry a `when` guard** (it's a normal
  `QueryEntry`, so `when` applies to it like any other op) — add a test
  confirming: when the `ForEach`'s own `when` evaluates false, the ENTIRE
  loop is skipped (no iterations run, `skipped: true` in the result), not
  "run 0 iterations because `over` happened to be empty". These are two
  different codepaths (`when`-skip happens before `over` is even resolved)
  and should be tested as such.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full` must be
  green (not just `-- for_each`/`-- planner` filtered — run the FULL suite
  for both crates since planner/auth changes can have wide blast radius).
- `cargo fmt -p shamir-query-types -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace — every prior phase this session broke some OTHER crate by
  growing a struct/enum without updating all call sites elsewhere; this
  phase is test-only so it's lower risk, but still check).
- Report the literal command output for each of the above, not just a
  summary claim.

## Out of scope (do not touch)

- Any engine/production code changes — if a gap-closure test reveals an
  actual bug (not just a documentation/coverage gap), STOP, do not attempt a
  fix, and report the bug precisely (file:line, expected vs actual, minimal
  repro) instead of patching production code — this phase's job is coverage,
  not new fixes; a real bug found here becomes its own follow-up task.
- E2E tests (Phase E, #656), benchmarks (Phase F, #657), docs (Phase G, #658).
- The deferred while-loop design (#659).
