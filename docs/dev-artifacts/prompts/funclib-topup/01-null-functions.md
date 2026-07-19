# Funclib top-up 4a — null-handling functions (coalesce/if_null/nullif/is_null)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

This is the first P0 item of "Этап 4 — v0.10 funclib top-up"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from release-readiness report 10, Part A §3
(`docs/dev-artifacts/research/2026-07-17-release-audit/10-release-readiness-v0.10.md`,
~lines 35-36, 120, 153, 290):

> Null-handling functions do not exist at all — no `coalesce`, `if_null`,
> `nullif`. This is the single most-used transformation family in any
> filter/projection language; `fn_call.rs`'s own doc-comment already
> advertises `COALESCE` as if it existed (grep `COALESCE` across the repo —
> it only appears in test fixtures / parser comments illustrating the
> `$fn` wire shape, never as a registered function). `$cond` can emulate
> `coalesce(a,b)` only clumsily, and needs `is_null` as a condition anyway,
> which also doesn't exist as a scalar.

Required functions, per the report's own naming (report 10, ~line 153):

- `coalesce(v…)` — n-ary, returns the first non-null argument (or `Null` if
  all are null / zero args... decide the zero-arg case: likely
  `missing_arg` error, mirror how `math.rs`'s `min`/`max` handle an empty
  arg list via `ok_or_else(|| ScalarError::new("missing_arg"))`).
- `if_null(v, default)` — exactly 2 args: `v` if non-null, else `default`.
- `nullif(a, b)` — exactly 2 args: `Null` if `a` and `b` compare equal
  (use `crate::compare::compare(a, b) == Ordering::Equal` for cross-type
  equality, matching how `math.rs`'s `clamp`/`between` already use
  `compare` for cross-type comparisons — do NOT use `PartialEq` directly,
  which would miss e.g. `Int(5)` vs `Dec(5.0)`), else `a` unchanged.
- `is_null(v) -> Bool` — exactly 1 arg: `true` iff `v` is `QueryValue::Null`.

## Investigation already done (context for you, verify it yourself too)

- **Module structure**: `crates/shamir-funclib/src/math.rs` is the
  project's own designated "reference implementation every other category
  module copies" (its own doc comment says so verbatim) — copy ITS shape
  exactly: a `//!` module doc comment listing registered functions and any
  conventions, a `pub fn register(reg: &mut ScalarRegistry)` that calls
  `reg.register("name", FnEntry::pure(|a| ..., min_args, max_args))` per
  function, `#[cfg(test)] mod tests;` at the bottom, and (per this
  codebase's test-organisation convention) a sibling `tests/` directory
  (`crates/shamir-funclib/src/null/tests/`) with a `mod.rs` manifest —
  check how `math/tests/` is laid out and mirror it exactly (this repo's
  CLAUDE.md test-organisation rules apply: one `tests/` dir, split by
  topic if there's more than one logical group, `tests/mod.rs` is
  re-exports only).
- **Registration wiring**: `crates/shamir-funclib/src/lib.rs`'s
  `register_builtins()` (~lines 47-62) calls `reg.in_folder("math",
  math::register)` for each category — this is what turns a plain-named
  `reg.register("abs", ...)` inside `math::register` into the wire-visible
  `math/abs`. Add `pub mod null;` to the module declarations (~lines 23-40)
  and `reg.in_folder("null", null::register);` to `register_builtins()`,
  following the report's suggested folder name (`null/` — the report also
  mentions `value_nav/` as an alternative, but `null/` is clearer and this
  brief picks it; do not use `value_nav/`, that folder already holds a
  different category of functions).
- **N-ary args**: `math.rs`'s `min`/`max` (`reg.register("min",
  FnEntry::pure(|a| reduce(a, Reduce::Min), 1, None))`) already establish
  the pattern for `coalesce` — `1` min args, `None` for unbounded max args.
- **`is_null`-shaped helper already exists elsewhere**: `crates/shamir-funclib/src/agg.rs`
  has a private `fn is_null(v: &QueryValue) -> bool` (~line 152) used by
  several aggregators to skip null inputs. Do NOT import or reuse THAT
  private function directly (it's `agg.rs`-private) — write your own
  equivalent one-liner (`matches!(v, QueryValue::Null)`) in the new `null`
  module; this is trivial enough that duplicating a 1-line match arm is
  correct, not a violation of DRY.

## The task

Create `crates/shamir-funclib/src/null.rs` (register function + module doc
comment, mirroring `math.rs`'s shape) implementing the four functions
above, plus `crates/shamir-funclib/src/null/tests/` (test directory, one
file per function or grouped logically — your call, following this
codebase's test-organisation conventions) with tests for each. Wire it into
`lib.rs` per the investigation notes above.

## Tests

1. `coalesce`: first-non-null wins among 3+ args (e.g.
   `coalesce(Null, Null, Int(5), Int(9))` → `Int(5)`); all-null →
   `QueryValue::Null` (NOT an error — a coalesce over all-nulls is a valid,
   common case, unlike `min`/`max`'s empty-arg-list error); single arg
   passthrough.
2. `if_null`: non-null `v` returns `v` unchanged; null `v` returns
   `default`; `default` itself may be `Null` (returns `Null`, no special
   casing).
3. `nullif`: `nullif(Int(5), Int(5))` → `Null`; `nullif(Int(5), Dec(5.0))`
   → `Null` (cross-type equality via `compare`, matching the report's
   emphasis on this codebase's Int/Dec/F64/Big cross-type equality
   convention already established throughout this campaign); `nullif(Int(5),
   Int(6))` → `Int(5)` (returns `a` unchanged when NOT equal).
4. `is_null`: `Null` → `true`; every other variant (`Int`, `Str`, `Bool`,
   `List`, `Map`, `Set`, `Dec`, `Big`, `F64`, `Bin`) → `false` — enumerate
   at least a representative few, not just one.
5. Regression: `register_builtins()` still builds without panicking/
   duplicate-registration errors, and a smoke-test resolving `null/coalesce`
   (or however the folder-prefixed wire name renders — check
   `ScalarRegistry::in_folder`'s exact prefixing convention, e.g. is it
   `"null/coalesce"` or `"null::coalesce"`?  Match whatever `math/abs`
   actually resolves to) through the top-level registry succeeds.

## Out of scope

- Do NOT implement any OTHER Этап 4 P0 item (percentile/string_agg wire
  params, datetime format/parse, uuid_v4, arrays/sort fix, parse_json/
  to_json) — those are separate, later leaf tasks in this same campaign.
- Do NOT touch `$cond`'s existing null-handling in the query-layer filter
  evaluator — this brief adds funclib SCALAR functions only, not new
  filter-AST operators.
- Do NOT touch `agg.rs`'s private `is_null` helper.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-funclib --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-funclib`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Confirm the exact wire-visible names your fix produces for all four
  functions (e.g. `null/coalesce` or whatever `in_folder`'s actual
  convention turns out to be) — state this explicitly, don't assume.
