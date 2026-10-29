# 244 — engine-query-batch-seam (engine crate split #1548, stage S0a: cut query-leaf → `query::batch` links, engine only)

Context (@oh ruling 2026-10-29). Plan #1548 later extracts the query leaf (`shamir-engine/src/query/{filter,read,write,auth,admin}`) into its own crate (S1, not this stage). A crate below the engine cannot name `query::batch` (batch depends on repo/table/tx/db_instance/validator). Re-verified at origin/master f9dc5788, the leaf's PRODUCTION code reaches into batch at exactly two places, and two leaf TEST files drive full batch executions:
- `query/auth/session.rs:4` `use crate::query::batch::{BatchOp, QueryEntry};` — both are re-exports of `shamir_query_types::batch::*` (see `query/batch/mod.rs` `pub use shamir_query_types::batch::{…}`).
- `query/filter/evaluation/runtime/core/eval_context.rs`: `FilterContext` fields `read_visibility_scope: Option<&'a crate::query::batch::ExecScope>` and `row_byte_meter: Option<&'a RowByteMeter<'a>>` (+ builders `with_read_visibility_scope`, `with_row_byte_meter`, and `remaining_read_visibility_budget`). Consumers: `query/batch/runner/query_runner/arms_read.rs:544-547` (sets both), `table/read/dispatch/{collect,paged}.rs` (`ctx.row_byte_meter` → `meter.charge(..)`, `meter.charge_record(..)`, `meter.credit_survivors(..)`; the static `RowByteMeter::value_size` call stays as is), `table/read/planning/read_temporal.rs` (`ctx.remaining_read_visibility_budget()`).
- Tests: `query/read/tests/aggregation/legacy_and_caps/aggregate_expression_execution_tests.rs` (310 lines) and `query/read/tests/query_results/select_refs_params_tests.rs` (215 lines) — 10 tests, both build a `DbInstance` and call `execute_batch_unchecked`; they use only absolute `crate::` paths (no `super::`).
Everything else in the leaf only references leaf modules or shamir_* crates (`crate::index*`, `crate::function` are re-exports of external crates — S1 rewrites those). Nothing outside shamir-engine uses `ExecScope`/`RowByteMeter`/these builders.

Read first: the files above, `query/batch/exec/exec_scope.rs` (RowByteMeter + ExecScope, 266 lines), `query/filter/mod.rs` (re-exports), `query/filter/evaluation/runtime/core/mod.rs`, `query/batch/tests/executor/ops/mod.rs`, `CLAUDE.md` (layout, imports, ideology).

## Changes
1. `query/auth/session.rs`: import `BatchOp`, `QueryEntry` from `shamir_query_types::batch` (same types; verify the re-export first).
2. New `query/filter/evaluation/runtime/core/read_budget.rs` (folder 4 → 5 entries) — two closely-coupled `pub(crate)` traits, leaf-owned:
   ```rust
   /// Shared read-visibility budget of an enclosing execution.
   pub(crate) trait ReadVisibilityScope: Send + Sync {
       fn read_visibility_budget(&self) -> Duration;
   }
   /// Policy-enabled per-read byte meter (result-size accounting).
   pub(crate) trait ReadByteMeter: Send + Sync {
       fn charge(&self, bytes: u64) -> Result<(), DbError>;
       fn charge_record(&self, record: &QueryRecord) -> Result<(), DbError>;
       fn credit_survivors(&self, records: &[QueryRecord]);
   }
   ```
   Signatures must equal the current inherent `RowByteMeter`/`ExecScope` methods (check them; `DbError` = `shamir_storage::error::DbError`, `QueryRecord` from `crate::query::read`). Register `pub mod read_budget;` in `core/mod.rs` and add `read_budget` to the `pub use evaluation::runtime::core::{…}` line in `query/filter/mod.rs`.
3. `eval_context.rs`: drop the batch import; fields become `Option<&'a dyn ReadVisibilityScope>` / `Option<&'a dyn ReadByteMeter>`; `with_read_visibility_scope(scope: &'a dyn ReadVisibilityScope)`, `with_row_byte_meter(meter: Option<&'a dyn ReadByteMeter>)`. Bodies, `clone_with_read_overlay`, defaults and docs otherwise unchanged.
4. `query/batch/exec/exec_scope.rs`: `impl ReadVisibilityScope for ExecScope` and `impl ReadByteMeter for RowByteMeter<'_>`, each method delegating to the inherent method of the same name via `Self::name(self, …)` (inherent items win resolution; rustc's `unconditional_recursion` lint under `-D warnings` is the safety net). Inherent methods, visibilities and bodies untouched.
5. `arms_read.rs`: `.with_row_byte_meter(meter.as_ref().map(|m| m as &dyn ReadByteMeter))`; `.with_read_visibility_scope(self.scope)` coerces unchanged. Import at file top.
6. `table/read/dispatch/*`, `read_temporal.rs`: expected unchanged (methods of a trait object's own trait resolve without an import). If rustc asks for an import, add it at the file top — nothing else.
7. Move (plain filesystem move, content byte-identical) the two test files into `query/batch/tests/executor/ops/` (4 → 6 entries); remove their `pub mod` lines from `query/read/tests/aggregation/legacy_and_caps/mod.rs` and `query/read/tests/query_results/mod.rs`; add them (alphabetical) to `query/batch/tests/executor/ops/mod.rs`.
8. Do NOT move `ExecScope`/`RowByteMeter`/`ResultSize`/`ExecutionDeadline` (rejected: drags `batch::control::receipts` and widens `pub(in crate::query::batch)` items). Do NOT touch `RowByteMeter::value_size` callers (table → batch links are S3's concern).

Perf note (accepted): one indirect call per charged row, only when result-size policy enabled a meter (`RowByteMeter::new` returns `None` otherwise); one per visibility wait for the scope. No allocation added.

## Tests (CI runs them — never locally)
No new tests; behaviour is unchanged. Moved test identities (report the 10 names): `query::read::tests::…::<file>::<name>` → `query::batch::tests::executor::ops::<file>::<name>`. Acceptance by reading:
- `git ls-files crates/shamir-engine/src/query/{filter,read,write,auth,admin} | xargs grep -nE 'crate::query::batch|crate::(repo|table|tx|db_instance|validator)\b'` → no code hits (prose in comments allowed).
- `grep -rE '^\s*#\[(tokio::)?test' crates/shamir-engine/src/query/read/tests crates/shamir-engine/src/query/batch/tests | wc -l` → `560` (unchanged); `…/executor/ops` → `54` (was 44).
- `grep -rn 'ExecScope\|RowByteMeter' crates/shamir-engine/src/query/filter` → none.
- No intra-doc link (`` [`ExecScope`] `` etc.) left dangling in edited files — the CI `rustdoc` gate (`--document-private-items`, broken links = error) cannot be run locally; check by reading.

## Rules
- Touch only the files named above (all under `crates/shamir-engine/src/query/`). No campaign/delta, no `.rust-guard/**`, no Cargo.toml change (decided in E_PLAN: test-discovery currently ignores deltas, so a campaign would fail on the 10 moved test names).
- No visibility widening beyond `pub mod read_budget;` holding `pub(crate)` traits; no new `#[allow]`; imports at file top; `mod.rs` = declarations/re-exports only; ≤ 7 entries/folder, ≤ 1000 lines/file; one file = one primary export (the two traits are one coupled seam). Comments laconic.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` / `mv`, or any git command that mutates the working tree or index. Only edit files (moves are plain filesystem moves); the orchestrator commits.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-engine -- --check`
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`
- `cargo clippy -p shamir-engine --all-targets --features test-util -- -D warnings` (`query/auth/session.rs` is `cfg(any(test, feature = "test-util"))`)
- `python -I scripts/ci/check_layout.py`
Report every NEW/MOVED file path, the counts above and the real exit codes.
