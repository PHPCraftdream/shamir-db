# CI-02c — layout sweep stage: shamir-query-types

Part of CI-02 (see `02-layout-sweep-query.md`). Scope: `crates/shamir-query-types/` ONLY. Pure structural refactor — behaviour, public API paths, error behaviour, test semantics and test names stay identical. Every public path is preserved via `pub use` facades, so NO caller in any crate changes.

## Baseline violations to resolve (before → after target)
- `dir src 15` → 7
- `dir src/admin/types 18` → 5
- `dir src/admin/types/tests 8` → 6
- `dir src/batch 25` → 7
- `dir src/batch/tests 13` → 5
- `dir src/read 14` → 5
- `dir src/read/tests 10` → 4
- `file src/batch/tests/planner_tests.rs 1196` → ≤1000 (split in two)

## Target tree (each dir annotated with its direct-entry count; ALL must be ≤7)

```
src/ (7)
├── lib.rs                 — see "lib.rs" below
├── admin/                 — mod.rs, access.rs, tests/ unchanged at this level
│   └── types/ (5)
│       ├── mod.rs         — facade
│       ├── db/ (7)        ← db_ops.rs, table_ops.rs, schema_ops.rs, fk_action.rs, retention.rs, buffer_config.rs
│       ├── ddl/ (7)       ← index_ops.rs, interner_ops.rs, list_ops.rs, migration_ops.rs, function_ops.rs, validator_ops.rs
│       ├── repl/ (3)      ← repl_ops.rs, seed_subscription/ (dir, unchanged inside)
│       ├── changes_since_response/ (6) — UNCHANGED
│       └── tests/ (6)
│           ├── mod.rs, create_index_op_tests.rs, fk_action_tests.rs, repl_ops_tests.rs, temporal_admin_tests.rs
│           └── repo/ (4)  ← constraints_dto_tests.rs, repo_ops_tests.rs, retention_ddl_tests.rs
├── durability/ (6)        — UNCHANGED
├── wire/ (5)              — UNCHANGED
├── ops/ (6)               — NEW group: private `mod ops;` in lib.rs + re-exports
│   ├── mod.rs
│   ├── table_ref.rs       ← moved from src/table_ref.rs
│   ├── call/              ← whole src/call/ dir moved as-is
│   ├── subscribe/         ← whole src/subscribe/ dir moved as-is
│   ├── write/             ← whole src/write/ dir moved as-is
│   └── batch/ (7)         ← whole src/batch/ dir moved + reorganized below
│       ├── mod.rs         — facade
│       ├── core/ (5)      ← alias.rs, reference.rs, interner_delta.rs, batch_error.rs
│       ├── ops/ (5)       ← batch_op.rs, for_each_op.rs, sub_batch_op.rs, query_entry.rs
│       ├── plan/ (7)      ← batch_plan.rs, planner.rs, edge_kind.rs, exec_policy.rs, limit_kind.rs, tree_visitor.rs
│       ├── req/ (7)       ← batch_request.rs, batch_response.rs, transaction_info.rs, result_encoding.rs, batch_limits.rs, options/ (dir unchanged inside)
│       ├── vis/ (4)       ← visibility_pending_reason.rs, visibility_state.rs, visibility_wait.rs
│       └── tests/ (5)
│           ├── mod.rs, batch_types_tests/ (dir unchanged inside)
│           ├── ops/ (6)   ← batch_error_tests.rs, batch_limits_tests.rs, distinct_repos_tests.rs, for_each_op_tests.rs, typed_options_tests.rs
│           ├── plan/ (7)  ← for_each_planner_tests.rs, limit_model_tests.rs, static_tree_gate_tests.rs, tree_visitor_tests.rs, planner_tests.rs (split part 1), planner_gate_tests.rs (split part 2)
│           └── vis/ (2)   ← visibility_tests.rs
├── read/ (6)              — stays top-level (lib.rs keeps `pub mod read;`), reorganized internally + absorbs filter
│   ├── mod.rs             — adds `pub(crate) mod filter;` facade + group decls
│   ├── core/ (6)          ← read_query.rs, query_record.rs, query_result.rs, select.rs, select_expr.rs
│   ├── clause/ (7)        ← agg.rs, ddl.rs, group_by.rs, limit.rs, order_by.rs, temporal.rs
│   ├── filter/ (7)        ← whole src/filter/ dir moved as-is (mod.rs, cond.rs, filter_enum.rs, filter_expr.rs, filter_value.rs, fn_call.rs, tests/)
│   ├── receipts/ (2)      — UNCHANGED
│   └── tests/ (4)
│       ├── mod.rs, receipts/ (dir unchanged)
│       ├── clause/ (5)    ← ddl_tests.rs, pagination_after_tests.rs, read_query_temporal_tests.rs, temporal_tests.rs
│       └── core/ (5)      ← aggregate_null_input_tests.rs, query_record_tests.rs, query_result_tests.rs, select_expr_tests.rs
└── security/ (5)          — NEW group: private `mod security;` in lib.rs + re-exports
    ├── mod.rs
    ├── auth/              ← whole src/auth/ dir moved as-is
    ├── validator/         ← whole src/validator/ dir moved as-is
    ├── hmac.rs            ← moved from src/hmac.rs
    └── tests/ (2)         ← mod.rs + hmac_tests.rs moved from src/tests/
```

## lib.rs changes (only these)
- Keep `pub mod admin; pub mod durability; pub mod read; pub mod wire;`.
- Remove `pub mod auth; pub mod batch; pub mod call; pub mod filter; #[cfg(feature = "crypto")] pub mod hmac; pub mod subscribe; pub mod table_ref; pub mod validator; pub mod write;` and `#[cfg(test)] mod tests;`.
- Add: `mod ops; mod security;` then
  `pub use ops::{batch, call, subscribe, table_ref, write};`
  `pub use read::filter;`
  `pub use security::{auth, validator};`
  `#[cfg(feature = "crypto")] pub use security::hmac;`
- Keep ALL existing item-level `pub use` lines in lib.rs (`pub use batch::ResultEncoding;` etc.) unchanged — they resolve through the facade re-exports.
- Leave the crate-level doc comment untouched.
- read/mod.rs declares the absorbed filter as `pub(crate) mod filter;` (read itself is public, so pub(crate) here keeps the external surface unchanged — lib.rs's `pub use read::filter;` restores the public path).

## planner_tests.rs split (1196 lines)
Split at a topic seam so both files ≤1000 lines, keeping every #[test] fn name and every helper fn exactly once. Suggested seam: part 1 keeps the sub-batch/dep, edge-provenance and where/when query-ref extraction tests; part 2 (`planner_gate_tests.rs`) takes the when-gate gap tests (when_gap1_*, when_gap2_*, insert-op marker helpers) and everything after. Both files share the helper fns at the top of the original (read_entry, sub_batch_entry, empty_batch_request, batch_request_with_queries, read_entry_with_query_ref, gated_entry_when, assert_when_rejected, insert_op_with_marker_field, insert_entry) — place each helper in the file whose tests use it; if both need one, keep it in planner_tests.rs with `pub(super)`... wait: they are sibling files in the SAME module `plan` — declare helpers `pub(crate)` only if needed and import via `use super::planner_tests::<helper>` from the other file; prefer zero sharing by grouping tests so each helper has exactly one user file. Imports at top of each new file only for what it uses.

## Facade rules (keep every public path stable so NO caller changes)
F1. A moved module keeps its visibility keyword; its NEW parent (the group mod.rs) declares it with that keyword (`pub mod X;` / `pub(crate) mod X;`).
F2. The group is declared in ITS parent as a private `mod <group>;`.
F3. The module's ORIGINAL parent manifest replaces the old `pub mod X;` with `pub use <group>::X;` using the original visibility keyword. Examples: batch/mod.rs declared `pub mod planner;` → batch/mod.rs gets `pub use plan::planner;`, plan/mod.rs gets `pub mod planner;`. lib.rs declared `pub mod table_ref;` → lib.rs gets `pub use ops::table_ref;`, ops/mod.rs gets `pub mod table_ref;`.
F4. ALL existing item-level re-exports in the original manifests stay EXACTLY as they are — they keep resolving through F3. Do not rewrite, reorder or deduplicate them.
F5. A `use super::X;` inside a file keeps working when its parent manifest re-exports `X` (F3). A file that moved DEEPER (into a new group — e.g. batch_op.rs → batch/ops/, planner.rs → batch/plan/, db_ops.rs → admin/types/db/) must have its `use super::X;` lines rewritten to the absolute `use crate::<top>::X;` form that resolves through the facade chain (e.g. `use super::for_each_op::ForEachOp;` in batch_op.rs becomes `use crate::batch::for_each_op::ForEachOp;`).
F6. `#[path]`/`serde(with = "crate::…")` string paths: grep every moved file for `#[path =`, `include!(`, `serde(with = "` and fix any that reference a moved location (u64_wire in changes_since_response did NOT move — check but expect no change).
F7. Never create a second public path for something that didn't have one; never remove an existing one. mod.rs manifests contain mod declarations + re-exports only.

## Import audit (mandatory)
After all moves: for EVERY new/changed file check each `use` line resolves in the new layout (trace crate::… through lib.rs re-exports → group mod.rs → file; super::… only per F5). Fix ONLY broken paths. Leave everything else byte-identical.

## Tests
- Test function names never change. Each new tests subfolder gets a `mod.rs` manifest matching the declaration style already used in that tests/mod.rs (`mod x;` vs `pub mod x;` — copy it), wired from the parent tests/mod.rs.
- src/tests/ moves to security/tests/: security/mod.rs gains `#[cfg(test)] mod tests;` (preserve any `#[cfg(feature = "crypto")]` gating found in the original src/tests/mod.rs declarations EXACTLY). lib.rs loses `#[cfg(test)] mod tests;`.

## Stale doc comments
Update ONLY doc-comment lines that enumerate or link modules and become wrong because of a move (batch/mod.rs's module list, read/mod.rs if it lists children). Minimal edits. Do NOT touch lib.rs's crate doc.

## Hard rules
- ⛔ NEVER run `git reset` / `git checkout` / `git clean` / `git stash` / `git restore` / `git rm` / `git mv`, or any git command that mutates the working tree or index. Read-only git (`status`, `diff`, `show`, `log`, `ls-files`) is fine. The orchestrator commits.
- ⛔ NO compile/test commands: no `cargo test|build|check|clippy|nextest|doc`, `rustc`, `npm`, `tsc`. CI is the gate. Allowed ONLY: `cargo fmt -p shamir-query-types` (write; as the very LAST step, once, after everything else) + `-- --check`, and `python -I scripts/ci/check_layout.py` (check mode ONLY — NEVER `--update-baseline`).
- Never silence a lint with `#[allow]`.
- Imports at the top of each file (new files you create). Keep every item's name, signature, visibility, doc comments and attributes byte-exact unless a rule above requires a change.
- Moving a file: `mkdir -p` + `cp <old> <new>` → apply only the required edits to the NEW file → `rm <old>` (plain rm). Never `git mv`/`git rm`.
- Do not touch anything outside `crates/shamir-query-types/` (no other crate, no scripts/, no docs/, no baseline file).

## Verification before reporting
1. `python -I scripts/ci/check_layout.py` — zero lines mentioning shamir-query-types.
2. `cargo fmt -p shamir-query-types -- --check` clean.
3. Re-read every manifest you edited in full (lib.rs, ops/mod.rs, ops/batch/mod.rs, security/mod.rs, read/mod.rs, admin/types/mod.rs, every group mod.rs, every tests/mod.rs): no `pub mod X;` left pointing at an old location; every `mod` decl matches an existing file; every moved module re-exported per F3.

## Report format
- before/after direct-entry counts: src/, admin/types/, admin/types/tests/, ops/batch/ (was src/batch/), ops/batch/tests/ (was src/batch/tests/), read/, read/tests/ (plus any new dir)
- before/after line counts: planner_tests.rs, planner_gate_tests.rs
- files moved (grouped, old → new), visibility widenings, super::/#[path]/serde(with) rewrites
- checker output lines for this crate (must be none); anything you could NOT do and why
