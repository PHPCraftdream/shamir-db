# Brief — e2e gap cluster B: HAVING + EXPLAIN dry-run + PlanType assertions

Task: #975 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, Cluster B (near the end of the doc) and the detailed row-level findings in §1 (OQL, look for `GroupBy`/`having` and `explain`/`PlanType` rows). Read both sections in full before starting.

## The gap

HAVING and EXPLAIN are both completely unexercised live (zero hits across both e2e suites) despite being first-class `ReadQuery`/`GroupBy` fields with 9 `PlanType` variants. EXPLAIN is the only way to assert the planner picked the right index strategy — currently only inferred indirectly via `stats.index_used`.

## Required work

Source types: `crates/shamir-query-types/src/read/group_by.rs` (~line 13, the `having` field), `crates/shamir-query-types/src/read/read_query.rs` (~line 45, the `explain` flag), `crates/shamir-query-types/src/read/query_result.rs` (~line 41, the `QueryResult.explain` shape — read this to know exactly what fields to assert on: `plan_type`, `index_used`, `estimated_rows`, and any others). Read the actual current field names/types before writing tests — do not guess from this brief's paraphrase.

1. **HAVING**: add a test reusing `e2e-data.test.ts`'s existing group-by fixture data (check that file first — it already has GROUP BY test data set up, reuse rather than duplicate) that adds a `having` predicate on an aggregate (e.g. `having(sum('qty'), gt, N)` or whatever the builder's actual method signature is — check `crates/shamir-client-ts/src/core/builders/query.ts` for the exact `.having()` API). Assert the result set is correctly filtered post-aggregation (rows that pass the raw filter but fail HAVING must be excluded).
2. **EXPLAIN**: add `explain: true` to several representative queries covering DIFFERENT planner paths — at minimum: a plain full-scan query, a regular/hash-indexed equality lookup, a sorted-index range query, an FTS query, and a vector-similarity query (reuse existing fixtures/tables from other test files where possible rather than building new ones from scratch). For each, assert `QueryResult.explain`'s `plan_type` matches the EXPECTED strategy (e.g. the indexed-equality query should NOT report a full scan), and that `index_used`/`estimated_rows` (or whatever the real field names are) are present and sane.
3. Decide the best home for these tests — likely a new file in `crates/shamir-client-ts/src/__tests__/` (e.g. `e2e-explain-having.test.ts`) following the existing `e2e-*.test.ts` conventions (spawn pattern via `e2e-harness.ts`, `describe.skipIf(!SERVER_AVAILABLE)`), rather than cramming into an already-large existing file — check file sizes of `e2e-data.test.ts` etc. first to decide.

Use ONLY query builders (`Query.groupBy(...).having(...)`, `.explain(true)` or whatever the actual fluent API is per `query.ts`) — no hand-assembled wire objects (repo-wide rule, CLAUDE.md).

## Verification

- Run the TS e2e suite (`vitest run` in `crates/shamir-client-ts`, or the workspace equivalent) and report the exact pass/fail counts before and after.
- If you also touch `tests/e2e/tests/*.js`, run `cd tests/e2e && node e2e.test.js` too and report its counts (baseline after task #974: 18 files / 133 passed / 0 failed).

## Scope discipline

- Do NOT touch replication (cluster A, #974, already done). Do NOT touch FTS/functional index options (cluster C, a separate task). Stay within HAVING/EXPLAIN.
- Do NOT modify production Rust code. If `SelectItem::Expression` or any other executor gap blocks a test you want to write, work around it with a supported query shape and note the limitation in your report — do not attempt to "fix" the executor.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` — the orchestrator verifies your diff and the test run, then commits. Only edit/create test files and run read-only/test commands.

## What to report back

List every test added and what it proves (especially which `plan_type` each EXPLAIN test expects and why), confirm ONLY query builders were used, and give exact test-run output with real pass/fail counts.
