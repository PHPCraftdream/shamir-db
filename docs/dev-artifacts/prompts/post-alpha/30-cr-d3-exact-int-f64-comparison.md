# Brief: CR-D3 — exact `Int`↔`F64` comparison + `KNOWN_LIMITATIONS.md` entry (#784)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — confirmed by an independent review, already flagged in code comments

`crates/shamir-engine/src/query/filter/resolve.rs::compare_values`'s
`(Value::Int(a), Value::F64(b))` / `(Value::F64(a), Value::Int(b))` arms
and `crates/shamir-engine/src/query/read/order.rs::compare_qv_sort_keys`'s
`(QvSortKey::I64(x), QvSortKey::F64(y))` / `(QvSortKey::F64(x),
QvSortKey::I64(y))` arms all cast the `i64` to `f64` before comparing
(`(*a as f64).partial_cmp(b)` / similar). Both sites already carry a CR-C5
doc comment marking this as a known, confirmed, deliberately-unfixed gap
(that task's scope was `Big`-focused, not plain `Int`) — this task is the
follow-up that CR-C5 itself asked for.

`(i64::MAX) as f64 == (i64::MAX - 1) as f64` — up to 1,024 distinct `i64`
values collapse onto a single `f64` near the top of the `i64` range (256
near `1e18`). This is REACHABLE in practice: nanosecond epoch timestamps
(~1.75e18 today), snowflake-style ids, 63-bit hashes/counters all live in
`Int` (only `u64 > i64::MAX` promotes to `Big` — plain `i64` covers the
WHOLE `i64` range, no early promotion). The bug fires on a CROSS-TYPE
`Int`↔`F64` comparison: a float literal/operand against an `Int` column
(`WHERE ns_timestamp > 1.5e18`), or an `ORDER BY` column mixing `Int` and
`F64` values. `Eq` can match distinct values; `Gt`/`Gte`/`Lt`/`Lte`
boundaries are fuzzy by up to ~1,024 at `i64::MAX` scale — wrong
inclusion/exclusion for range filters over ns-timestamps or large ids. In
`ORDER BY`, colliding values compare `Equal` and fall back to stable
insertion order (a non-total order, not data loss on its own — but
intersects with CR-D2's keyset null-detection machinery if such a mixed
column is ALSO the seek key. Note: CR-D2's fix ONLY probes for `Null`/
missing values, not this `Int`/`F64` mixing — this is a SEPARATE gap CR-D2
did not close). `Int`↔`Int` and `Int`↔`Dec` remain exact and unaffected.

## Fix — exact `i64`↔`f64` comparison, no `BigInt` needed

This is materially smaller than CR-C5's own `BigInt` cross-multiplication
work — `f64` already has enough headroom (11-bit exponent) to represent
every integer up to `2^63` in MAGNITUDE (just not every value at high
magnitude, since the 52-bit mantissa runs out of precision) which is
exactly what makes a bounds-check + `floor`/`fract` technique exact
without arbitrary-precision arithmetic. Verified-correct algorithm (derive
it yourself / verify this reasoning before implementing, don't blindly
copy without understanding why each step is exact):

```
fn cmp_i64_f64(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None; // preserve the EXISTING NaN convention this codebase
                      // already uses for F64<->F64 (partial_cmp's own NaN
                      // handling) -- do not invent new NaN semantics here.
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Ordering::Less } else { Ordering::Greater });
        // any finite i64 is < +inf, > -inf.
    }
    // f is finite from here on.
    //
    // Bound f against i64's range using EXACT powers of two (2^63 as an
    // f64 literal is exact -- it's a power of two, always exactly
    // representable). i64::MIN == -2^63 exactly; i64::MAX == 2^63 - 1, so
    // the exclusive upper bound for "f could possibly be an in-range i64
    // value" is 2^63 (f >= 2^63 means f is >= i64::MAX + 1, definitely
    // greater than any i64).
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
    const I64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 9223372036854775808.0; // 2^63, exact
    if f < I64_MIN_AS_F64 {
        return Some(Ordering::Greater); // i (>= i64::MIN) > f
    }
    if f >= I64_MAX_EXCLUSIVE_UPPER_BOUND {
        return Some(Ordering::Less); // i (<= i64::MAX) < f
    }
    // f is now known finite and within [-2^63, 2^63). Key fact: any f64
    // with |f| >= 2^53 has NO fractional bits available at all (the whole
    // 52-bit mantissa is consumed by the integer part at that exponent),
    // so f.fract() == 0.0 identically for the entire remaining magnitude
    // range this branch can reach beyond 2^53 -- f.floor() == f exactly
    // in that regime. Below 2^53, floor()/fract() behave normally. Either
    // way, f.floor() is an EXACT integer-valued f64 within
    // [-2^63, 2^63), which is therefore losslessly representable as i64
    // (a specific integer value < 2^63 always fits in i64's range,
    // regardless of f64's sparser representable-value spacing near the
    // top of that range).
    let f_floor = f.floor();
    let f_floor_i64 = f_floor as i64; // exact: see reasoning above.
    match i.cmp(&f_floor_i64) {
        Ordering::Equal => {
            // i == floor(f) exactly. If f had a nonzero fractional part,
            // f > floor(f) = i, so i < f. (For |f| >= 2^53 this branch
            // never triggers since fract() is always 0 there -- covered
            // for completeness at lower magnitudes.)
            if f.fract() > 0.0 {
                Some(Ordering::Less)
            } else {
                Some(Ordering::Equal)
            }
        }
        other => Some(other),
    }
}
```

Double-check this derivation yourself (especially the exactness claims
around `f.floor() as i64` and the `2^53` fractional-bits argument) before
committing to it — if you find a flaw, fix the algorithm, don't just paste
this. Write the unit-boundary tests below BEFORE finalizing, and use them
to validate whichever exact implementation you land on.

Apply this helper (with whatever adjustments the call-site conventions
need — check what type `Value`'s `Int` field actually is, `i64` almost
certainly, matching this brief's signature) to BOTH sites:

- `resolve.rs::compare_values`'s `(Value::Int(a), Value::F64(b))` /
  `(Value::F64(a), Value::Int(b))` arms — replace the lossy `as f64` cast
  with a call to this exact comparator (reversing the `Ordering` for the
  flipped-argument-order arm, same pattern CR-C5's own `Big`↔`Int`/`Big`↔
  `Dec` arms already use for their reversed counterparts).
- `order.rs::compare_qv_sort_keys`'s `(QvSortKey::I64(x), QvSortKey::F64(y))`
  / `(QvSortKey::F64(x), QvSortKey::I64(y))` arms — same replacement,
  keeping the EXISTING `.unwrap_or(std::cmp::Ordering::Equal)` NaN fallback
  wrapper around the call (this function's established convention for an
  incomparable result — do not change that convention, just make the
  NON-NaN case exact).

Update the doc comments at both sites (they currently say "OUT OF SCOPE
for this task's fix... tracked as a separate follow-up" — from CR-C5) to
reflect that this IS now that follow-up, landed.

## Docs

`docs/guide-docs/KNOWN_LIMITATIONS.md` §7 ("Numbers") currently says
NOTHING about this gap despite the CHANGELOG admitting it exists — add a
bullet. Since this task is expected to FIX the gap, the bullet should
describe it in PAST tense as a closed correctness fix (mirroring how
FG-6/CR-C5's own fixes are described in the CHANGELOG) — cite the exact
mechanism (bounds-check + floor/fract, no BigInt needed) briefly. If for
some reason you cannot land a fully-correct fix and must document a
narrower residual instead, be precise about exactly what's still broken,
mirroring CR-D2's honesty about its own residual mixed-type/NaN gap.

Also update the CHANGELOG's `[Unreleased]` FG-6 bullet (search for "Also
found and explicitly documented (NOT fixed, tracked as a separate
follow-up): plain `Int`↔`F64` comparison...") — reword it to describe this
fix having landed, instead of describing the gap as still-outstanding.

## Tests (TDD — write failing tests first)

In `crates/shamir-engine/src/query/filter/tests/eval_tests/dec_cross_type_tests.rs`
(the same file CR-C5 added its Big-number boundary tests to):

- **`i64::MAX` vs `i64::MAX - 1`, compared against an `F64` operand that
  collapses them under the OLD cast** — e.g. `compare_values(&Value::Int(i64::MAX),
  &Value::F64((i64::MAX - 1) as f64))` must NOT report `Equal` (prove this
  FAILS against the pre-fix code first, exactly like CR-C5's own
  regression-proof pattern — include a sanity assertion that the naive
  `as f64` casts of both operands really do collapse to the same value,
  same style as CR-C5's `compare_values_big_vs_dec_close_boundary_is_exact`
  test).
- **Boundary at `2^63`**: `f = 2^63` (exactly at the exclusive bound) vs
  `i = i64::MAX` — must report `Less` (f is strictly greater than any
  representable i64).
- **`i64::MIN` boundary**: `f = -2^63` (exactly `i64::MIN` as f64) vs
  `i = i64::MIN` — must report `Equal`.
- **Fractional tie-break**: `i = 5`, `f = 5.5` — must report `Less` (`i <
  f`); `i = 5`, `f = 4.5` — must report `Greater`.
- **NaN unchanged**: `compare_values(&Value::Int(5), &Value::F64(f64::NAN))`
  still returns `None` (the existing, unmodified NaN convention).
- **`QvSortKey` mirror**: the same `i64::MAX`-collapse scenario through
  `apply_order_by_qv`/`compare_qv_sort_keys`, proving a mixed `Int`+`F64`
  `ORDER BY` column now produces a correct total order at this boundary
  (not the old "compares Equal, falls back to insertion order" behavior).
- **Regression**: every existing FG-1/FG-6/CR-C5 test stays green —
  `Int`↔`Int`, `Int`↔`Dec`, `Big`↔`*` arms are UNTOUCHED by this task.

## Gate

```
cargo fmt -p shamir-engine -p shamir-query-types -- --check
cargo clippy -p shamir-engine -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-query-types --full
```

All must pass before returning. Primary code area: `shamir-engine`
(`query/filter/resolve.rs`, `query/read/order.rs`, their tests),
`KNOWN_LIMITATIONS.md`, `CHANGELOG.md`. Do NOT touch any `Big`-involving
arm (those are CR-C5's already-correct territory) or anything in
`shamir-server`'s cursor code (disjoint from this task).
