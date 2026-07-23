# Brief: CR-C5 — exact `Big` comparisons: eliminate f64 rounding (#780)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — verified against the current tree 2026-07-23 (investigation already done, findings below are exact)

FG-1/FG-6 made `u64` storage lossless and `Big` (a `num_bigint::BigInt`,
promoted from a `u64 > i64::MAX`) filterable/sortable — but several
comparison/aggregate code paths still convert `Big` to `f64` before
comparing, and two distinct large integers (e.g. `i64::MAX` and
`i64::MAX + 1`, or generally anything `>= 2^53`) can round to the SAME
`f64` — silently reintroducing the exact precision-loss bug FG-1/FG-6 were
built to close, just moved one layer up (storage is now honest, some
COMPUTE is not).

**Confirmed LOSSY sites** (all convert one/both operands to `f64` via a
`to_f64()`/`as f64` cast before comparing):

1. `crates/shamir-engine/src/query/filter/resolve.rs::compare_values`
   (~lines 128-134): `(Big, Big)`, `(Int, Big)`/`(Big, Int)`,
   `(F64, Big)`/`(Big, F64)`, `(Dec, Big)`/`(Big, Dec)` — all route through
   a `lossy_f64(&BigInt) -> f64` helper (~lines 84-86,
   `b.to_f64().unwrap_or(f64::NAN)`).
2. `crates/shamir-engine/src/query/read/order.rs::QvSortKey`'s cross-type
   `Ord`/comparison impl (~lines 309-332): `(I64, Big)`/`(Big, I64)`,
   `(F64, Big)`/`(Big, F64)`, `(Dec, Big)`/`(Big, Dec)` — same `big_to_f64`
   pattern (~lines 181-183). Note: `(Big, Big)` itself (~line 312) is
   ALREADY exact (`x.cmp(y)`, direct `BigInt::cmp`) — do not touch that arm.
3. `crates/shamir-engine/src/query/read/aggregate.rs`: the `Sum`/`Avg`
   accumulator steps' `Dec`/`Big` fallback (~lines 426-430 for Sum,
   ~457-462 for Avg) both route through `agg_leaf_to_f64` (~lines 616-621,
   `InnerValue::Big(b) => Some(b.to_f64().unwrap_or(f64::NAN))`). Min/Max's
   `OwnedExtreme::Tree` variant (~lines 506, 546) routes through
   `compare_values` — fixing item 1 above automatically fixes this too, no
   separate change needed here.

**Already EXACT — do NOT touch, these are correct precedent to follow:**

- `compare_values`'s `(Big, Str)`/`(Str, Big)` arms (~lines 143-144):
  parse the string operand as an exact `BigInt` (`b.parse::<num_bigint::BigInt>()`)
  and compare directly — this IS the pattern to mirror for the other
  lossy arms.
- `canonical_eq` (`crates/shamir-engine/src/query/read/hashable_query_value.rs`,
  ~lines 160-187): `Big`↔`Big`, `Big`↔`Str`, `Dec`↔`Big` EQUALITY already
  goes through exact string-canonicalization (`x.to_string() ==
  y.to_string()`), not `f64`. This file needs NO changes — it's already
  correct; the task is scoped to ORDERING/comparison and aggregation, not
  equality/dedup.
- `compare_values`'s plain `(Int, F64)`/`(F64, Int)` arms (~lines 115-116):
  `Int` is `i64`, bounded well under `2^53` in magnitude is NOT actually
  guaranteed (`i64::MAX` IS `> 2^53`) — wait, re-verify this yourself:
  is `(Int, F64)` (NOT involving `Big`) actually safe, or does it have the
  SAME bug for a large `i64`? Check whether this arm is in scope too, or
  whether `Int` values in this codebase are constrained to a range where
  `as f64` is provably lossless (e.g. by a validation/promotion rule that
  promotes anything `> 2^53`-risky to `Big` before it ever reaches this
  arm). Do not assume the investigation summary above is exhaustive — this
  ONE claim ("Int↔F64 cross-type is EXACT because Int is i64, bounded
  under 2^53") deserves your own re-verification since it looks
  questionable on its face (`i64::MAX` is `~9.2 * 10^18`, far above
  `2^53 ≈ 9 * 10^15`). If you find `Int↔F64` (no `Big` involved) is ALSO
  lossy for large `i64` values, that's a genuine additional finding beyond
  this brief's original scope — fix it using the same exact-comparison
  technique below if it's a small, analogous addition; if fixing it opens
  a much larger can of worms (e.g. it turns out EVERY plain-`Int` column
  needs a rethink), stop, document what you found precisely in your final
  report, and do NOT attempt a wholesale fix beyond this task's Big-focused
  scope — flag it as a separate follow-up instead.

## Fix — exact comparison techniques, applied surgically

### `Big` vs `Int` — trivial, MUST fix

Convert the `Int` (`i64`) operand to an exact `BigInt` (`num_bigint::BigInt::from(i)`)
and compare via `BigInt::cmp` directly — an `i64` always converts to
`BigInt` losslessly (this is NOT the same as the reverse `f64`
conversion). Apply this to `compare_values`'s `(Int, Big)`/`(Big, Int)`
arms and `QvSortKey`'s `(I64, Big)`/`(Big, I64)` arms. No approximation, no
edge case — always correct.

### `Big` vs `Dec` — exact via cross-multiplication, SHOULD fix

`rust_decimal::Decimal` is a 96-bit fixed-point type (mantissa + scale,
NOT a float) — an exact comparison against an arbitrary-precision `BigInt`
is achievable via cross-multiplication rather than converting either side
to `f64`: extract `Decimal`'s unscaled mantissa (check `Decimal`'s public
API — likely `Decimal::mantissa()`/`.scale()` or similar; verify the exact
method names in `rust_decimal` 1.40's docs/source before writing this)
and its `scale` (number of fractional digits), then compare
`big_value * 10^scale` (as `BigInt`, using `BigInt::from(10).pow(scale)` —
`BigInt` arithmetic has no magnitude limit) against
`BigInt::from(mantissa)` — this is EXACT regardless of how large either
operand is, since both sides of the comparison end up as arbitrary-
precision integers. Handle the sign correctly (a negative `Decimal` and/or
negative `BigInt`). Apply to `compare_values`'s `(Dec, Big)`/`(Big, Dec)`
arms and `QvSortKey`'s `(Dec, Big)`/`(Big, Dec)` arms.

If, after investigating `rust_decimal`'s actual API, this cross-
multiplication approach turns out genuinely awkward to implement cleanly
(e.g. no clean mantissa/scale accessor, or a sign-handling trap that makes
this riskier than it's worth for a "surgical" fix), it is ACCEPTABLE to
fall back to the SAME string-canonicalization technique `canonical_eq`
already uses for equality — but ordering via string comparison requires
correctly handling sign, integer-part zero-padding to equal lengths, and
the decimal point position, which is MORE subtle than equality's simple
`to_string() == to_string()`. Prefer the cross-multiplication approach if
at all feasible; only fall back to string-based ordering if you can make
it genuinely correct (test the boundary cases below either way), and
document clearly which approach you landed on and why.

### `Big` vs `F64` — accepted approximation, document (do NOT force an exact fix)

`F64` is itself an inherently imprecise IEEE-754 column type — there is no
single "correct" exact answer when comparing an EXACT `BigInt` against an
APPROXIMATE `f64` value beyond "which f64 is this BigInt closest to"
semantics. Comparing an exact type against an approximate type doesn't
have the same bug class as comparing two exact types (`Int`/`Dec`) via a
lossy intermediate — the approximation here is INHERENT to the `F64`
column's own nature, not introduced by sloppy comparison code. Leave
`compare_values`'s and `QvSortKey`'s `(F64, Big)`/`(Big, F64)` arms AS-IS
(still using `lossy_f64`/`big_to_f64`), but ADD a doc comment on each
explicitly stating this is a deliberate, accepted approximation (comparing
against an inherently-approximate float column), distinct from the
Int/Dec arms which get fixed to be exact. This distinction — "F64
approximation is inherent to the column type, Int/Dec approximation was a
comparison-code bug" — is the crux of this task's scope; make it explicit
in the code, not just in your head.

### Aggregates — Sum/Avg over `Big`, decide and document

`Sum`'s `Dec`/`Big` fallback (~lines 426-430,
`aggregate.rs::agg_leaf_to_f64`) currently accumulates via `f64` for ANY
row contributing a `Big`/`Dec` value. Per the task's own instruction
("Aggregates: sum over Big may genuinely need BigInt accumulation or a
documented overflow/precision policy — decide and document rather than
silently keeping f64"):

- **Preferred**: give `Sum` a genuine exact-integer accumulation lane for
  `Big`/`Int` inputs (a running `BigInt` accumulator, promoted from `i64`
  the same way a single `Big` value promotes today), falling back to the
  existing `f64` lane ONLY once a genuinely non-integer `F64`/`Dec-with-
  fractional-part` value enters the same aggregate (mixing an exact
  integer running total with a float automatically forces the whole
  aggregate into float semantics from that point, which is unavoidable —
  document this transition point clearly).
- **If that's too large a restructure for this task's surgical scope**:
  at minimum, add a clear doc comment on `agg_leaf_to_f64` and the
  `Sum`/`Avg` accumulator states stating PRECISELY what precision is lost
  and under what conditions (e.g. "a running sum of `Big` values loses
  precision once the exact total exceeds `2^53`; this is a known,
  documented limitation, not a silent bug") — do not leave the current
  state UNDOCUMENTED either way. Decide which of these two paths you take
  and justify the choice in your final report; a documented limitation is
  an acceptable outcome for THIS task if the exact-accumulation
  restructure proves too invasive, but silence is not.
- `Avg` inherently produces a non-integer result (a ratio) — it's
  reasonable for its FINAL division to stay `f64`-based regardless of
  which path you take for `Sum`; the question is only whether the
  intermediate running total (before the final division) is exact or
  already-lossy.

## Tests (TDD — write failing tests first)

In `crates/shamir-engine/src/query/filter/tests/eval_tests/dec_cross_type_tests.rs`
(or a suitable sibling test file matching this file's existing
conventions):

- **`i64::MAX` vs `i64::MAX + 1`** (`i64::MAX + 1` promoted to `Big`,
  matching this codebase's own promotion rule — check how a test already
  constructs a `Big` value, e.g. `BigInt::from(i64::MAX) + 1`) —
  `compare_values(&Value::Int(i64::MAX), &Value::Big(bigger))` and the
  `QvSortKey` equivalent must correctly report `Less`, not `Equal` (the
  CORE regression this whole task exists to fix — verify this test FAILS
  against the current `lossy_f64`-based code before your fix, then passes
  after).
- **`u64::MAX` ordering vs nearby values** — `u64::MAX` (promoted to
  `Big`) vs `u64::MAX - 1` (also `Big` or `Int` depending on this
  codebase's exact promotion boundary — check) must sort/compare
  correctly, not collapse to equal.
- **Mixed `Int`+`Big` column, total order + stability** — a multi-row
  `ORDER BY` over a column mixing plain `Int` and promoted `Big` values,
  including some that are numerically CLOSE (within f64's rounding
  distance of each other) — assert the full sort order is exactly correct
  (every row in its precisely correct position, not just "roughly
  sorted").
- **`Big` vs `Dec` boundary case** — an exact-integer-valued `Big` vs a
  `Dec` with a fractional part that's numerically close, proving the
  cross-multiplication (or your chosen fallback) approach orders them
  correctly at a precision `f64` could not distinguish.
- **Sum/Avg aggregate test** for whatever policy you land on (exact
  accumulation proof, or a test asserting the DOCUMENTED limitation's
  boundary behaves as documented — either is acceptable, matching your
  choice above).
- **Regression**: existing FG-1/FG-6 suites
  (`compare_values_big_fallback` and everything else touching `Big` in
  `dec_cross_type_tests.rs` and elsewhere) must stay green.

## Gate

```
cargo fmt -p shamir-engine -p shamir-query-types -- --check
cargo clippy -p shamir-engine -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-query-types --full
```

All must pass before returning. Primary code area: `shamir-engine`
(`query/filter/resolve.rs`, `query/read/order.rs`,
`query/read/aggregate.rs`, their tests). Do NOT touch
`hashable_query_value.rs`'s `canonical_eq` (already exact, out of scope)
or any cursor/server code (this is a pure engine compare/aggregate
correctness task, fully disjoint from the cursor-streaming work).
