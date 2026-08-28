# shamir-query-builder-macros -- API & wire-protocol design

## Summary

The macro surface is well-aligned with the runtime builder: every emitted path
(`filter::*`, `query::Query::*`, `select::*`, `write::*`) was cross-checked
against `shamir-query-builder` and matches an existing public constructor with
compatible arity, and `q!`/`filter!` output is wire-shape tested against
hand-built builder equivalents (all 17 predicates, all 6 statement forms, plus
msgpack snapshots) in `shamir-query-builder/src/macros/tests/`. The two real
gaps are (1) a family of silent token-drop bugs in the DSL sub-parsers that can
miscompile a write op without any compile error, and (2) `q!(call ...)`, which
is the one lowering that bypasses the `::shamir_query_builder` path contract the
crate documents for itself, hand-assembles the `CallOp` wire DTO, and pins
`repo: "main"` with no repo-qualified syntax. The crate itself contains no
tests — coverage lives (correctly, given the cycle constraint) in the builder
crate, but only on the happy path; error paths are untested.

## Findings

### 1. Silent token-drop in DSL sub-parsers miscompiles write ops (missing-comma in doc maps, call args, select-item args)

**File:line:** `src/query_parse.rs:572-588` (`parse_doc_map`), same pattern at `:689-696` (`CallMacro` args), `:409-431` (`agg_fn`/`func` select items), `:481-488` (`parse_dotted_ident_from` — no exhaustion check); consumed by insert/update/upsert (`:590-671`).

**Severity:** high

**Issue:** Every sub-parser that consumes a delimited group element-by-element does `if content.peek(Token![,]) { ... } else { break; }` — and on `break` never checks that `content` is exhausted. Leftover tokens inside the group are silently discarded when the group is dropped:

- `q!(insert into users values { "name" => "Alice" "age" => 30 })` (missing comma) parses as a **one-field** insert; the `"age" => 30` pair vanishes. Identically for `update ... set {...}` and `upsert ... key/value`.
- `q!(call f(1 2))` yields `params: [1]`; `2` is dropped.
- `q!(from u select agg_fn("median", age dup) as m)` and `func("ns", [col(a)] junk) as f` silently ignore the trailing tokens; `count(field junk)` likewise (the sub-stream is never checked for exhaustion).

**Failure scenario:** A user omits one comma in an insert/update/upsert doc map. The macro compiles cleanly, the wire op carries fewer fields than the user wrote, and the wrong data is committed at runtime — the worst failure mode for a write path (silent, no compile error, no runtime error). For a DSL whose entire purpose is to make malformed queries unrepresentable, malformed input must be a compile error.

**Suggested fix:** After parsing each element, require either end-of-group or a comma:

```rust
if content.is_empty() { break; }
if !content.peek(Token![,]) {
    return Err(content.error("q!: expected `,` between doc-map pairs"));
}
content.parse::<Token![,]>()?;
```

Apply the same exhaust-or-comma rule to `CallMacro`'s arg loop, and add a final `if !content.is_empty() { return Err(...) }` after `parse_dotted_ident_from` / the `func`-args `Expr` parse. Add error-path tests (there are currently none; `q_insert_trailing_comma` at `shamir-query-builder/src/macros/tests/q_macro_tests.rs:686-695` only covers the valid trailing comma).

### 2. `q!(call ...)` violates the crate's own emitted-path contract: expansion requires a direct `shamir-query-types` dependency

**File:line:** `src/lib.rs:1-5` (contract) vs `src/query_parse.rs:911-918` (violation).

**Severity:** medium

**Issue:** The crate doc states: "These macros emit **fully-qualified paths** (`::shamir_query_builder::...`) so they work from any crate that depends on `shamir-query-builder`." The read/insert/update/delete/upsert lowerings honor this; `CallMacro::to_tokens` alone emits `::shamir_query_types::call::CallOp { ... }` and `::std::convert::Into::<::shamir_query_types::filter::FilterValue>::into(...)`. `shamir-query-builder` deliberately re-exports wire DTOs so guests don't need `shamir-query-types` (`shamir-query-builder/src/lib.rs:66-68`) and re-exports the macros themselves (`:79`), but does **not** re-export `CallOp` or `FilterValue` (verified: only `FnCall` is re-exported, via `val/filter_value.rs:7`).

**Failure scenario:** An external consumer (WASM guest, SDK user) whose `Cargo.toml` lists only `shamir-query-builder` — exactly the setup the doc comment promises to support — writes `q!(call my_proc(1))` and gets an unresolved-crate compile error pointing at macro-generated tokens, i.e. the least actionable error site. Inside this workspace every q! user happens to also depend on `shamir-query-types`, so the trap is invisible locally.

**Suggested fix:** Either (a) add `pub use shamir_query_types::call::CallOp;` and `pub use shamir_query_types::filter::FilterValue;` to `shamir-query-builder`'s root (consistent with the existing DTO re-export rationale) and emit `::shamir_query_builder::...` paths in `CallMacro`, or (b) amend `src/lib.rs:1-5` to document the exception and the extra dependency requirement. (a) is preferable — it restores a single invariant for all six statement forms.

### 3. `q!(call ...)` bypasses the builder layer entirely and pins `repo: "main"` with no repo-qualified syntax

**File:line:** `src/query_parse.rs:899-921` (struct-literal construction, `repo` hardcode at `:917`); grammar doc `src/lib.rs:102-111`; builder counterparts `shamir-query-builder/src/batch/batch.rs:686-714` (`Batch::call` / `Batch::call_in_repo`).

**Severity:** medium

**Issue:** All five other statement forms lower into `shamir-query-builder` constructors (`Query::from`/`with_repo`, `write::Insert::into`/`with_repo`, ...); `call` alone hand-assembles the raw wire DTO via struct literal. Two consequences:

- **Repo cap:** `repo: String::from("main")` is pinned at expansion time. Every other statement supports repo qualification (`from main.users`, `insert into main.users`, ..., lowering to `with_repo`), and the builder layer itself offers `call_in_repo` — but the `q!` grammar has no repo form for `call`, so a stored procedure outside the default repo is inexpressible in the DSL.
- **Versioning coupling:** the struct literal hard-couples every expansion site to `CallOp`'s exact field set (`shamir-query-types/src/call/mod.rs:31-43`, not `#[non_exhaustive]`). Adding a field to `CallOp` breaks all downstream `q!(call ...)` uses with raw struct-literal errors, and the macro would keep emitting a stale literal even if the DTO's serde `default_repo()` (the wire-level default, `call/mod.rs:13-15`) ever changed — silently overriding the wire default instead of inheriting it. This is also the one place the builder-only construction rule (CLAUDE.md "Query construction — builder only", incl. its "state why in a comment" requirement) is bypassed without a justification comment.

**Failure scenario:** User needs `call` in repo `"analytics"`; no syntax exists, so they must fall back to hand-building `CallOp` (or `Batch::call_in_repo`) — re-introducing exactly the hand-assembled wire op the DSL exists to prevent. Separately, any future `CallOp` field addition turns into a workspace-wide compile break at every expansion site.

**Suggested fix:** Extend the grammar consistently with tables — e.g. `q!(call main.my_proc(...))` / `q!(call main."reports/daily"(...))` lowering `repo` from the prefix — and emit the DTO through a single constructor (e.g. add `CallOp::new(name, params)` / a `query_builder::write`-level `call`/`call_in_repo` free function) that the macro calls, so field-set evolution has one owner. If the struct literal is kept deliberately, add the one-line "why" comment the convention requires.

### 4. Clause keywords are silently reserved inside `q!` where/having, contradicting the documented "full `filter!` expression grammar"

**File:line:** `src/query_parse.rs:497-501` (terminator check) and `:547-554` (`is_clause_keyword`); doc claim `src/lib.rs:125-130`.

**Severity:** low

**Issue:** `parse_filter_expr` stops at `select`/`group_by`/`having`/`order_by`/`limit`/`offset` anywhere at token depth 0 — including when those tokens are *field names*. `q!(from users where limit == 5)` breaks immediately ("expected a filter expression after `where`"), `q!(... where status == 1 && order_by == 2)` produces a misleading `syn` "expected expression" on the truncated token stream, and even a dotted segment `where a.select == 1` breaks. The same field names work fine in standalone `filter!` (whose `field_path` has no reserved words), so the doc's "Both use the full `filter!` expression grammar" overpromises. Field names are arbitrary strings in a document DB, so `limit`/`offset` as field names are plausible.

**Failure scenario:** No silent corruption — always a loud compile error — but a user filtering a field named `limit` gets an error pointing at the wrong thing, and the filter!/q! grammar asymmetry is undocumented.

**Suggested fix:** Minimum: document the reserved-word set in the `q!` grammar section (and note the `filter!` escape hatch). Better: only treat a clause keyword as a terminator when it is not immediately preceded by `.` (track the previous token tree) and when the keyword begins a syntactically plausible clause — or switch to a fork-based "keyword + rest parses as clause" probe.

### 5. Two unaliased `count(*)` items silently produce duplicate `"count"` output keys

**File:line:** `src/query_parse.rs:958-966` (implicit `"count"` alias), grammar `src/lib.rs:136`; underlying constructor `shamir-query-builder/src/select/select_item.rs:83-87` accepts any alias without uniqueness validation.

**Severity:** low

**Issue:** `count(*)` is the only select item with an optional alias, defaulting to `"count"`. `q!(from users select count(*), count(*))` parses and lowers to two `SelectItem::CountAll { alias: Some("count") }` entries — an ambiguous projection whose result-map keys collide at execution time. Every other aggregate form requires an explicit `as alias`; the default exists only for the single-`count(*)` idiom, but nothing enforces that.

**Failure scenario:** User writes two `count(*)` items (e.g. under different `where` contexts they expect the engine to distinguish); the wire op carries two identical output keys and the result silently collapses to one.

**Suggested fix:** In `parse_select_item`/`QueryMacro`, error (or auto-number `count`, `count_2`, ...) when a second alias-less `count(*)` appears in one projection; a duplicate-alias check across all select items would close the whole class.

### 6. `group_by` / `order_by` accept only bare idents — no dotted paths, no string-literal field names

**File:line:** `src/query_parse.rs:219-230` (group_by: `input.parse::<Ident>()`), `:261-282` (order_by), `:441-452` (select fields, which *do* support `a.b`).

**Severity:** low

**Issue:** `select` and where-clause LHS support dotted field paths (`address.city`), and tables accept string literals (`from "user-events"`), but `group_by a.b` and `order_by address.city desc` are parse errors (group_by stops at `.` then trips the trailing-tokens check; order_by fails with "expected `asc` or `desc`"), and no field position accepts a string literal — so a field named with a hyphen or other non-ident character cannot be projected/grouped/ordered at all.

**Failure scenario:** Loud compile errors, so no wire risk — but the surface is asymmetric for a nested-document database where grouping/ordering on nested fields and non-ident field names are legitimate needs, and users will reasonably expect parity with `select`.

**Suggested fix:** Reuse `parse_dotted_ident_from` for `group_by` and per-item field parsing in `order_by` (lowering to `["a","b"]` paths the builders already accept via `IntoFieldPath`), and consider accepting string literals for field names in all field positions, mirroring `<table>`.

### 7. Doc nit: "All five forms" — there are six

**File:line:** `src/lib.rs:59-60`.

**Severity:** nit

**Issue:** "`q!` ... All five forms return the corresponding builder DTO" — the grammar documents six statement types (from/insert/update/delete/upsert/call), and the AST has six variants.

**Suggested fix:** "All six forms".

### 8. Nit: emitted `::shamir_query_builder` absolute paths break if a downstream renames the dependency

**File:line:** `src/query_parse.rs:64, 70-72, 725, 732, 808, 843-894, 940-997` (all emissions); `src/filter_lower.rs` throughout.

**Severity:** nit

**Issue:** Proc-macro expansions reference the builder by its canonical crate name, so a downstream `shamir-query-builder = { package = "...", rename }` in `Cargo.toml` breaks every expansion. This is the standard proc-macro limitation, the crate is `publish = false` and workspace-internal, and the re-export at `shamir-query-builder/src/lib.rs:79` mitigates discovery — but since `CallMacro` (finding 2) must touch this decision anyway, it is the natural moment to standardize on re-exported-path emission for all lowerings.

**Suggested fix:** No action needed now; if touching finding 2, emit all DTO paths through `::shamir_query_builder` re-exports uniformly.

---

*Verification notes: emitted constructor names/signatures were cross-checked against `shamir-query-builder/src/{filter/leaf.rs, filter/combinators.rs, query/query.rs, query/conds.rs, select/select_item.rs, write/{insert,update,delete,upsert,doc}.rs}` and `shamir-query-types/src/call/mod.rs`; the `paren.span.join()` calls in `parse_filter_expr` were confirmed valid (syn 2's delimiter tokens carry `proc_macro2::extra::DelimSpan`, whose `join()` is the zero-arg form).*
