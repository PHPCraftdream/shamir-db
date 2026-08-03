# Brief — TS e2e coverage inventory + gap matrix for OQL and DDL

Task: #964 in the session TaskList. Explicit user request (2026-08-03, flagged as important): TypeScript e2e tests must cover the ENTIRE OQL and ENTIRE DDL surface — not selectively, but per a real inventory of capabilities.

**This is PHASE 1 ONLY: produce the inventory and gap matrix. Do NOT attempt to write new tests in this session** — the matrix this session produces will drive follow-up tasks, each scoped to a specific gap area, run in separate `crush` sessions. Writing tests now, before the matrix exists and is reviewed, risks duplicating effort or missing systemic gaps. Stay disciplined to this phase-1 scope even if you're confident you could "just fix it" — the orchestrator needs the matrix as a durable artifact regardless.

## What to inventory

### OQL (source of truth: `crates/shamir-query-types/src`, NOT memory/training data)

Walk the actual types/enums in this crate and enumerate every distinct capability:
- Filters: every `FilterOp`/predicate variant (eq/ne/gt/gte/lt/lte/in/notIn/between/and/or/not/fts/computed/vectorSimilarity/queryRef and any others you find — do not assume the list from this brief is exhaustive, read the actual enum).
- Aggregates: countAll/sum/avg/min/max, `group by`, `having`.
- Batch DAG: multi-query batches, `queryRef` cross-references, conditional execution, loops (if these exist as real wire ops — verify, don't assume).
- Temporal/AsOf reads.
- Projections: field selection, computed expressions (check `SelectItem::Expression` — the review noted the executor currently REJECTS this; if so, it's DELIBERATELY out of scope for e2e coverage since the feature doesn't work yet — verify and note this explicitly, don't file it as an e2e gap).
- Sorting: orderByAsc/orderByDesc, composite sort.
- Pagination: limit/offset, countTotal.
- Cursors: `Latest`, `with_version` if it exists, key types.
- EXPLAIN (dry-run).
- FTS filters, vector similarity filters (with options like efSearch/oversample), functional-index filters.

### DDL (source of truth: `crates/shamir-query-types/src` DDL ops + `crates/shamir-client-ts/src/core/builders/ddl.ts` + `admin.ts`)

- create/drop/rename for: db, repo, table.
- create/drop/rename for EACH of the four index families: regular (hash), unique, sorted, index2 (fts/functional/vector) — enumerate every distinct `CreateIndexOp` shape/option combination the wire type supports (tokenizer/language/ranking for fts, expression for functional, dim/metric/quantization for vector).
- Buffer config: setBufferConfig/getBufferConfig/alterBufferConfig.
- Migrations: migrationStatus and any other migration ops.
- Validators: bind/list/drop validator bindings, if these exist as client-facing ops.
- Replication ops: publication/replScope/replicationProfile/replStream/subscription, setReplicator (already e2e-tested per task #931, verify it's still covered).
- Admin/ACL ops: chmod/chgrp/createUser/createScramUser/createGroup/addGroupMember/removeGroupMember/grantRole/revokeRole/setSuperuser/dropUser/accessTree/resolvePrincipal/listUsers.

## What to inventory on the TEST side

For each capability above, search:
- `tests/e2e/tests/*.js` (the 18-file suite, now fully query-builder-based per tasks #939-956).
- `crates/shamir-client-ts/src/__tests__/*.test.ts` and `crates/shamir-client-ts/src/core/builders/__tests__/*.test.ts` (unit-level builder shape tests — note these separately from TRUE e2e tests against a real server; a builder unit test does NOT count as e2e coverage for this matrix, be strict about that distinction).

For each capability, determine: **covered by a real e2e test against a live server** (yes/no), and if yes, which file/test name.

## Deliverable

Write the matrix to `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md` (a NEW file — do not overwrite the existing review doc). Format: one table for OQL, one for DDL, columns: Capability | Source location (file:line or type name) | E2E test? (yes/no) | Test file:name (if yes) | Notes.

At the end of the document, add a prioritized list of the gaps found, grouped into natural follow-up-task-sized clusters (e.g. "cluster A: FTS/functional/vector index2 DDL lifecycle e2e", "cluster B: aggregate/group/having e2e", etc.) — this is what the orchestrator will turn into individual TaskCreate entries for the next round of `crush` sessions. Don't be shy about the list being long; a long, honest gap list is the correct outcome of an inventory that hasn't been done before (per the review, "покрытие не измерено ни разу").

## Scope discipline

- Do NOT write, modify, or delete any test files. Do NOT modify any production code. This is a READ-ONLY investigation + one new markdown deliverable.
- Do NOT modify the existing review doc or any other committed `docs/dev-artifacts/` file.
- No `cargo`/`npm` test runs needed for this phase (nothing to verify — you're not changing test behavior). A quick `node tests/e2e/e2e.test.js` sanity run to confirm the baseline (18 files / 130 passed) is fine if you want to ground your inventory, but not required.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` — the orchestrator commits the new matrix file after reviewing it. Only read files and write the ONE new markdown deliverable.

## What to report back

Confirm the matrix file path, a rough capability count (how many OQL capabilities, how many DDL capabilities enumerated), how many are covered vs. not, and the cluster list you proposed for follow-up tasks.
