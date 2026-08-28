# shamir-query-builder-macros -- Correctness & TDD-coverage

## Summary

The lowering logic is sound on its happy paths: every emitted path
(`::shamir_query_builder::filter::*`, `query::Query::*`, `write::*`,
`select::*`, `shamir_query_types::call::CallOp`) was verified against the
real builder signatures and matches, and the consumer-side differential
tests (macro output vs. builder output compared over a msgpack/QueryValue
round-trip, plus 3 wire snapshots) are genuine, non-vacuous green-path
coverage. The dominant correctness gap is systemic: `q!`'s hand-rolled
parsers never check that parenthesized/braced sub-streams are fully
consumed, so several classes of malformed input silently compile to a
*different, lossy* op instead of erroring -- on the insert path that is
silent field loss in a database write. TDD-wise, the crate itself contains
zero tests and the entire diagnostic surface (~15 `syn::Error` sites) has
no compile-fail coverage anywhere, so the CLAUDE.md Red/Green/Refactor
discipline was only ever applied to the green paths.

## Findings

### 1. Malformed doc-map / call-args / select-fn args are silently truncated, not rejected (missing `content.is_empty()` checks)

- File:line: `crates/shamir-query-builder-macros/src/query_parse.rs:576-587` (`parse_doc_map`), `:685-700` (`CallMacro::parse` args loop), `:375-436` (`parse_select_item` `count`/`sum`/`avg`/`min`/`max`/`agg_fn`/`func` branches)
- Severity: high
- Issue: `syn` parse buffers do not require full consumption; leftover
  tokens are silently ignored unless checked. Every sub-stream in `q!` is
  parsed with a "peek comma, else break" loop and never verified empty:
  - `parse_doc_map`: after a pair, `if content.peek(Token![,]) { .. } else { break }` -- any leftover (`"b" => 2`, a stray literal) is dropped.
  - `CallMacro` args: same shape -- `call f(1 "x")` keeps only `1`.
  - `count(age 5)` / `count(a, b)` / `sum(a, b)` / `agg_fn("m", a, b)` / `func("n", [x] junk)` / `count(*, x)`: `parse_dotted_ident_from(&content)` / `content.parse::<Expr>()` leave the rest of the paren group unconsumed and unchecked.
- Failure scenario: `q!(insert into users values { "name" => "Alice" "age" => 30 })` (missing comma) compiles cleanly and emits `doc().set("name", "Alice")` only -- a record is inserted with `age` silently missing. `select sum(amount, price) as total` silently aggregates only `amount`. The user's query and the executed query differ with no diagnostic anywhere.
- Suggested fix: after each of these loops/single parses, add
  `if !content.is_empty() { return Err(content.error("q!: unexpected tokens in ...")); }`
  (or parse a trailing `syn::parse::Nothing`). Add compile-fail tests for each site (see finding 3).

### 2. `q!(call ...)` hardcodes `repo: "main"` with no grammar to override it

- File:line: `crates/shamir-query-builder-macros/src/query_parse.rs:911-918` (emission), tested/pinned at `crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs:612` and `:644-648`
- Severity: medium
- Issue: every other statement form supports a repo-qualified table
  (`from main.users`, `insert into main.users`, ...), but the `call`
  grammar has no repo syntax and unconditionally emits
  `repo: ::std::string::String::from("main")`. The hardcode is invisible
  unless you read the macro source; the doc comment never mentions it.
- Failure scenario: a stored procedure that reads `vault.secrets` can only
  be invoked with repo context `"main"` through `q!`. Targeting another
  repo requires hand-constructing `shamir_query_types::call::CallOp { .. }`,
  which violates the workspace-wide "query construction -- builder only"
  rule the macros exist to enforce. The existing tests enshrine `"main"`
  rather than flag the limitation.
- Suggested fix: extend the grammar (e.g. `call repo.name(...)` or a
  `call name in_repo r(...)` form) mapping onto `with_repo`-style
  construction; at minimum document the hardcode in the `q!` doc.

### 3. Zero negative/error-path tests; the entire macro diagnostic surface is unverified (TDD "Red" missing for every error path)

- File:line: crate has no `tests/` directory and no `#[cfg(test)]` code at all; all coverage is consumer-side (`crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs`, `q_macro_tests.rs`) and is 100% happy-path differential testing
- Severity: medium
- Issue: ~15 `syn::Error` diagnostic sites have no compile-fail (trybuild)
  tests: unsupported binary op (`+`), unknown predicate, wrong arity for
  each of the 17 predicates, non-field LHS, tuple-index field access,
  `delete` without `where`, `order_by` missing `asc`/`desc`, clause-order
  violations, trailing garbage, empty `where`. A refactor of
  `filter_lower.rs`/`query_parse.rs` could break every error message or
  accidentally accept/reject the wrong input and the suite stays green.
  Also uncovered green-path branches: `count(*)` without `as` (default
  alias `"count"`, query_parse.rs:958-966 -- behavior not even documented),
  `write_table_tokens`' string-literal-table arm (query_parse.rs:822-831;
  `q!(insert into "weird table" ...)` untested), and the doc-promised bare
  variable RHS in `filter!` (`lib.rs:23-24` "variables -- anything
  `impl Into<FilterValue>`" has no test with a plain identifier RHS).
- Failure scenario: regression in error handling or in an arity/edge check
  ships undetected; the vacuum is exactly what CLAUDE.md's Red step exists
  to prevent.
- Suggested fix: add a trybuild UI-test directory (following the
  CLAUDE.md `tests/` layout, e.g. `src/macros/tests/ui_fail/`) covering
  one `.rs` per diagnostic site; add the missing green-path cases listed above.

### 4. `group_by` / `order_by` reject dotted field paths, unlike `select`/`where` and the builder APIs

- File:line: `crates/shamir-query-builder-macros/src/query_parse.rs:223` (`group_by_fields.push(input.parse::<Ident>()?)`), `:265` (`order_by` field parse)
- Severity: low
- Issue: `select` items and `where` LHS accept `a.b`, and the underlying
  builder accepts dotted paths (`Query::group_by_many` takes
  `IntoFieldPath`; `order_by_asc/desc` take `Into<String>`), but the `q!`
  grammar parses a bare `Ident` here. `q!(from users order_by address.city desc)` fails with the misleading
  "order_by: expected `asc` or `desc` after field name" (pointing at the
  dot). The `q!` doc defines `<field>` via the select section, where
  `field or a.b` is explicit, so the doc implies support.
- Failure scenario: users must abandon the DSL and drop to the raw builder
  for a dotted sort/group key; the error message misdirects them toward
  the direction keyword.
- Suggested fix: parse dotted ident chains in both clauses (reuse
  `parse_dotted_ident_from`) and emit them as single dotted strings (or
  multi-segment paths) as the builder accepts.

### 5. Clause keywords are reserved at the top level of `where`/`having`, with misleading errors

- File:line: `crates/shamir-query-builder-macros/src/query_parse.rs:497-501` + `:547-554` (`is_clause_keyword`)
- Severity: low
- Issue: the raw-token scan for `where`/`having` bodies terminates at any
  top-level `select`/`group_by`/`having`/`order_by`/`limit`/`offset`, so a
  field legitimately named `limit` (a plausible schema field) cannot be
  used un-parenthesized: `q!(from t where limit > 5)` dies with
  "expected a filter expression after `where`"; `where x == limit` dies
  with a raw syn "expected expression". Parenthesized uses work
  (`where (limit > 5)`), which makes the failure extra confusing.
- Failure scenario: always a compile error (never a silent mis-parse --
  that was checked), but the diagnostics don't hint at the cause.
- Suggested fix: document the reserved words in the `q!` doc; when the
  scan breaks with an empty or lopsided token buffer, emit a targeted
  "field name collides with a clause keyword; parenthesize it" error.

### 6. Trailing comma in `group_by`/`select`/`order_by` lists is rejected with a confusing error; `peek_clause_keyword_after_comma` contains dead branches

- File:line: `crates/shamir-query-builder-macros/src/query_parse.rs:561-567`, used at `:224`, `:252`, `:276`
- Severity: low
- Issue: when the fork finds end-of-input (or `asc`/`desc`) after a comma,
  the helper returns true and the loop breaks *without consuming the
  comma*, so the stray comma always survives to the final
  `!input.is_empty()` check and yields "unexpected tokens after query;
  clauses must appear in order...". Meanwhile doc maps *do* allow trailing
  commas (pinned by `q_insert_trailing_comma`), so list syntax is
  inconsistent with map syntax. The `fork.peek(kw::asc)`/`kw::desc` arms
  are unreachable for any sensible input (the `order_by` loop itself
  errors on a missing direction before the comma is ever examined).
- Failure scenario: `q!(from users select a, b,)` errors with a message
  about clause order rather than about the comma.
- Suggested fix: consume the trailing comma before breaking (making lists
  consistent with doc maps), or delete the `fork.is_empty()`/asc/desc arms
  so the list loop stops only at real clause keywords.

### 7. Doc drift: "all 19 predicate calls" (there are 17); `vector_similarity_ef`/`_opts` unreachable from the DSL

- File:line: `crates/shamir-query-builder-macros/src/lib.rs:129-130` vs. `crates/shamir-query-builder-macros/src/filter_lower.rs:134-297`; builder-side gap at `crates/shamir-query-builder/src/filter/leaf.rs:295` and `:313`
- Severity: nit
- Issue: the `q!` doc claims 19 predicate forms; the lowering implements
  17. Additionally `filter::vector_similarity_ef` and
  `filter::vector_similarity_opts` exist in the builder but have no
  `filter!`/`q!` predicate form, so they can only be reached by
  hand-building filters.
- Failure scenario: none at runtime; doc/feature-surface drift misleads
  DSL users about coverage.
- Suggested fix: correct the count in the doc; either add the two
  vector predicates to the lowering or note their absence.
