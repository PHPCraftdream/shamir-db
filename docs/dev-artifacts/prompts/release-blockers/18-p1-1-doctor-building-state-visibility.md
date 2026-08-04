# Brief — P1-1: `doctor::verify()` must surface stuck-`Building` regular/unique/sorted indexes

Task: #966 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P1-1. Depends on #959's persisted state machine (already landed). Read this brief in full — the scope is deliberately narrower than the review's headline framing; the review itself explicitly sanctions this narrower scope ("до релиза либо явно урезать обещания" — before release, OR explicitly scale back the promise).

## What already exists — do not re-derive from scratch

**index2 (fts/functional/vector) ALREADY self-heals automatically on table
open** (F-50 Step 3b, `crates/shamir-engine/src/table/table_manager.rs`
~line 395-455): a `Building` index2 backend found on open has its partial
postings dropped and its backfill re-run automatically, flipping to `Ready`.
This is UNRELATED to this task and must not be touched.

**The regular/unique/sorted ("base_index") family has NO such self-heal**,
confirmed by the codebase's own existing doc comment in
`crates/shamir-engine/src/table/table_manager_sorted_index.rs` ~line 141-159
(read it in full — it is the authoritative statement of this exact gap,
written when F-72/#899 deliberately deferred it): a crash mid-backfill
leaves the definition `Building` and planner-invisible forever, healable
ONLY via a manual, full-rebuild `doctor::repair()` call. **A full
open-time self-heal (mirroring index2's F-50 Step 3b) is EXPLICITLY OUT OF
SCOPE for this task** — it is a substantial, correctness-sensitive engine
change (crash-safety of a rebuild-on-open loop across 3 index families)
that deserves its own dedicated task with a full TDD cycle, not a single
60-minute delegated pass. This task implements ONLY the review's
explicitly-sanctioned fallback: **make the stuck state impossible to miss.**

## The actual gap in `doctor::verify()` today

`crates/shamir-engine/src/table/doctor.rs`'s `IndexHealth` (used for
regular/unique/sorted, ~line 68-79) ONLY compares `expected_entries ==
actual_entries` — it does **NOT** look at the index's `state` at all. A
`Building`-stuck index whose partial entry count HAPPENS to match the
expected count (or one with zero rows in a fresh table, trivially matching)
would report `is_healthy() == true` even though it's permanently
planner-invisible. Compare this to `Index2Health` (~line 89-102, already
correct): it's unhealthy specifically because `state != Ready`, independent
of any entry-count math. Confirm both `IndexDefinition` and
`SortedIndexDefinition` ALREADY carry a `state: IndexState` field
(`crates/shamir-index/src/base_index/index_definition.rs` ~line 33,
`sorted_index_definition.rs` ~line 90) — `doctor::verify()`'s existing
loops over `regular_defs`/`unique_defs`/`sorted_defs` already have `def`
in scope with `.state` on it; this is a small, mechanical, additive change.

## Required work

1. Extend `IndexHealth` (`doctor.rs` ~line 68-79) with a `state:
   crate::index2::state::IndexState` field (same type `Index2Health` uses)
   and update `IndexHealth::is_healthy()` to require `state ==
   IndexState::Ready` IN ADDITION TO the existing entry-count check —
   mirror `Index2Health`'s shape/behavior exactly (including an optional
   `message: Option<String>` diagnostic string for the unhealthy case,
   matching `Index2Health`'s existing message text style at ~line 224-232).
   Populate `state` from `def.state` in each of the 3 existing per-family
   loops (`regular_indexes`/`unique_indexes`/`sorted_indexes` construction,
   ~line 181-213) — `def` is already the right `IndexDefinition`/
   `SortedIndexDefinition` with `.state` on it, no new data plumbing needed.
2. Do NOT change `repair()`'s behavior — it already unconditionally rebuilds
   every index regardless of state, which already "fixes" a stuck-Building
   index when explicitly invoked (that's the existing "manual `doctor::
   repair()`" escape hatch named in the review; this task only makes the
   PROBLEM visible via `verify()`, not change how it's fixed).
3. Add deterministic tests proving the new visibility. Reuse the EXISTING
   backfill-pause test hooks — do NOT invent a new interruption mechanism:
   - Regular/unique: `IndexManager::set_create_index_backfill_hook`
     (`crates/shamir-index/src/base_index/index_manager.rs` ~line 320-329,
     `BackfillPauseHook` at `base_index/backfill_pause_hook.rs`).
   - Sorted: the equivalent `create_sorted_index_backfill_hook` field
     (`table_manager_sorted_index.rs` ~line 186-189) — check its exact
     setter method name before using it.
   For each of the 3 families: install the pause hook, start a
   `create_index`/`create_unique_index`/`create_sorted_index` call on a
   background task, let it pause mid-backfill (still `Building` on disk/in
   registry), then call `verify()` and assert: (a) the corresponding
   `IndexHealth` entry has `state == Building`, (b) `is_healthy() ==
   false`, (c) the overall `VerifyReport::is_healthy()` is `false` too.
   Also keep/add one happy-path test per family proving a fully-`Ready`
   index still reports healthy (regression guard — do not let the new
   `state` check false-positive on normal indexes).
4. Check existing tests in `crates/shamir-engine/src/table/tests/` for a
   doctor-focused test file to extend (search for `VerifyReport`/`doctor`)
   rather than creating a new one, per repo convention — unless none exists,
   in which case follow the standard `tests/` subdirectory layout
   (`crates/shamir-engine/src/table/tests/mod.rs` registers new files).

## Follow-up NOT done here — note it, do not attempt it

Server-side readiness/metrics signaling (`shamir-server`'s `/readyz`/
`/metrics`, `crates/shamir-server/src/observability.rs`) is explicitly NOT
part of this task — `/readyz`'s own doc comment states it must stay cheap
and never depend on other subsystems (it only checks "listeners bound"),
which is incompatible with wiring in a `doctor::verify()`-style scan
without a separate, lighter-weight design (e.g., a cheap in-memory registry
walk rather than a full data-store scan). Do not attempt this wiring — just
confirm in your report that it remains open, so the orchestrator can track
it as a separate follow-up.

## Gate (MANDATORY — this is production code, not test-only)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```

If `fmt --check` fails, run `cargo fmt -p shamir-engine` (scoped, never
`--all`). All three must pass before reporting done.

## Scope discipline

- Do NOT implement open-time auto-heal for regular/unique/sorted — see
  "What already exists" above. That is a separate, larger, deliberately
  deferred task.
- Do NOT touch index2/F-50 Step 3b code at all.
- Do NOT touch `repair()`'s rebuild logic — only `verify()`'s reporting.
- Do NOT attempt the server-side readiness/metrics wiring — report it as
  an open follow-up instead.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Show the exact new `IndexHealth` shape and `is_healthy()` logic. Confirm
each of the 3 families (regular/unique/sorted) has both a stuck-Building
test (using the existing pause hooks, not a new mechanism) and a
healthy-Ready regression test. Confirm the readiness/metrics follow-up is
explicitly left open, not attempted. Give exact `cargo fmt --check` /
`cargo clippy` / `./scripts/test.sh -p shamir-engine` output.
