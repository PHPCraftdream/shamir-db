# CI-02a — layout sweep stage: shamir-query-builder-macros

Part of CI-02 (see `02-layout-sweep-query.md`). Scope: `crates/shamir-query-builder-macros/` ONLY. Pure structural refactor — behaviour, public API paths, error behaviour, test semantics and test names stay identical.

## Baseline violations to resolve
- `file crates/shamir-query-builder-macros/src/query_parse.rs 1019` (limit 1000)

Current `src/` has 3 entries (filter_lower.rs, lib.rs, query_parse.rs) — within limits.

## Task: split query_parse.rs along the parse/lower seam

Current layout (line numbers from origin/master):
- L1–63: module doc + imports
- L64–181: AST structs (QueryMacro, DocMapAst, InsertMacro, UpdateMacro, DeleteMacro, UpsertMacro, CallMacro)
- L182–322: `impl Parse for QMacro` / `impl Parse for QueryMacro`
- L323–589: parse helpers (parse_table_arg, parse_select_item, parse_optional_as, parse_required_as, parse_dotted_ident_from, parse_filter_expr, is_clause_keyword, peek_clause_keyword_after_comma, parse_doc_map)
- L590–711: `impl Parse for Insert/Update/Delete/Upsert/Call`
- L712–1002: lowering (`impl QueryMacro` lower fn, lower_doc_map, write_table_tokens, lowering impls for Insert/Update/Delete/Upsert/Call, `impl QMacro`, lower_select_item, segments_to_field_path)
- L1013–1019: `pub fn q_macro` entry point

Split:
- `src/query_parse.rs` KEEPS: module doc, imports, all AST structs, all `impl Parse` blocks, all parse_* helpers, is_clause_keyword, peek_clause_keyword_after_comma, parse_doc_map. Must end ≤1000 lines.
- NEW `src/query_parse_lower.rs`: the whole lowering section (L712–1002) plus `pub fn q_macro` moved verbatim. Starts with a 2–4 line doc comment: "Lowering half of the `q!` macro — see [`crate::query_parse`] for the parse half and the module docs." Imports at top: only what the moved code actually uses (grep identifiers per import; an unused import fails CI clippy `-D warnings`).
- Items shared between the two files (AST structs, helpers) stay in query_parse.rs; widen visibility ONLY where the split requires it (private → `pub(crate)`), record every widening in the report.
- query_parse_lower.rs imports them explicitly: `use crate::query_parse::{...};` at the top.
- Keep `src/lib.rs` UNCHANGED: lib.rs L10 declares `mod query_parse;` and L173 calls `query_parse::q_macro(input)`. Make both keep working by adding to query_parse.rs (after its imports): `pub(crate) use crate::query_parse_lower::q_macro;`. Read lib.rs first to confirm; if lib.rs needs a one-line `mod query_parse_lower;` addition instead, that is acceptable — prefer zero lib.rs changes.
- Result: `src/` has 4 entries; both files ≤1000 lines.

## Hard rules
- ⛔ NEVER run `git reset` / `git checkout` / `git clean` / `git stash` / `git restore` / `git rm` / `git mv`, or any git command that mutates the working tree or index. Read-only git (`status`, `diff`, `show`, `log`, `ls-files`) is fine. The orchestrator commits.
- ⛔ NO compile/test commands: no `cargo test|build|check|clippy|nextest|doc`, `rustc`, `npm`, `tsc`. CI is the gate. Allowed ONLY: `cargo fmt -p shamir-query-builder-macros` (write; as the very LAST step) + `-- --check`, and `python -I scripts/ci/check_layout.py` (check mode ONLY — NEVER `--update-baseline`).
- Never silence a lint with `#[allow]`.
- Imports at the top of each file. Keep every item's name, signature, visibility, doc comments and attributes byte-exact unless the split explicitly requires a change.
- Do not touch anything outside `crates/shamir-query-builder-macros/`.

## Verification before reporting
1. `python -I scripts/ci/check_layout.py` — zero lines mentioning shamir-query-builder-macros.
2. `cargo fmt -p shamir-query-builder-macros -- --check` clean (after running `cargo fmt -p shamir-query-builder-macros` once).
3. Re-read both files in full: every item from the original exists exactly once across the two files; q_macro reachable as `crate::query_parse::q_macro`.

## Report format
- before/after line counts of query_parse.rs and query_parse_lower.rs
- list of items moved / kept, any visibility widenings (item, from → to)
- the exact lib.rs outcome (unchanged / what changed)
- checker output lines for this crate (must be none)
