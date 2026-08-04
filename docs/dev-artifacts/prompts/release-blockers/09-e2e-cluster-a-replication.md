# Brief — e2e gap cluster A: replication lifecycle/introspection ops

Task: #974 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, Cluster A (§ near the end, "Cluster A — Replication lifecycle & introspection ops e2e"), and the detailed row-level findings at §2.12 "Replication". Read both sections in full before starting.

## The gap

7 wire ops — `drop_publication`, `drop_subscription`, `drop_replication_profile`, `alter_subscription` (pause/resume/set_profile), `list_publications`, `list_subscriptions`, `replication_status` — have builder-shape unit coverage AND msgpack byte-parity coverage (`repl_parity.test.ts`), but **zero live-server execution** in either e2e suite. `create_publication`/`create_replication_profile`/`create_subscription` ARE already exercised live in `tests/e2e/tests/17-replication-convergence.test.js` (a working 2-server leader/follower convergence harness) — this task extends coverage to the missing lifecycle/introspection half, it does not build convergence infrastructure from scratch.

## Required work

Extend `tests/e2e/tests/16-replication.test.js` (which already exercises `ReplHello`/`ReplPull` against a live server) OR add a new file if that's cleaner given the existing file's scope — your call, but check `tests/e2e/tests/17-replication-convergence.test.js`'s existing 2-server setup harness first since these lifecycle ops naturally build on a publication/subscription/profile that already exists.

Add live e2e coverage for a full lifecycle:
1. `create_publication` (already proven, reuse the pattern) → **`list_publications`** (assert the new publication appears) → **`drop_publication`** (assert it's gone from a subsequent `list_publications`).
2. `create_replication_profile` → **`drop_replication_profile`** (assert error/no-op behavior on subsequent use, per whatever the wire contract specifies).
3. `create_subscription` → **`list_subscriptions`** (assert it appears) → **`alter_subscription`** exercising pause, resume, AND set_profile (assert `replication_status` or `list_subscriptions` output reflects each state transition) → **`drop_subscription`** (assert it's gone).
4. **`replication_status`**: assert its output shape/fields are sane at least once mid-lifecycle (e.g. after creating a subscription, before dropping it).

Use ONLY query builders (`@shamir/client`'s `replication.ts` builders per the matrix's own reference to `repl_ops.rs`/`repl_parity.test.ts` for exact wire shapes) — no hand-assembled request objects (repo-wide rule, CLAUDE.md).

## Verification

- `cd tests/e2e && node e2e.test.js` — must stay green, report the exact file/passed count before and after (baseline before this task: confirm via `git log`/`docs/checkpoints` what the last known-good count was, likely 18 files / 130+ tests since #939-956).
- If you touch anything in `crates/shamir-client-ts`, also run its `vitest` suite (`npm test` in that crate, or the workspace equivalent) and report pass/fail counts.

## Scope discipline

- Do NOT touch OQL coverage (HAVING/EXPLAIN is cluster B, a separate task). Do NOT touch index2/FTS option breadth (cluster C). Stay within replication lifecycle ops.
- Do NOT modify production Rust code — this is a test-only task (if you discover an actual bug in the replication ops while writing tests, STOP and report it in your final summary instead of silently working around it or fixing it — a real bug found here needs its own reviewed task).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` — the orchestrator verifies your diff and the test run, then commits. Only edit/create test files and run read-only/test commands.

## What to report back

List every test added and what it proves, confirm ONLY query builders were used (no hand-assembled objects), and give the exact `node e2e.test.js` (and vitest, if touched) output with real pass/fail counts.
