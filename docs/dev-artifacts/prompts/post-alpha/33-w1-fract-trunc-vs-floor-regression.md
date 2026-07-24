# Brief: W-1 — `cmp_i64_f64` tie-break uses trunc-based `fract()`, not floor-based (#788, URGENT release blocker)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — a genuine regression in this session's own CR-D3 fix, confirmed by direct compilation

CR-D3 (#784, commit `0e2cc0b8`) added `cmp_i64_f64` to
`crates/shamir-engine/src/query/filter/resolve.rs` (~lines 96-162) and a
mirrored copy in `crates/shamir-engine/src/query/read/order.rs`
(~lines 204-240), to make `Int`↔`F64` comparisons exact. The Equal-tie-break
branch (`resolve.rs:150-160`, `order.rs`'s mirror) is:

```rust
match i.cmp(&f_floor_i64) {
    Ordering::Equal => {
        if f.fract() > 0.0 {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Equal)
        }
    }
    other => Some(other),
}
```

This assumes `f.fract()` is FLOOR-based (i.e. `f - f.floor()`, always `>= 0`
for finite `f`). It is NOT — Rust's `f64::fract()` is defined as `self -
self.trunc()`, which is TRUNCATION-based and sign-preserving: for negative
`f`, `fract()` is negative or zero, never positive.

**Confirmed by direct compilation** (do this yourself before touching
anything, to build your own confidence in the bug — do not just trust this
brief's prose):

```rust
fn main() {
    let f: f64 = -0.5;
    println!("floor={}, trunc={}, fract={}", f.floor(), f.trunc(), f.fract());
    let f2: f64 = -4.5;
    println!("floor={}, trunc={}, fract={}", f2.floor(), f2.trunc(), f2.fract());
}
```

Output: `floor=-1, trunc=-0, fract=-0.5` and `floor=-5, trunc=-4,
fract=-0.5` — `fract()` is negative in both cases, `floor()` is the correct
floor value.

Trace `cmp_i64_f64(-1, -0.5)` through the current code: `f = -0.5` is
finite, within bounds. `f_floor = f.floor() = -1.0`, `f_floor_i64 = -1`.
`i.cmp(&f_floor_i64)` = `(-1).cmp(&-1)` = `Equal`. Then `f.fract() > 0.0` →
`-0.5 > 0.0` → `false` → returns `Some(Ordering::Equal)`. **WRONG**: the
correct answer is `i (-1) < f (-0.5)`, i.e. `Less` — `-1` is strictly less
than `-0.5` on the real number line. The SAME bug fires for
`cmp_i64_f64(-5, -4.5)` (also wrongly `Equal` instead of `Less`), and for
every `i64` `i` compared against any finite negative non-integer `f64`
whose floor equals `i`.

**Impact**: every one of `Eq`/`Ne`/`Lt`/`Gte`/`Lte` gives a wrong answer for
these pairs (`Eq` wrongly reports match, `Ne`/`Lt` wrongly report
non-match, `Gte`/`Lte` wrongly report true when they shouldn't), and
`ORDER BY` over a mixed `Int`+`F64` column produces a non-transitive
"equal" relationship for negative fractional values. The PRE-CR-D3 lossy
`as f64` cast handled every case with `|i| < 2^53` correctly (small
integers cast to `f64` exactly) — CR-D3 fixed the rare large-magnitude
collapse bug but introduced this NEW, far more common negative-fraction
bug in the process. None of CR-D3's own tests exercised a negative
fractional `F64` operand (`compare_values_int_f64_fractional_tie_break`
only tested `i = 5` against `f = 5.5`/`f = 4.5`, both positive).

## Fix — one-line change at both sites, floor-consistent instead of trunc-consistent

Replace the fractional-part check with a direct comparison against
`f_floor` (already computed a few lines above in both functions), which is
correct regardless of sign:

```rust
match i.cmp(&f_floor_i64) {
    Ordering::Equal => {
        // i == floor(f) exactly. f >= floor(f) always (floor rounds DOWN,
        // never up) -- f > floor(f) iff f has ANY nonzero fractional part
        // in the true floor sense, positive OR negative f alike. Comparing
        // against f_floor directly (not f.fract(), which is TRUNC-based
        // and sign-preserving -- f.fract() is negative for negative
        // fractional f, the bug this replaces) is correct for every sign.
        if f > f_floor {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Equal)
        }
    }
    other => Some(other),
}
```

Verify this yourself before applying: for `f = -0.5`, `f_floor = -1.0`,
`f > f_floor` → `-0.5 > -1.0` → `true` → correctly returns `Less` (i.e.
`i (-1) < f (-0.5)`). For `f = 5.5`, `f_floor = 5.0`, `f > f_floor` → `true`
→ correctly returns `Less` for `i = 5` (unchanged from before — the
positive case was already correct, this fix must not regress it). For an
exact integer `f` (e.g. `f = 5.0`), `f_floor = 5.0`, `f > f_floor` → `false`
→ correctly returns `Equal`. Confirm all three by direct reasoning (or a
scratch compile) before finalizing — do not just paste this without
verifying it yourself, same standard this whole campaign has held every
numeric fix to.

Apply the identical fix at BOTH sites:

- `crates/shamir-engine/src/query/filter/resolve.rs`'s `cmp_i64_f64`
  (~lines 150-160).
- `crates/shamir-engine/src/query/read/order.rs`'s mirrored `cmp_i64_f64`
  (~lines 228-238) — check the exact line numbers when you open the file,
  they may have drifted slightly since this brief was written.

Also fix BOTH doc comments (`resolve.rs:118-120` and `order.rs`'s
equivalent), which currently say "the sign of `f.fract()` (0 vs positive —
`f` is finite and `f >= f.floor()` always) breaks the tie" — this
"`f >= f.floor()` always" claim about `fract()`'s sign is the FALSE PREMISE
that caused this bug (it conflates `f >= f.floor()`, which IS always true,
with "`f.fract() >= 0` always", which is NOT true for `fract()`'s actual
trunc-based definition). Rewrite to state plainly that the comparison is
against `f_floor` directly (not `fract()`), and why that's sign-correct
where `fract()` is not.

## Tests (TDD — write failing tests first)

In `crates/shamir-engine/src/query/filter/tests/eval_tests/dec_cross_type_tests.rs`
(the same file CR-D3 added its own tests to — extend it, do not create a
new file):

- **The exact regression case**: `compare_values(&Value::Int(-1),
  &Value::F64(-0.5))` must be `Some(Ordering::Less)`, NOT `Equal`. Include a
  sanity assertion (mirroring CR-D3's own test style) that `(-1_i64 as f64)
  != -0.5` is irrelevant here (this isn't a magnitude-collapse case — the
  point is the TIE-BREAK logic itself, not the bounds/floor computation),
  so instead sanity-assert `(-0.5_f64).floor() == -1.0` and
  `(-0.5_f64).fract() < 0.0` (proving the OLD code's premise was false) as
  the setup-invariant check.
- **Mirror at `-5` / `-4.5`**: `compare_values(&Value::Int(-5),
  &Value::F64(-4.5))` must be `Some(Ordering::Less)`.
- **Reversed-argument-order mirror for both above**: `compare_values(&Value::F64(-0.5),
  &Value::Int(-1))` must be `Some(Ordering::Greater)` (and similarly for the
  `-5`/`-4.5` pair) — proves the `(F64, Int)` arm's `.map(Ordering::reverse)`
  still composes correctly with the fixed comparator.
- **Positive-fraction regression guard**: re-assert CR-D3's own existing
  `compare_values_int_f64_fractional_tie_break` test still passes unchanged
  (i = 5 vs f = 5.5 → Less; i = 5 vs f = 4.5 → Greater) — this fix must not
  regress the case that was already correct.
- **Exact-integer-valued negative `f64`**: `compare_values(&Value::Int(-3),
  &Value::F64(-3.0))` must be `Some(Ordering::Equal)` (proves the fix
  doesn't break the true-equal case for negative operands).
- **`QvSortKey` mirror** (in the same file, alongside CR-D3's own
  `order_by_mixed_int_and_f64_close_values_exact_total_order` test): an
  `ORDER BY` over a column mixing `Int(-5)`/`F64(-4.5)`/`Int(-6)` sorts to
  the correct total order `Int(-6) < Int(-5) < F64(-4.5)`, not the
  old-bug's `Int(-5)` and `F64(-4.5)` comparing `Equal`.

## Gate

```
cargo fmt -p shamir-engine -p shamir-query-types -- --check
cargo clippy -p shamir-engine -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-query-types --full
```

All must pass before returning. Primary code area: `shamir-engine`
(`query/filter/resolve.rs`, `query/read/order.rs`, their tests). Do NOT
touch anything else CR-D3 already landed correctly (the NaN handling, the
±2^63 boundary checks, the infinite-handling branch) — this is a narrow,
surgical fix to exactly the tie-break arm's sign bug, nothing more.
