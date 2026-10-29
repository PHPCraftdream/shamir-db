# 251 — audit-fix-engine-query-regex (P1 audit follow-up: F14 fail-open on unknown fields)

Source: post-merge audit of PR #349 (P1 F14), verified on master. `crates/shamir-engine-query/src/query/filter/evaluation/planning/compile.rs` (~lines 55-100): in `try_compile_filter_with_overlay`, `Filter::Regex/Like/ILike` build the regex ONLY when `intern_path` succeeds; for an unknown field the arm returns `Ok(FilterNode::False)` WITHOUT validating the pattern. Consequence: `try_compile_filter(Not(Regex{field: <unknown>, pattern: "("}))` returns `Not(False)` (= match everything) instead of an error - the exact invalid-pattern-under-NOT fail-open the checked compiler was meant to remove. (The batch preflight protects batch callers only.) The existing direct-compiler regression test interns `"name"` first, so it cannot catch this (`tests/eval_tests/predicates/string_regex_tests.rs` ~201).

## Change
1. In those three arms validate/build the pattern FIRST (before the interner lookup), then fold to `False` for unknown fields. Keep the error text and the size/complexity caps (`build_regex`, `build_like_regex`) identical. Check any sibling code path with the same shape (non-overlay `try_compile_filter`, `compile_filter*` wrappers, `compile_non_pattern`) and fix the whole class.
2. Tests (new file or existing predicate test module, per layout rules): unknown field x {invalid regex, oversized regex, invalid/oversized Like pattern, ILike} x {bare, under Not, under And/Or} all return `Err`; known-field cases unchanged; a valid pattern on an unknown field still folds to `False` without error.

## Rules
Only `shamir-engine-query` (+ its tests). No new files except tests; no `#[allow]`; layout limits; imports at top. Do NOT touch the batch preflight.

## Local gates (only these)
`cargo fmt -p shamir-engine-query -- --check`; `cargo clippy -p shamir-engine-query --all-targets -- -D warnings`; `cargo check -p shamir-engine --all-targets`; `python -I scripts/ci/check_layout.py`. Real exit codes in the report.
