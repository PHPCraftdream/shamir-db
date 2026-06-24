IMPLEMENTATION TASK (TDD). Do NOT commit, do NOT push. Tests via ./scripts/test.sh (raw cargo test blocked). Touch ONLY crates/shamir-query-builder/src/val/ (and its tests) — do NOT touch engine, batch/, ddl/, query/, or any other crate/module (other agents are working there).

Goal (ACTION-ITEMS B3): add Rust builder constructors for the `$expr` and `$cond` FilterValue variants. TS already has these (`filter.expr()`/`filter.cond()`); the Rust builder lacks them — close the parity gap.

Read first:
- crates/shamir-query-builder/src/val/ — the existing `val::*` constructors (lit, col, func, param, qref, etc.) and how they build `FilterValue`. Match this style exactly.
- crates/shamir-query-types/src/filter/ — the wire types: `FilterValue::Expr` / `FilterExpr` / `FilterExprOp` (arithmetic Add/Sub/Mul/Div/Mod/Neg, string Concat/Lower/Upper/Trim/Length, logic And/Or/Not, comparison Eq/Ne/Gt/Gte/Lt/Lte) and `FilterValue::Cond` / `Cond` (ternary if/then/else). Read the EXACT type/field/variant names — do not invent.

Add to the `val` module (one-primary-export-per-file per CLAUDE.md — likely new sibling files re-exported via the module's mod.rs):
1. `val::expr(op, args)` building `FilterValue::Expr` (mirror however the DTO is constructed — `FilterExpr::new(op, args)` or similar). Plus ergonomic wrappers for the common ops, e.g. `val::add(a, b)`, `val::sub`, `val::mul`, `val::div`, `val::concat(parts)`, `val::lower(x)`, `val::upper`, `val::trim`, `val::length` — pick the set that mirrors `FilterExprOp` cleanly; state which you added.
2. `val::cond(condition, then, or_else)` building `FilterValue::Cond` (mirror `Cond::new(...)`).

Tests (TDD, follow CLAUDE.md layout — val tests live under crates/shamir-query-builder/src/val/tests/ or the crate's test dir):
- 🔴 serde/structural round-trip: `val::add(val::col("a"), val::lit(1))` → the exact `FilterValue::Expr` shape (assert the built FilterValue equals the hand-built DTO, or its wire bytes).
- 🔴 `val::cond(<filter>, <then val>, <else val>)` → exact `FilterValue::Cond` shape.
- cover a few operators (one arithmetic, one string, one logic) + the ternary.
- 🟢 implement until green.
- Run: ./scripts/test.sh -p shamir-query-builder (read FULL output; never pipe|grep).

Keep the diff surgical, imports at top. End with a final message: the constructors you added + test pass count.

RATE-LIMIT: do this YOURSELF in a single agent — NO sub-agents. Use grep/view directly.
