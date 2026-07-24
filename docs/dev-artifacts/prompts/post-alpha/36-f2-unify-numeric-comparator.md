# Brief for #791 (F-2) — unify the Int↔F64 numeric comparator across all fast paths

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

The Wave-E fix W-1 (commit `10fd6d27`) fixed a HIGH-severity regression in
`cmp_i64_f64`: comparing `Int(i64)` against `F64(f64)` must use an exact
bounds-check + `floor`/`fract` technique, NOT the lossy `f.fract() > 0.0` or
a naive `i as f64` / `f as i64` cast, because:

- casting a large `i64` to `f64` can round (loses precision above `2^53`);
- `f64::fract()` is truncation-based (sign-preserving): for negative `f`,
  `fract()` is negative or zero, NEVER positive — so `f.fract() > 0.0` is
  not a valid "does f have a nonzero fractional part" test.

The fix landed in exactly TWO places (duplicated, not shared):

- `crates/shamir-engine/src/query/filter/resolve.rs` — `cmp_i64_f64` (around
  line 127), used by `compare_values` (the general evaluator) and `IN`/`Eq`
  atom evaluation.
- `crates/shamir-engine/src/query/read/order.rs` — a byte-for-byte second
  copy of `cmp_i64_f64` (around line 210), used by `QvSortKey`'s ORDER BY
  comparator.

An independent review (`docs/dev-artifacts/research/` — the 2026-07-24 Wave
E review, its own follow-up pass) found that THREE more numeric comparison
sites in the same crate still use the OLD lossy technique and were never
updated by W-1:

1. **`crates/shamir-engine/src/query/filter/eval_bytes.rs`, function
   `compare_raw_to_filter` (around lines 465-474)** — the bytes-level
   pre-filter (`FilterNode::matches_msgpack_bytes`). Read the module doc
   comment at the top of this file carefully (lines 1-38): this pre-filter
   has a STRICT invariant — `Some(false)` short-circuits the row as
   rejected WITHOUT ever running the full accurate decode+filter path.
   `Some(true)` and `None` both fall through to the safe full-decode path.
   This means a WRONG `Some(false)` here (or a wrong `Some(true)` that
   later flips wrong under `Ne`) from the lossy cross-type arms is a real
   false-negative/false-positive bug, not just a cosmetic imprecision —
   the module's own doc comment claims "never produces a false-negative or
   a false-positive" and the lossy arms currently violate that for large
   `i64`/`u64` values or specific float/int boundary cases. The arms in
   question:
   ```rust
   (RawScalar::I64(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
   (RawScalar::U64(a), FilterValue::Float(b)) => (*a as f64).partial_cmp(b),
   (RawScalar::F64(a), FilterValue::Int(b)) => a.partial_cmp(&(*b as f64)),
   ```
   Note there is a `RawScalar::U64` arm here that resolve.rs/order.rs don't
   have to deal with (their inputs are always signed `i64` `QueryValue::Int`).
   `u64` needs its own exact comparator against `f64` — same technique
   (bounds-check + floor/fract), but bounded to `[0, 2^64)` instead of
   `[-2^63, 2^63)`. Derive it the same way `cmp_i64_f64`'s doc comment
   derives the i64 bounds (both `0.0` and `2^64` as `f64` literals are
   exact).

2. **`crates/shamir-engine/src/query/filter/filter_node.rs`, function
   `set_contains_coercing` (around lines 48-76)** — used by `IN`/`NOT IN`
   fast-path set membership. The `Int(n)` arm does `n as f64` (lossy cast,
   same class of bug as W-1's root cause: for `|n| >= 2^53` this can round
   to a DIFFERENT f64 than what's actually stored in the `TSet`, causing a
   false miss or false hit). The `F64(f)` arm's bounds clamp
   (`f >= i64::MIN as f64 && f <= i64::MAX as f64`) has the EXACT same
   off-by-one risk W-1 fixed: `i64::MAX as f64` rounds UP to `2^63` (since
   `i64::MAX == 2^63 - 1` is not exactly representable as `f64`), so
   `f == 2^63.0` incorrectly passes the `<=` check and `f as i64` then
   saturates/is UB-adjacent instead of correctly reporting "no exact i64
   equivalent, no match". Replace with the exact bounds-checked technique.

3. **`crates/shamir-engine/src/query/read/aggregate.rs`,
   `OwnedScalar::cmp_scalar` (around lines 93-114)** — MIN/MAX
   accumulator comparison. Same lossy arms:
   ```rust
   (OwnedScalar::Int(b), ScalarRef::F64(a)) => (*b as f64).partial_cmp(&a),
   (OwnedScalar::F64(b), ScalarRef::Int(a)) => b.partial_cmp(&(a as f64)),
   ```
   Same fix.

There is also a cursor keyset boundary comparator worth checking (do NOT
change its call sites' control flow, only its numeric comparison if it has
the same lossy pattern): `crates/shamir-server/src/db_handler/cursor_handlers.rs`
— search for wherever it compares an `Int`/`F64` bookmark value against a
row's sort field (likely reuses `compare_values` from `shamir-engine`
already, in which case it's ALREADY fixed transitively once
`compare_values` uses the shared comparator — verify this instead of
duplicating work).

## What "done" looks like

1. **Consolidate into ONE shared location** inside `shamir-engine` (both
   current copies plus the three broken sites are all within this single
   crate, so no cross-crate extraction is needed). Suggested location:
   new file `crates/shamir-engine/src/query/filter/numeric_cmp.rs`
   (one-file-one-export convention: this file owns the numeric-comparison
   helpers as a tight cohesive group — `cmp_i64_f64` + `cmp_u64_f64`).
   Wire it in via `crates/shamir-engine/src/query/filter/mod.rs` as
   `pub(crate) mod numeric_cmp;` (visibility must reach both
   `query::filter::*` and `query::read::*` siblings — check what
   visibility level is actually required by compiling; `pub(crate)` should
   suffice since both are inside the same crate).
2. Delete the two duplicated private `cmp_i64_f64` copies in `resolve.rs`
   and `order.rs`; both now call the shared `numeric_cmp::cmp_i64_f64`.
   Preserve the existing detailed doc comment (the bounds-check
   derivation) — move it to the new shared location, don't duplicate it
   twice more.
3. Add `cmp_u64_f64(u: u64, f: f64) -> Option<Ordering>` to the same
   module, analogous derivation for the `[0, 2^64)` range.
4. Fix `eval_bytes.rs::compare_raw_to_filter`'s three cross-type arms to
   call `cmp_i64_f64`/`cmp_u64_f64` instead of the lossy cast+partial_cmp.
5. Fix `filter_node.rs::set_contains_coercing`'s `Int(n)` and `F64(f)` arms
   to use the exact comparator (either call `cmp_i64_f64` directly for the
   ordering check refactored into an equality test, or — simpler — replace
   the lossy bounds clamp with the same exact `2^63` derivation used in
   `cmp_i64_f64`, since this function only needs equality not full
   ordering; use whichever is a smaller, clearer diff. If reusing
   `cmp_i64_f64` fully: equal iff `cmp_i64_f64(n, f) == Some(Ordering::Equal)`).
6. Fix `aggregate.rs::OwnedScalar::cmp_scalar`'s two cross-type arms to
   call the shared comparator.
7. **Cross-path regression tests** — the review's core complaint is that
   "the answer can depend on which fast path got picked." Add tests
   (in the existing `crates/shamir-engine/src/query/filter/tests/eval_tests/dec_cross_type_tests.rs`
   or a new sibling test file if that one is scoped too narrowly to
   Dec/Big — check its current scope first) that take the SAME set of
   Int/F64 edge-case pairs (negative fractional, `2^53`-boundary,
   `2^63`-boundary, `2^64`-boundary for the u64 case) and assert that:
   - the general evaluator (`compare_values` / `Filter::Eq` etc.),
   - the bytes pre-filter (`FilterNode::matches_msgpack_bytes` — construct
     a raw msgpack record and call it directly, or find the existing
     helper that does this in this test tree),
   - `IN`/`NOT IN` set membership (`set_contains_coercing` — may need a
     small local test in `filter_node.rs`'s own test file if one exists,
     or via the public `Filter::In` path),
   - MIN/MAX aggregation (`aggregate.rs`'s accumulator via a small
     `GroupBy`/aggregate query),
   - ORDER BY (`QvSortKey` comparator),

   all agree on the same answer for each edge case. Follow the repo's test
   organisation rule: one `tests/` dir per module, `tests/mod.rs` is a
   manifest only, no inline `#[cfg(test)] mod tests { ... }` blocks.

## Constraints

- Do NOT touch `shamir-server`'s cursor keyset code unless your
  investigation in step "cursor keyset boundary comparator" above finds it
  has its OWN duplicated lossy arm (unlikely — verify first, don't assume).
- Do NOT change any behavior for non-numeric types (Str/Bin/Bool/Null) —
  scope is strictly the Int↔F64 (and new Int/U64↔F64) cross-type arms.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test` — it is blocked by the perimeter guard.
- `cargo fmt -p shamir-engine` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean for the crate you touch (don't fix pre-existing unrelated lints).
- Follow the workspace conventions in `CLAUDE.md`: `use` statements at file
  top, `mod.rs` files are re-exports only, one primary export per file,
  surgical diff — no incidental refactors.

## Verification the orchestrator will run (you don't need to, but your
diff must survive it)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```
