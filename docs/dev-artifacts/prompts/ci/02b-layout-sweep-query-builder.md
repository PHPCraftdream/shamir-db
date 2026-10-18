# CI-02b — layout sweep stage: shamir-query-builder

Part of CI-02 (see `02-layout-sweep-query.md`). Scope: `crates/shamir-query-builder/` ONLY. Pure structural refactor — behaviour, public API paths, error behaviour, test semantics and test names stay identical. Every public path is preserved via `pub use` facades, so NO caller in any crate changes.

## Baseline violations to resolve (before → after target)
- `dir src 14` → 7
- `dir src/batch 11` → 7
- `dir src/batch/tests 13` → 5
- `dir src/ddl 33` → 7
- `dir src/ddl/tests 10` → 5
- `dir src/write 8` → 4
- `file src/batch/batch.rs 1405` → ≤1000 (split in two)
- `file src/query/tests/query_tests.rs 1115` → ≤1000 (split in two)

## Target tree (each dir annotated with its direct-entry count; ALL must be ≤7)

```
src/ (7)
├── lib.rs                 — see "lib.rs" below
├── macros/                — UNCHANGED, `#[macro_use] pub mod macros;` line stays in lib.rs untouched
├── batch/ (7)
│   ├── mod.rs             — facade: see below
│   ├── batch.rs           — split part 1 (struct Batch, Default, first impl Batch chunk) ≤1000 lines
│   ├── batch_refs.rs      — split part 2 (rest of impl Batch + the "$query ref walking helpers" free fns) ≤1000 lines
│   ├── op/ (4)            ← into_batch_op/ (dir, unchanged inside), try_into_batch_op.rs, build_error.rs
│   ├── runtime/ (5)       ← handle.rs, durability.rs, isolation.rs, subscribe.rs
│   ├── vis/ (3)           ← index_visibility.rs, AND visibility.rs moved here from src/visibility.rs
│   └── tests/ (5)
│       ├── mod.rs, aggregate_input_tests.rs, batch_tests.rs
│       ├── flow/ (7)      ← after_tests.rs, call_tests.rs, for_each_tests.rs, sub_batch_tests.rs, subscribe_tests.rs, when_tests.rs
│       └── op/ (5)        ← ddl_methods_tests/ (dir, unchanged inside), to_request_via_msgpack_tests.rs, try_into_batch_op_tests.rs, visibility_tests.rs
├── ddl/ (7)
│   ├── mod.rs             — facade
│   ├── db/ (7)            ← create_db.rs, drop_db.rs, rename_db.rs, create_table.rs, drop_table.rs, rename_table.rs
│   ├── schema/ (7)        ← schema.rs, describe_table.rs, list.rs, function.rs, buffer_config.rs, quantization.rs
│   ├── index/ (7)         ← create_index.rs, create_index_build_error.rs, drop_index.rs, rename_index.rs, index_spec.rs, tokenizer.rs
│   ├── policy/ (7)        ← access_control.rs, auth.rs, retention.rs, metric.rs, replication.rs, migration.rs
│   ├── repo/ (7)          ← alter_repo/ (dir, unchanged inside), drop_repo.rs, rename_repo.rs, res.rs, interner.rs, interner_resolve.rs
│   └── tests/ (5)
│       ├── mod.rs, helpers.rs
│       ├── index/ (3)     ← index_spec_tests.rs, interner_resolve_tests.rs
│       ├── policy/ (4)    ← access_ddl_tests.rs, migration_ddl_tests.rs, replication_ddl_tests.rs
│       └── schema/ (4)    ← function_ddl_tests.rs, list_ddl_tests.rs, schema_ddl_tests/ (dir, unchanged inside)
├── read/ (7)              — NEW group: private `mod read;` in lib.rs + re-exports
│   ├── mod.rs             — declares cursor/filter/query/select/val, no logic
│   ├── cursor.rs          ← moved from src/cursor.rs (its `#[cfg(test)] mod tests;` wiring moves with it)
│   ├── cursor/ (1)        ← tests.rs
│   ├── filter/ (4)        ← whole src/filter/ dir moved as-is (mod.rs, combinators.rs, leaf.rs, tests/)
│   ├── query/ (6)         ← whole src/query/ dir: mod.rs, conds.rs, into_select_item.rs, query.rs, query_build_error.rs, tests/
│   │   └── tests/ (4)     ← mod.rs, temporal_tests.rs, query_tests.rs (split part 1), query_conds_tests.rs (split part 2)
│   ├── select/ (3)        ← whole src/select/ dir moved as-is
│   └── val/ (4)           ← whole src/val/ dir moved as-is
├── wire/ (3)
│   ├── mod.rs             — adds `pub mod response;` facade line
│   ├── response/ (3)      ← whole src/response/ dir moved as-is (mod.rs, batch_response_ext.rs, tests/)
│   └── tests/             — unchanged
└── write/ (4)
    ├── mod.rs             — facade
    ├── builder_error.rs   — stays
    ├── ops/ (6)           ← delete.rs, doc.rs, insert.rs, update.rs, upsert.rs
    └── tests/             — unchanged
```

## lib.rs changes (only these)
- Remove `pub mod cursor; pub mod filter; pub mod query; pub mod response; pub mod select; pub mod val; pub mod visibility;`.
- Add: `mod read;` then `pub use read::{cursor, filter, query, select, val};`, `pub use batch::visibility;`, `pub use wire::response;`.
- Keep `pub mod batch; pub mod ddl; pub mod wire; pub mod write;` and the `#[macro_use] pub mod macros;` line EXACTLY as-is (do not move macros/, do not reorder #[macro_use] scoping).
- Keep all existing `pub use` lines in lib.rs unchanged (they resolve through the facades).
- Leave the crate-level doc comment untouched.

## batch/batch.rs split
- batch.rs has a `#[path = "into_batch_op/repo_admin.rs"] mod repo_admin;` near the top — after into_batch_op/ moves to op/, update the path attribute to `#[path = "op/into_batch_op/repo_admin.rs"]` (relative to batch/).
- Split the `impl Batch` block (starts ~L54) at a method-group boundary so batch.rs ≤1000 lines; the tail impl-Batch chunk plus the free helper fns after the `// ====…$query ref walking helpers` banner (~L1144 to EOF: collect_query_refs, the typed walkers, and everything else) go to batch_refs.rs as a second `impl Batch` block + free fns.
- If a private helper is used by BOTH parts, keep it in the file with more users and widen to `pub(super)` for the other; record the widening.
- batch_refs.rs header: short doc comment ("Second half of `Batch`'s impl + `$query`-ref walking helpers — see [`super::batch`]."), imports at top only for what it uses (grep per import; unused imports fail CI clippy `-D warnings`), plus `use super::batch::Batch;` as needed.

## Facade rules (keep every public path stable so NO caller changes)
F1. A moved module keeps its visibility keyword; its NEW parent (the group mod.rs) declares it with that keyword (`pub mod X;` / `pub(crate) mod X;`).
F2. The group is declared in ITS parent as a private `mod <group>;`.
F3. The module's ORIGINAL parent manifest replaces the old `pub mod X;` with `pub use <group>::X;` using the original visibility keyword. Example: src/cursor.rs was `pub mod cursor;` in lib.rs → lib.rs gets `pub use read::cursor;`; read/mod.rs gets `pub mod cursor;`. Example: build_error.rs was declared in batch/mod.rs → batch/mod.rs gets `pub use op::build_error;`; op/mod.rs gets `pub mod build_error;`.
F4. ALL existing item-level re-exports in the original manifests (`pub use batch::*;`, `pub use build_error::*;`, `pub use subscribe::{SourceBuilder, Subscribe};` etc.) stay EXACTLY as they are — they keep resolving through F3. Do not rewrite, reorder or deduplicate them.
F5. A `use super::X;` inside a file keeps working when its parent manifest re-exports `X` (F3) — sibling imports in batch.rs, batch_refs.rs and files that stay at the same depth need no edit. A file that moved DEEPER (into a new group) must have its `use super::X;` lines rewritten to the absolute `use crate::<top>::X;` form (e.g. write/insert.rs → write/ops/insert.rs: `use super::builder_error::...` becomes `use crate::write::builder_error::...`).
F6. `#[path]` attributes are relative to the containing file's directory — update for new depth (see batch.rs above).
F7. Never create a second public path for something that didn't have one; never remove an existing one. Files under `mod.rs` manifests keep "re-exports + mod declarations only" — no logic.

## Import audit (mandatory)
After all moves: for EVERY new/changed file check each `use` line resolves in the new layout (trace crate::… through lib.rs re-exports → group mod.rs → file; super::… only per F5). Fix ONLY broken paths. Grep moved files for string-encoded paths: `serde(with = "`, `#[path =`, `include!(`. Fix what the moves broke; leave everything else byte-identical.

## Tests
- Test function names never change. Each new tests subfolder (flow/, op/, and under ddl/tests/) gets a `mod.rs` manifest listing its files, matching the declaration style already used in the existing tests/mod.rs (`mod x;` vs `pub mod x;` — copy it).
- Wire each new subfolder into its parent tests/mod.rs (e.g. batch/tests/mod.rs gains `mod flow; mod op;` — matching existing style — and loses the entries that moved).
- If a moved test file used `use super::helpers` or similar (ddl/tests), rewrite to the correct new path (e.g. `use crate::ddl::tests::helpers` — check how helpers.rs is declared/used today and keep it working).

## Stale doc comments
Update ONLY doc-comment lines that enumerate or link modules and become wrong because of a move (e.g. ddl/mod.rs's module list, read/query/mod.rs if it names paths). Minimal edits. Do NOT touch the crate doc in lib.rs.

## Hard rules
- ⛔ NEVER run `git reset` / `git checkout` / `git clean` / `git stash` / `git restore` / `git rm` / `git mv`, or any git command that mutates the working tree or index. Read-only git (`status`, `diff`, `show`, `log`, `ls-files`) is fine. The orchestrator commits.
- ⛔ NO compile/test commands: no `cargo test|build|check|clippy|nextest|doc`, `rustc`, `npm`, `tsc`. CI is the gate. Allowed ONLY: `cargo fmt -p shamir-query-builder` (write; as the very LAST step, once, after everything else) + `-- --check`, and `python -I scripts/ci/check_layout.py` (check mode ONLY — NEVER `--update-baseline`).
- Never silence a lint with `#[allow]`.
- Imports at the top of each file (new files you create). Keep every item's name, signature, visibility, doc comments and attributes byte-exact unless a rule above requires a change.
- Moving a file: `mkdir -p` + `cp <old> <new>` → apply only the required edits to the NEW file → `rm <old>` (plain rm). Never `git mv`/`git rm`.
- Do not touch anything outside `crates/shamir-query-builder/` (no other crate, no scripts/, no docs/, no baseline file).

## Verification before reporting
1. `python -I scripts/ci/check_layout.py` — zero lines mentioning shamir-query-builder.
2. `cargo fmt -p shamir-query-builder -- --check` clean.
3. Re-read every manifest you edited in full (lib.rs, batch/mod.rs, ddl/mod.rs, write/mod.rs, wire/mod.rs, read/mod.rs, every new group mod.rs, every tests/mod.rs): no `pub mod X;` left pointing at an old location; every `mod` decl matches an existing file; every moved module re-exported per F3.

## Report format
- before/after direct-entry counts: src/, batch/, batch/tests/, ddl/, ddl/tests/, write/ (plus any new dir)
- before/after line counts: batch.rs, batch_refs.rs, query_tests.rs, query_conds_tests.rs
- files moved (grouped, old → new), visibility widenings, #[path]/super:: rewrites
- checker output lines for this crate (must be none); anything you could NOT do and why

## Addendum (orchestrator decision, in-flight)
- `validator.rs` → `ddl/schema/validator.rs` (validators are table/column schema rules, matching validator_ops on the types side); declared `pub mod validator;` in `schema/mod.rs`, re-exported `pub use schema::{… validator …};` in `ddl/mod.rs` so `crate::ddl::validator::…` still resolves.
- To keep `schema/` at ≤7 entries, `buffer_config.rs` + `quantization.rs` moved to `ddl/schema/config/` (`config/mod.rs` declares both `pub(crate) mod`; `schema/mod.rs` does `mod config;` + `pub use config::{buffer_config, quantization};`; `ddl/mod.rs` keeps `pub use buffer_config::*;` / `pub use quantization::*;` and re-exports both module names). Final: `ddl/schema/` = 7, `ddl/schema/config/` = 3.
- `#[path]` fix beyond the brief: `ddl/mod.rs`'s `#[path = "alter_repo/create_repo.rs"] mod create_repo;` moved to `repo/mod.rs` as `#[path = "alter_repo/create_repo.rs"] pub(crate) mod create_repo;` (F6: path is relative to the containing file's new directory).
