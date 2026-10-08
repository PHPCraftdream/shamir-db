<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder-macros — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Ordinary rejections use structured compile errors and scoped leftovers are rejected. Diagnostic coverage remains missing. The suggested depth guard requires a non-recursive error-construction path.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 4 | 0 | 0 | 0 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — No error-path test coverage for any diagnostic branch of `filter!` / `q!`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

No reachable negative harness was found. Registered accepted-input tests cannot catch changing unknown-predicate rejection into acceptance or weakening delete's required-WHERE rule. This is a coverage gap, not evidence of current high-impact corruption.

Evidence: [crates/shamir-query-builder-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/Cargo.toml#L13); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/mod.rs#L2); [crates/shamir-query-builder-macros/src/filter_lower.rs:299](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L299); [crates/shamir-query-builder-macros/src/query_parse.rs:637](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L637).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Unknown function-like select item produces a misleading clause-order error

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

select myfunc(x) consumes myfunc as a field after unknown-name fallthrough, leaving parentheses for the outer query error. It is rejected, but the message does not identify the unsupported select function.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:433](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L433); [crates/shamir-query-builder-macros/src/query_parse.rs:441](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L441); [crates/shamir-query-builder-macros/src/query_parse.rs:300](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L300).

<a id="review-3"></a>

### Claim 3 — Field-path and alias spans are discarded; downstream errors point at the macro call site

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Identifier spans are lost when converted to String; quote 1.0.45 ToTokens for str creates a new literal. The exact future compiler underline is unverified. Published source: https://docs.rs/crate/quote/1.0.45/source/src/to_tokens.rs.

Evidence: [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660); [crates/shamir-query-builder-macros/src/filter_lower.rs:333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L333); [crates/shamir-query-builder-macros/src/query_parse.rs:947](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L947); [crates/shamir-query-builder-macros/src/query_parse.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1003).

<a id="review-4"></a>

### Claim 4 — Unbounded recursion in expression lowering; pathological nesting aborts rustc instead of erroring

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Depth-dependent recursion is present, but accepted depth and abort behavior are not proven. A lowering cap alone also leaves parsing, recursive diagnostic rendering and AST destruction outside its protection.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L26); [crates/shamir-query-builder-macros/src/filter_lower.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L102); [crates/shamir-query-builder-macros/src/filter_lower.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L337); [crates/shamir-query-builder-macros/src/filter_lower.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L43); [Cargo.lock:4023](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4023).

<a id="review-5"></a>

### Claim 5 — Reused diagnostics carry the wrong context at some call sites

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

An empty HAVING expression reports 'after where'; like("status", pattern) reports a comparison-LHS problem. Both inputs are correctly rejected, so this is message specificity only.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L235); [crates/shamir-query-builder-macros/src/query_parse.rs:540](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L540); [crates/shamir-query-builder-macros/src/filter_lower.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L143); [crates/shamir-query-builder-macros/src/filter_lower.rs:351](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L351).

## Evidence and recipe corrections

- No explicit own-crate panic calls is supported; 'never panic on input' is stronger and unproved. Emitted Doc::set also contains runtime expect calls outside this crate.
- The historical depth-cap recipe uses Error::new_spanned(expr). syn 2.0.114 src/error.rs:194 calls into_token_stream, and ExprParen/ExprUnary printing recursively visits children. Use an already available token span for a bounded error, and separately account for parsing/destruction. Published sources: https://docs.rs/crate/syn/2.0.114/source/src/error.rs and https://docs.rs/crate/syn/2.0.114/source/src/expr.rs.
- A fixture directory is not registration. An integration-only harness is excluded by default --lib mode.
- Unknown select-function rejection comes directly from the outer-stream check, not scoped-buffer Drop on that branch.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder-macros -- Error handling & resource lifecycle

## Summary

The crate is disciplined on the Result/panic axis and fully conforms to CLAUDE.md's error-handling rules: every fallible path returns `syn::Result`, both proc-macro entries use `parse_macro_input!` + `to_compile_error()` (spanned compile errors, never panic-on-input), and a grep confirms zero `unwrap()`/`expect()`/`panic!` in real code (the only `.unwrap()`s are inside ```ignore```-fenced doc examples, and `doctest = false` is set). Resource lifecycle is trivially satisfied: a proc-macro crate owns no runtime resources (no files, locks, tasks, channels), so error-path cleanup is N/A; syn-sub-buffer discipline (leftover tokens in `parenthesized!`/`braced!` scopes) is surfaced by syn 2's `check_unexpected()` machinery (verified against the resolved syn 2.0.114 source), not silently dropped. The one real gap for this theme is that none of the ~25 distinct diagnostic branches is pinned by any test: the crate has no `tests/` directory and no compile-fail harness anywhere in the workspace.

## Findings

### 1. No error-path test coverage for any diagnostic branch of `filter!` / `q!`
- **File:line:** `crates/shamir-query-builder-macros/Cargo.toml:1-19` (no `[dev-dependencies]`, no `[[test]]`); `crates/shamir-query-builder-macros/src/` (no `tests/` directory at all)
- **Severity:** high
- **Issue:** The crate's entire product is compile-time diagnostics, yet ~25 distinct error paths are completely untested: unknown statement keyword (`query_parse.rs:197-199`), `order_by` missing `asc`/`desc` (`:273`), clause-order violation (`:300-305`), empty `where` expression (`:539-541`), `insert`/`update`/`delete`/`upsert`/`call` trailing-token errors (`:601-603`, `:621-623`, `:644-646`, `:662-664`, `:702-704`), `delete` without `where` (`:637-641`), required-alias errors for aggregates (`:469-478`), unknown predicate (`filter_lower.rs:299-307`), per-predicate arity errors (`:137-141`, `:153-157`, `:168-172`, `:183-187`, `:199-203`, `:215-218`, `:231-234`, `:246-250`, `:263-266`, `:280-285`), unsupported binary/unary operators (`:83-86`, `:107-111`), unsupported-expression catch-all (`:43-49`), non-path predicate callee (`:121-129`), invalid/tuple-index field path (`:343-346`, `:349-353`). There is no `trybuild` dev-dependency anywhere in the workspace; the only macro tests (in the consumer, `crates/shamir-query-builder/src/macros/tests/`) are compile-pass wire-shape tests, and the repo's own release audit (`docs/dev-artifacts/research/2026-07-17-release-audit/08-test-coverage-ci-robustness.md:260`) already acknowledges "no trybuild/UI tests pinning" macro diagnostics. This violates the repo's TDD protocol (CLAUDE.md "Protocol of development") for exactly the half of the DSL that is error behavior: any refactor (e.g. adding a predicate, touching `parse_filter_expr`'s clause-terminator logic) can silently regress a diagnostic's text, trigger site, or turn a rejection into an acceptance without any test noticing.
- **Failure scenario:** A refactor changes `is_clause_keyword`/`peek_clause_keyword_after_comma` or one arity check; a malformed `q!(...)` that today produces a clean compile error starts being accepted (or vice versa) and the gate stays green.
- **Suggested fix:** Add a `trybuild` dev-dependency (to this crate or the consumer; cargo permits cyclic dev-deps, so pointing at `shamir-query-builder` re-exports avoids a workspace cycle concern) plus a `tests/compile_fail/*.rs` fixture per branch listed above, each with the expected diagnostic string asserted. Per CLAUDE.md, follow the `tests/` layout (manifest-only `mod.rs`, topic files like `filter_error_tests.rs` / `q_error_tests.rs`) and run through `./scripts/test.sh` (trybuild tests are lib/test-target based, so the wrapper covers them).

### 2. Unknown function-like select item produces a misleading clause-order error
- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:433-435` (fallthrough), error surfaced at `:300-305`
- **Severity:** low
- **Issue:** In `parse_select_item`, an `ident(...)` whose name is not one of `count|sum|avg|min|max|agg_fn|func` falls through to plain-field parsing (`_ => {}` arm), which consumes only the ident and leaves the call parens unconsumed. The select-items loop then breaks (next token is `(`, not `,`), and the enclosing `!input.is_empty()` check fires with "q!: unexpected tokens after query; clauses must appear in order: from, where, group_by, ...". The input is correctly rejected (no silent acceptance), but the message tells the user their clause order is wrong when the real problem is an unrecognized function name.
- **Failure scenario:** `q!(from t select myfunc(x))` — user hunts for a misplaced clause instead of checking the supported select-function list.
- **Suggested fix:** In the `_` arm, when the fork confirmed `Ident` followed by `Paren`, return a targeted error immediately, e.g. `input.error(format!("q!: unknown select function `{id_str}`; use count, sum, avg, min, max, agg_fn, or func"))`.

### 3. Field-path and alias spans are discarded; downstream errors point at the macro call site
- **File:line:** `crates/shamir-query-builder-macros/src/filter_lower.rs:317-327` (`field_path`), `:330-355` (`collect_field_segments` collects `String`s); `crates/shamir-query-builder-macros/src/query_parse.rs:1003-1011` (`segments_to_field_path`), `:947`, `:974`, `:987`, `:994` (alias `to_string()`)
- **Severity:** low
- **Issue:** LHS field segments and select aliases are flattened to `String` and re-emitted via `quote!`, so the generated string literals carry `Span::call_site()` instead of the user's original ident spans. The RHS of comparisons is quoted verbatim (spans preserved), but any downstream type error that lands on the generated field-path/alias token — e.g. a `filter::*` or `select::*` signature mismatch after a builder API change, or a bad `Into<FilterValue>` impl on the field side — is attributed to the whole macro invocation rather than the specific field token the user wrote.
- **Failure scenario:** `filter!(status == "active")` fails to compile because `filter::eq`'s field parameter changed; rustc underlines the entire `filter!(...)` call with no pointer to `status`, unlike the RHS which would underlined correctly.
- **Suggested fix:** Preserve identity spans when materializing the literal: `syn::LitStr::new(&s, ident.span())` instead of `id.to_string()` + `quote!{ #s }` in `field_path`, `segments_to_field_path`, and the alias sites. Mechanical, no behavior change.

### 4. Unbounded recursion in expression lowering; pathological nesting aborts rustc instead of erroring
- **File:line:** `crates/shamir-query-builder-macros/src/filter_lower.rs:26-51` (`lower` recursion), `:330-348` (`collect_field_segments` recursion)
- **Severity:** low
- **Issue:** `lower()` and `collect_field_segments()` recurse once per nesting level of user input with no depth cap. A pathologically deep filter (hundreds of thousands of nested parens/negations in a single macro invocation) can overflow the proc-macro thread stack, which manifests as an abrupt rustc abort (stack overflow) rather than a spanned compile error. Depth is bounded by whatever limits syn's own `Expr` parser applies during *parsing*, but the lowering recursion itself is uncapped, and stack-overflow-on-deep-input is the classic proc-macro crash class (the same one that motivated syn's own recursion guard for types). Input here is first-party code, so this is hardening, not an exploit vector.
- **Failure scenario:** A generated or copy-pasted filter with extreme nesting wedges the developer's build with an OS-level stack-overflow message and no file/line attribution.
- **Suggested fix:** Thread a `depth: usize` parameter through `lower`/`collect_field_segments`, incrementing at each recursive entry, and return `Err(syn::Error::new_spanned(expr, "filter!: expression is too deeply nested"))` past a cap (e.g. 128). Cheap, and converts the worst case into an ordinary diagnostic.

### 5. Reused diagnostics carry the wrong context at some call sites
- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:539-541` ("after `where`" fires from the `having` path at `:233-239`); `crates/shamir-query-builder-macros/src/filter_lower.rs:349-353` ("LHS of comparison" fires for predicate-call field arguments, e.g. `like("status", ...)` via `:143`)
- **Severity:** nit
- **Issue:** `parse_filter_expr`'s empty-token error hardcodes "expected a filter expression after `where`" but the function also serves `having`; `collect_field_segments`' catch-all hardcodes "LHS of comparison must be a field name" but is also the validator for the field argument of every predicate call. Rejection behavior is correct; only the message text is misleading in the secondary contexts.
- **Suggested fix:** Parameterize the context word (`parse_filter_expr(input, clause: &str)`) and give the predicate-argument path its own message ("predicate field must be an ident or dotted field path"), or split into two thin wrappers.

## Non-findings (checked and clean, for the record)

- **Panic avoidance:** zero `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!` in non-doc code; every user-input rejection is a spanned `syn::Error` funneled through `to_compile_error()`.
- **Leftover tokens in nested `parenthesized!`/`braced!` buffers** (e.g. `count(age junk)`, `{ "a" => 1 "b" => 2 }`, `call f(1 2)`): verified against resolved syn 2.0.114 (`Cargo.lock:4011`) — `ParseBuffer::drop` flags them into the `unexpected` chain and `Parser::parse2::check_unexpected` turns them into "unexpected token, expected `)`/`}`" compile errors. Not silent, not a panic; no finding.
- **thiserror/anyhow discipline:** N/A by construction — a proc-macro crate's error type is `syn::Error`; CLAUDE.md's `thiserror`/`anyhow` rules have no surface to apply to here.
- **Resource cleanup on error paths:** N/A — no files, locks, async tasks, or allocations outliving the macro expansion; `parse_macro_input!` early-returns are the only abort path and hold no resources.

</details>
