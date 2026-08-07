# Brief — #1024: implement `SelectItem::Expression` by delegating to the existing `FilterValue`/`FilterExpr` evaluator

## Context

S.H.A.M.I.R. Database. Source: both 2026-08-05 review reports (fh §9 п.1;
codex §OQL п.1-2). `SelectItem::Expression` (`crates/shamir-query-types/
src/read/select.rs`) is accepted by the wire DTO/parser/TS type but
REJECTED at execution time with `DbError::Validation
("select_expression_not_supported")`
(`crates/shamir-engine/src/query/read/select_projection.rs:118-129`,
also enforced in `aggregate.rs`'s `validate_aggregate_select`). Both
reports independently concluded the same thing: implementing a computed-
expression evaluator is the smallest, most natural next OQL step (ahead
of JOIN/window functions), and should happen now that this session's P0
DDL-lifecycle blockers are closed.

## Already investigated — the finding that shapes this brief's recommendation

I read the actual types before writing this brief, and found something
neither review report's framing anticipated: **`SelectExpr` (the AST
`SelectItem::Expression` carries) is a strict, narrower duplicate of an
AST this codebase ALREADY implements and evaluates in production.**

- `SelectExpr` (`crates/shamir-query-types/src/read/select_expr.rs`) has
  exactly 6 variants: `Add`/`Sub`/`Mul`/`Div` (arithmetic only),
  `Field { path }`, `Literal { value }`.
- `FilterExpr`/`FilterExprOp` (`crates/shamir-query-types/src/filter/
  filter_expr.rs`) — used by the `$expr` filter operator, already fully
  implemented and evaluated everywhere filters run — has `Add`/`Sub`/
  `Mul`/`Div`/`Mod`/`Neg` (math), `Concat`/`Lower`/`Upper`/`Trim`/
  `Length` (string), `And`/`Or`/`Not` (logic), `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/
  `Lte` (comparison) — a strict superset of `SelectExpr`'s 4 math ops.
- `FilterValue::Expr { expr: FilterExpr }` (`crates/shamir-query-types/
  src/filter/filter_value.rs:66-69`) is how a `FilterExpr` tree is
  embedded anywhere a `FilterValue` is expected — and `FilterValue` is
  EXACTLY what `SelectItem::Function`'s `args` already use, evaluated via
  `resolve_filter_query` inside `SelectProjection::project_value`
  (`select_projection.rs:192-201`, the `funcs` loop) — the SAME single
  production choke point every read plan already funnels through.

**Recommendation (this brief's proposed answer to the (a)/(b) decision,
present it to the orchestrator in your final report — implement it if you
agree after your own verification, push back with a counter-argument if
you don't):** implement `SelectItem::Expression` NOT by writing a new
evaluator, but by **translating `SelectExpr` into an equivalent
`FilterValue::Expr(FilterExpr{...})` tree at `SelectProjection::new` time,
and feeding the translated value into the SAME `funcs` vec
`SelectItem::Function` already populates.** This reuses 100% of the
existing, already-tested arithmetic/type-coercion/field-resolution
evaluation logic (`resolve_filter_query`, the cond/field-path/query-ref
prescan caches) — near-zero new evaluation code, just an AST-to-AST
translation layer. This is a materially lower-risk, lower-effort
implementation than either report likely assumed when it described the
feature as needing its own "arithmetic, comparison, boolean, coalesce,
scalar functions" evaluator built from scratch — that evaluator already
exists (`FilterExpr`), `SelectExpr` just never got wired to it.

## What to implement

1. **A translation function** `select_expr_to_filter_value(expr:
   &SelectExpr) -> FilterValue` (or equivalent) in
   `shamir-query-types` or `shamir-engine` (your call on the right crate
   — `select_expr.rs` lives in `shamir-query-types`, but if the
   translation needs anything only `shamir-engine` has, put it there;
   note the two crates' existing dependency direction before choosing):
   - `SelectExpr::Add{left,right}` → `FilterValue::Expr{expr:
     FilterExpr{op: FilterExprOp::Add, args: vec![translate(left),
     translate(right)]}}` (mirror for Sub/Mul/Div).
   - `SelectExpr::Field{path}` → `FilterValue::FieldRef{path}` (confirm
     `SelectExpr::Field.path`'s type (`crate::filter::FieldPath`) matches
     `FilterValue::FieldRef.path`'s type exactly — both should already
     import the same `FieldPath` type; if there's any mismatch, resolve
     it precisely, don't paper over with a lossy conversion).
   - `SelectExpr::Literal{value: SelectExprValue}` → the matching
     `FilterValue` literal variant (`SelectExprValue::Null/Bool/Int/
     Float/String` → `FilterValue::Null/Bool/Int/Float/String` — check
     these map 1:1 with no representation gaps, e.g. does `FilterValue`
     have a `Float` variant with the exact same semantics).
2. **Wire it into `SelectProjection::new`** (`select_projection.rs:
   118-129`): replace the `SelectItem::Expression { .. } =>
   return Err(...)` arm with: translate the `expr` field via the new
   function, push `(key, translated_filter_value)` into the SAME `funcs`
   vec `SelectItem::Function`'s arm already builds (`key` derived from
   `alias.clone().unwrap_or_else(|| /* what fallback? SelectItem::
   Expression has no natural "name" like Function's `name` field —
   investigate what a sensible default key should be when no alias is
   given; check what the TS type / existing tests / either review report
   expect, or pick a reasonable convention and document why */)`.
3. **Confirm the prescan caches already cover the translated shape** —
   `prescan_cond_cache`/`prescan_field_path_cache`/
   `prescan_query_ref_cache` run over every `funcs` entry already
   (`select_projection.rs:148-152`); since the translated value is a
   plain `FilterValue`/`FilterExpr` tree with no `$cond`/`$query` nodes,
   this should be a no-op pass-through, but verify by reading those
   prescan functions rather than assuming.
4. **Remove the now-dead validation-error path** in
   `crates/shamir-engine/src/query/read/aggregate.rs`'s
   `validate_aggregate_select` if it specifically rejects
   `SelectItem::Expression` the same way (find it, confirm, either update
   it to allow the now-implemented variant or leave a short comment
   explaining why it's still restricted there specifically if there's a
   genuine reason — e.g. expressions inside an aggregate context might
   need different handling; investigate before assuming either way).
5. **Both SDKs** — check `crates/shamir-client/src/builder/` (Rust) and
   `crates/shamir-client-ts/src/core/builders/` (TS) for whether a typed
   builder helper for `SelectItem::Expression`/`SelectExpr` already
   exists (the wire type/parser/TS type already accept it per the task
   description) or whether callers can currently only construct it via
   the raw DTO. If a builder helper is missing and the raw DTO is the
   only way to reach this now-working feature, add a minimal typed
   constructor mirroring how other `SelectItem` variants are exposed
   (don't over-build — match existing conventions in those files).

## Tests

- Unit tests proving `SelectExpr` → `FilterValue` translation is correct
  for each of the 6 variants (including nested — `Add{Mul{...}, Field}`
  etc.).
- Integration test(s) proving a real query with a `SelectItem::Expression`
  in its `SELECT` list now returns the computed value instead of
  erroring — cover at least one arithmetic case over real field data, one
  literal-only case, and confirm the alias/key naming behaves as decided
  in step 2.
- A test proving the OLD rejection error path is genuinely gone (the
  `select_expression_not_supported` error no longer fires for a
  previously-rejected shape) — don't just add new passing tests, prove
  the old failure mode is closed.
- If `validate_aggregate_select` still rejects this variant in an
  aggregate context (step 4), a test pinning that specific remaining
  restriction with its own accurate error message/reasoning.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- This is a real, if narrow, wire-contract behavior change (a
  previously-always-erroring input now succeeds) — not a breaking change
  in the SemVer sense (nothing that worked before stops working), but
  note it as a real capability addition in your final report.
- Gate: `cargo fmt -p shamir-query-types -p shamir-engine -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-query-types -p shamir-engine -p
  shamir-client --full`. Use the wrapper, never raw `cargo test`/`cargo
  nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] Verified (or refuted, with a clear counter-argument) the
      "SelectExpr is a redundant subset of FilterExpr" finding above
      before implementing.
- [ ] `SelectExpr` → `FilterValue` translation implemented and unit
      tested for all 6 variants.
- [ ] `SelectProjection::new` no longer rejects `SelectItem::Expression`
      — it evaluates it via the existing `funcs`/`resolve_filter_query`
      pipeline.
- [ ] Integration test(s) proving a real query with a computed SELECT
      expression works end-to-end, and that the old rejection error is
      gone.
- [ ] `validate_aggregate_select` investigated and either updated or
      left with a documented reason.
- [ ] SDK builder gap (if any) investigated, closed if missing and
      cheap to add.
- [ ] fmt/clippy/test gates green, real output reported.
