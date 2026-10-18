# CI-02d — layout sweep verifier (read-only)

Runs AFTER stages 02a/02b/02c. You verify; you fix NOTHING. Scope: `crates/shamir-query-builder-macros/`, `crates/shamir-query-builder/`, `crates/shamir-query-types/` + `scripts/ci/layout_baseline.txt` (must be untouched by the stages).

## Hard rules
- ⛔ NEVER run any git command that mutates the working tree or index; read-only git only (`status`, `diff`, `show`, `log`, `ls-files`).
- ⛔ NO compile/test commands (`cargo test|build|check|clippy|nextest|doc`, `rustc`, `npm`, `tsc`). Allowed: `python -I scripts/ci/check_layout.py` and `cargo fmt -p <crate> -- --check`.
- You do not edit any file. Findings go in the report.

## Procedure
1. `git status --porcelain` + `git diff --stat origin/master` — confirm changed/untracked paths are ONLY inside the three crates. Flag anything else.
2. Run `python -I scripts/ci/check_layout.py` — must exit 0 modulo the untouched out-of-scope baseline entries; no violation line may mention the three crates.
3. For each crate, read IN FULL every file that is new or changed vs origin/master (`git diff --name-only origin/master -- crates/<crate>` plus untracked). Checks per file:
   a. Every `mod`/`pub mod` declaration matches an existing file (`X` → `X.rs` or `X/mod.rs` next to the declaring file).
   b. Every `use` line resolves: trace `crate::…` through lib.rs re-exports → group mod.rs → file; `super::…` resolves in the file's new parent; external crates unchanged.
   c. No item (struct/enum/trait/fn/const/static/mod/macros) that existed in the original file was dropped, and none duplicated: compare item lists `git show origin/master:<old-path>` vs the new location(s).
   d. Test fns: for each moved/changed test file, the set of `#[test]`/`#[tokio::test]` fn names is IDENTICAL to origin/master, and every tests/*.rs is declared in its tests/mod.rs (including new subfolders).
   e. No `#[allow]` was added anywhere.
   f. No behaviour-bearing edit beyond moves: imports, visibility qualifiers, `#[path]` values, `mod`/`pub use` manifest lines, stale module-enumeration doc lines, and the four deliberate file splits. Anything else is a FINDING.
4. Public-path spot checks — for each, confirm the re-export chain makes it resolve (name the chain in the report):
   - builder: `shamir_query_builder::{query::Query, filter::and, val::lit, cursor::CreateCursorBuilder-or-actual-cursor-type, visibility::Visibility, response::…, write::insert, batch::build_error::BuildError, batch::handle::Handle, ddl::create_index::…, ddl::res::database, ddl::interner_resolve::…}`
   - types: `shamir_query_types::{batch::planner::BatchPlanner, batch::batch_op::BatchOp, batch::batch_error::BatchError, batch::batch_request::BatchRequest, batch::options::IsolationLevel, read::ReadQuery, read::limit::Limit, filter::Filter, table_ref::TableRef, write::InsertOp, call::CallOp, subscribe::SubscribeOp, admin::types::db_ops::…, admin::types::repl_ops::…, validator::WriteOp, durability::DurabilityOverride}` (use the actual type names found in origin/master)
   - macros: `shamir_query_builder_macros::q` proc-macro entry still calls through to the moved `q_macro`.
5. `cargo fmt -p shamir-query-builder -- --check`, `cargo fmt -p shamir-query-types -- --check`, `cargo fmt -p shamir-query-builder-macros -- --check` — all clean.
6. Line/entry audit: confirm no in-scope file >1000 lines and no in-scope dir >7 direct entries (the checker covers this; also spot-check `wc -l` on the four split files' parts).

## Report format
- VERDICT: PASS or FAIL per crate (02a/02b/02c) with file:line evidence for every finding
- the public-path chains verified in step 4
- checker output tail + fmt check results
- any deviation from the stage briefs you observed
