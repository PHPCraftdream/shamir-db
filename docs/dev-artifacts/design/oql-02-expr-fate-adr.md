# ADR: fate of `FilterValue::Expr` ($expr) — OQL Epic 02 / Phase A (#635)

## Context

`FilterValue::Expr { expr: FilterExpr }` (`crates/shamir-query-types/src/filter/filter_expr.rs`)
wires a small fixed operator set (`FilterExprOp`) — `Add/Sub/Mul/Div/Mod/Neg`
(arithmetic), `Concat/Lower/Upper/Trim/Length` (string), `And/Or/Not` (logic),
`Eq/Ne/Gt/Gte/Lt/Lte` (comparison) — over a `Vec<FilterValue>` of operands.
It sits alongside `FilterValue::FnCall { call: FnCall }`, which invokes a
named scalar function registered in `shamir-funclib`'s `ScalarRegistry`
(`ctx.scalars.call(name, args)`), and the question this task poses is
whether `$expr` is a redundant second evaluation system for the same job.

## Analysis

`FnCall` and `Expr` are not the same mechanism wearing two names:

- `FnCall` dispatches through funclib's registry (`math.rs`, `strings.rs`,
  `crypto.rs`, `agg.rs`, ...) — a large, extensible, string-keyed catalogue
  (`abs`, `ceil`, `round`, `pow`, `sqrt`, `clamp`, `between`, hashing,
  encoding, datetime, aggregation, ...). It is the "library function call"
  surface.
- `FilterExprOp` is a small, closed, statically-typed operator enum meant
  to read like inline expression syntax (`{"$expr": {"op": "add", "args":
  [...]}}`) rather than a named-function call. Checking funclib's registry
  (`grep` across `math.rs`/`strings.rs`) shows only **partial** overlap:
  `neg`, `mod`, `lower`, `upper`, `trim`, `length`, `concat` exist as
  funclib scalar fns, but **`add`, `sub`, `mul`, `div`, `and`, `or`, `not`,
  `eq`, `ne`, `gt`, `gte`, `lt`, `lte` do not exist in funclib at all** —
  there is no scalar-fn equivalent for basic arithmetic or boolean/
  comparison logic today. Building `$expr` support purely by aliasing to
  `FnCall` would therefore require *first* extending funclib with a dozen
  new registrations, which is out of scope and duplicates work the
  `FilterExprOp` enum already encodes at the type level (closed set, no
  string-name registry lookup, no `ScalarError` boxing).

Given this, `$expr` is **not** a parallel duplicate of `$fn` — it is the
lower-level arithmetic/logic/comparison primitive layer that funclib's
scalar registry does not (yet) provide, and it can be evaluated directly
against `QueryValue` without touching the registry at all.

## Decision

**Implement `$expr` evaluation**, following the same recursive pattern as
`$cond`: each arg in `FilterExpr.args` is resolved via
`resolve_filter_query` first (so args may themselves be `$ref`/`$fn`/
`$cond`/nested `$expr`), then the operator is applied directly over the
resolved `QueryValue`s in `eval_filter_expr` (`crates/shamir-engine/src/
query/filter/resolve.rs`). Arithmetic preserves `Int` when both operands
are `Int` and the exact op doesn't overflow (via `checked_*`), otherwise
promotes to `F64`; comparison ops reuse the existing `compare_values`
helper already used by `FilterNode::matches`; string ops operate on UTF-8
`&str`. Any type mismatch, wrong arity, or unresolvable arg collapses to
`None` (absent) — the same error-handling convention `FnCall` already
uses, so filter comparisons against an ill-typed `$expr` behave like any
other missing value (no panics, no silent wrong-type coercion).

We keep `Expr` in `FilterValue` rather than removing it: it is genuinely
serving operators `FnCall`/funclib do not cover, removing it would strand
that functionality (no equivalent existed to fall back to), and — per the
brief's framing — deleting live, non-duplicate wire surface is not
warranted just because both variants happen to "compute a scalar".
