<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder-macros — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The local import violation, stale predicate count, diagnostic-test/discoverability gap and small visibility/emitter nits remain. The single large parser/codegen file is permitted as a closely coupled group and does not establish a mandatory structural defect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 4 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Mid-function `use syn::BinOp;` violates "Imports at the top"

Status: `confirmed-open`. Current risk: `nit`.

The import remains inside lower_binary and none of the documented exceptions applies. This is a convention violation with no demonstrated runtime consequence; medium severity is unwarranted.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:56](../../../../../crates/shamir-query-builder-macros/src/filter_lower.rs#L56); [CLAUDE.md:615](../../../../../CLAUDE.md#L615).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `q!` rustdoc claims "all 19 predicate calls"; 17 exist

Status: `confirmed-open`. Current risk: `nit`.

The count remains 19 in rustdoc while the whitelist implements the same 17 documented predicate names.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:129](../../../../../crates/shamir-query-builder-macros/src/lib.rs#L129); [crates/shamir-query-builder-macros/src/filter_lower.rs:134](../../../../../crates/shamir-query-builder-macros/src/filter_lower.rs#L134).

Grouping/duplicate: `SUMMARY.md#1.4`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Zero tests in the crate; error-path branches untested anywhere

Status: `confirmed-open`. Current risk: `low`.

The macro crate still has no tests and no documentation pointer to consumer coverage. Consumer test registration is correct, but ordinary rejection paths remain untested. Absence of local tests alone is not a layout violation.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:1](../../../../../crates/shamir-query-builder-macros/src/lib.rs#L1); [crates/shamir-query-builder-macros/Cargo.toml:13](../../../../../crates/shamir-query-builder-macros/Cargo.toml#L13); [crates/shamir-query-builder/src/macros/mod.rs:191](../../../../../crates/shamir-query-builder/src/macros/mod.rs#L191); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](../../../../../crates/shamir-query-builder/src/macros/tests/mod.rs#L2).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `query_parse.rs` is a 1,019-line, five-role file — strains "one file = one primary export"

Status: `not-applicable`. Current risk: —.

The file remains 1,019 lines, but its private AST, parser and generator form one closely coupled q! implementation, expressly allowed by the rule. Merge/blame costs were not measured. Splitting is optional organization, not remediation of a proven violation.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:64](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L64); [crates/shamir-query-builder-macros/src/query_parse.rs:182](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L182); [crates/shamir-query-builder-macros/src/query_parse.rs:1013](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L1013); [CLAUDE.md:498](../../../../../CLAUDE.md#L498).

Grouping/duplicate: `SUMMARY.md#7.4`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Unjustified `pub`, redundant wrapper, duplicated field-path emitter

Status: `confirmed-open`. Current risk: `nit`.

field_path remains unnecessarily pub within a private module, and segment-to-token emission remains duplicated. lower_expr is an internal public entry wrapping lower; removing that seam is optional, not a correctness fix. No divergent output behavior was found.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:9](../../../../../crates/shamir-query-builder-macros/src/lib.rs#L9); [crates/shamir-query-builder-macros/src/filter_lower.rs:20](../../../../../crates/shamir-query-builder-macros/src/filter_lower.rs#L20); [crates/shamir-query-builder-macros/src/filter_lower.rs:317](../../../../../crates/shamir-query-builder-macros/src/filter_lower.rs#L317); [crates/shamir-query-builder-macros/src/query_parse.rs:1003](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L1003).

Grouping/duplicate: `SUMMARY.md#7.5`. This row is not another independent defect.

## Corrections and qualified non-findings

- The import and stale-count issues are nits, not medium runtime defects.
- The closely-coupled-group exception positively permits the current file organization; an optional split should not remain in mandatory defect counts.
- Tuple-index rejection belongs to Expr validation; the query parser's Ident-based grammar independently excludes numeric tuple segments. Its absence from the shared emission-shaped helper is not proof of inconsistent acceptance.
- Consumer modules use manifest-only test registration, and the proc-macro delegates remain thin. No inline test-module, own-crate mod.rs or stray-file problem was found.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder-macros -- Style & CLAUDE.md structural conformance

## Summary

The crate is structurally close to CLAUDE.md: `lib.rs` holds only thin `#[proc_macro]` delegates plus rustdoc, each macro's logic lives in its own sibling file (`filter_lower.rs`, `query_parse.rs`), there are no `mod.rs` files to police, imports are at file top everywhere except one site, and the `// ── section ──` banner/comment discipline is clean with no stray debug files or inline `#[cfg(test)] mod tests`. Behavioural coverage for both macros is thorough (all 6 comparisons, logical ops/precedence, and all 17 predicate names exercised via wire-equivalence tests), but it lives entirely in the consumer crate (`shamir-query-builder/src/macros/tests/`) — this crate ships zero tests, and only happy paths are covered anywhere. Three concrete deviations: one mid-function `use`, one stale doc count ("19" predicates vs 17), and a minor visibility/duplication nit.

## Findings

### 1. Mid-function `use syn::BinOp;` violates "Imports at the top"
- **File:line:** `src/filter_lower.rs:56` (inside `fn lower_binary`)
- **Severity:** medium
- **Issue:** CLAUDE.md ("📦 Imports at the top") requires all `use` statements in the file header, "never inside a function or block body", with three documented exceptions (test `use super::*`, trait-for-one-method with a collision comment, `cfg`-gated bodies). None applies: `BinOp` is an enum used for pattern matching, hoisting collides with nothing (no other `BinOp` in the file), and the body is not macro-generated/`cfg`-gated.
- **Failure scenario:** none functional; it normalises the exception and erodes the auditability of the convention (`grep '^use '` header checks silently miss it).
- **Suggested fix:** merge `BinOp` into the header import (`use syn::{parse_macro_input, BinOp, Expr};`) and delete line 56.

### 2. `q!` rustdoc claims "all 19 predicate calls"; 17 exist
- **File:line:** `src/lib.rs:129`
- **Severity:** low
- **Issue:** the `where`/`having` doc says the grammar covers "all 19 predicate calls", but both the `filter!` doc (`lib.rs:32-40`) and every error string in `filter_lower.rs` (e.g. lines 124-127, 302-305) enumerate exactly 17: like, ilike, regex, is_null, is_not_null, exists, not_exists, contains, contains_any, contains_all, in_, not_in, between, fts, vector_similarity, computed, computed_with_args.
- **Failure scenario:** reader assumes two predicates are missing (or stops trusting the doc); doc/API drift accumulates.
- **Suggested fix:** change "19" to "17" (or drop the count: "all supported predicate calls").

### 3. Zero tests in the crate; error-path branches untested anywhere
- **File:line:** `crates/shamir-query-builder-macros` (no `src/**/tests/`, no `tests/`); error branches at `src/filter_lower.rs:83-86,107-111,134-307` and `src/query_parse.rs:300-306,469-478,539-541,637-641`
- **Severity:** low
- **Issue:** every macro test is positive-path and lives in the consumer crate, `shamir-query-builder/src/macros/tests/{filter_macro_tests.rs,q_macro_tests.rs}` (the layout there conforms — `tests/mod.rs` is a manifest-only re-export file). None exercise a single `syn::Error` branch: unsupported operator, non-`!` unary, unknown predicate / wrong arity, empty `where`, clause-order violation, `delete` without `where`, aggregate missing `as`. Placement outside the proc-macro crate is legitimate (emitted tokens reference `::shamir_query_builder`, unresolvable from inside this crate), but nothing points a reader there.
- **Failure scenario:** a refactor of an error branch (reworded or tightened arity/unknown-predicate checks, changed clause-order handling) regresses diagnostics with no failing test; a contributor looking for macro tests in this crate finds none.
- **Suggested fix:** add compile-fail coverage (trybuild as a dev-dependency — of the consumer or of this crate; cargo permits cyclic dev-deps) for the main error branches, and add one line to the `lib.rs` crate docs: "Tests: `shamir-query-builder/src/macros/tests/`".

### 4. `query_parse.rs` is a 1,019-line, five-role file — strains "one file = one primary export"
- **File:line:** `src/query_parse.rs:1-1019`
- **Severity:** low
- **Issue:** the file holds the `kw` keyword module (38-60), ~10 private AST types (64-178), six `Parse` impls (182-708), token-buffering helpers (493-567), and all codegen (710-1019). Within the letter of CLAUDE.md this is one closely-coupled group serving a single macro with all types private, so it is not a violation — but the rule's stated motivation ("atomic diffs and meaningful `git blame`") is strained: Read-grammar edits and Insert codegen land in one file.
- **Failure scenario:** blame noise and merge contention as the DSL grows.
- **Suggested fix (optional):** split into `q_ast.rs` / `q_parse.rs` / `q_gen.rs` siblings with `lib.rs` wiring them — only as a dedicated task, honouring "no new files unless the task genuinely needs them".

### 5. Unjustified `pub`, redundant wrapper, duplicated field-path emitter
- **File:line:** `src/filter_lower.rs:20-22,317`; `src/query_parse.rs:1003-1011`
- **Severity:** nit
- **Issue:** (a) `pub fn field_path` (`filter_lower.rs:317`) has no caller outside its own file — `query_parse.rs` uses its own `segments_to_field_path` instead — so it should be private (since `mod filter_lower;` is private it never leaks, but minimal visibility expresses intent). (b) `lower_expr` (`filter_lower.rs:20-22`) is a zero-value wrapper around `lower`; give it the body directly. (c) The output-emission half is duplicated between `field_path`/`collect_field_segments` (single segment → bare string, multi → `[a, b]`) and `segments_to_field_path`; the tuple-index rejection exists only in the former, and the two can drift.
- **Failure scenario:** the two emitters diverge silently (one gains validation/quoting, the other does not).
- **Suggested fix:** make `field_path` private (or have `query_parse.rs` reuse it), inline the `lower_expr` wrapper, and consolidate the segment-array emission into one helper.

</details>
