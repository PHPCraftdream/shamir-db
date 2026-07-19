# Cleanup tail C — checked arithmetic + Big support in compare/cast

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

This brief covers FOUR small, independent LOW-severity findings (12, 13,
14, 15) from the release audit
(`docs/dev-artifacts/research/2026-07-17-release-audit/04-logical-correctness-bugs.md`).
Each fix below has already been investigated and the exact current code
read — line numbers are accurate as of this brief, but re-read each site
yourself before editing (this campaign has touched several of these files
in earlier stages).

---

## Fix 1 (Finding 12) — `$expr` `mod` unchecked remainder can panic

### The bug

`crates/shamir-engine/src/query/filter/resolve.rs`, `FilterExprOp::Mod` arm
(~lines 395-411):

```rust
FilterExprOp::Mod => {
    let [a, b] = args.as_slice() else {
        return None;
    };
    if let (QueryValue::Int(x), QueryValue::Int(y)) = (a, b) {
        if *y == 0 {
            return None;
        }
        return Some(QueryValue::Int(x % y));
    }
    let x = as_f64(a)?;
    let y = as_f64(b)?;
    if y == 0.0 {
        return None;
    }
    Some(QueryValue::F64(x % y))
}
```

`x % y` panics in debug/overflow-checked builds when `x == i64::MIN` and
`y == -1` (the one input pair where two's-complement remainder overflows).
The sibling ops (`Add`/`Sub`/`Mul`, via the `numeric_binop` helper just
above this match, ~lines 340-363) already guard their integer path with
`checked_add`/`checked_sub`/`checked_mul` and fall back to the float lane
on overflow — `Mod` is the one op that doesn't follow this pattern (note:
`Mod` can't simply reuse `numeric_binop` as-is, because `numeric_binop`'s
float path has no zero-divisor check, and `Mod`/`Div` both need one that
`Add`/`Sub`/`Mul` don't — keep `Mod`'s existing dual-branch shape, don't
try to force it through `numeric_binop`).

### The fix

Replace the unchecked `x % y` with `x.checked_rem(*y)`, and on the rare
`None` (only for the `i64::MIN % -1` case), fall through to the SAME float
computation the non-Int branch already does below it — mirroring how
`numeric_binop` falls back to float on Add/Sub/Mul overflow, rather than
returning `None` (which would make the whole `$expr` evaluate to absent —
surprising for a case that has a perfectly well-defined mathematical
answer, `0`, that just doesn't fit i64 two's-complement remainder
semantics):

```rust
FilterExprOp::Mod => {
    let [a, b] = args.as_slice() else {
        return None;
    };
    if let (QueryValue::Int(x), QueryValue::Int(y)) = (a, b) {
        if *y == 0 {
            return None;
        }
        if let Some(r) = x.checked_rem(*y) {
            return Some(QueryValue::Int(r));
        }
        // i64::MIN % -1 — two's-complement overflow artifact, not a real
        // undefined case (mathematically 0). Fall through to the float path.
    }
    let x = as_f64(a)?;
    let y = as_f64(b)?;
    if y == 0.0 {
        return None;
    }
    Some(QueryValue::F64(x % y))
}
```

### Tests

1. `{"$expr": {"op": "mod", "args": [-9223372036854775808, -1]}}` (i.e.
   `i64::MIN % -1`) must NOT panic and must evaluate to `0` (as `F64(0.0)`
   via the fallback, or however your fix shapes it — assert on the actual
   value, don't just assert "doesn't panic").
2. Regression: ordinary `mod` cases (`7 % 3`, negative operands, float
   operands) continue to work exactly as before.
3. Regression: the existing `y == 0` → `None` behavior for both Int and
   float paths is unchanged.

---

## Fix 2 (Finding 13a) — Sum accumulator unchecked i64 overflow

### The bug

`crates/shamir-engine/src/query/read/aggregate.rs`, `AggState::Sum`'s
`step` arm (~line 389-403):

```rust
AggState::Sum { sum_i, sum_f, has_float } => {
    if let Some(s) = scalar {
        match s {
            ScalarRef::Int(i) => *sum_i += i,
            ScalarRef::F64(f) => {
                *has_float = true;
                *sum_f += f;
            }
            _ => {}
        }
    } else {
        // ... Dec/Big/container fallback — materialize this one field,
        // Dec/Big flow into the f64 lane via agg_leaf_to_f64 ...
    }
}
```

`*sum_i += i` is unchecked — a sum crossing `±2^63` panics (debug) or
silently wraps (release), producing a wrong total with no error. Note the
float lane (`has_float`/`sum_f`) ALREADY exists precisely for this kind of
"can't stay in the int lane" case (it's how Dec/Big/F64 rows are folded
in) — this fix reuses that exact same lane for integer overflow, it does
not invent a new mechanism.

### The fix

Read `finish()`'s `AggState::Sum` arm too (~line 552-568) before editing —
it computes `sum_f + sum_i as f64` when `has_float`, so the fix must keep
`sum_i` holding an always-exact (never-overflowed) partial sum, diverting
any row that WOULD overflow it into `sum_f` instead:

```rust
ScalarRef::Int(i) => match sum_i.checked_add(i) {
    Some(new_sum) => *sum_i = new_sum,
    None => {
        *has_float = true;
        *sum_f += i as f64;
    }
},
```

### Tests

1. Summing values that individually fit in `i64` but whose running total
   crosses `i64::MAX` (e.g. two rows near `i64::MAX/2 + 1` each) must NOT
   panic, and must produce a `QueryValue::F64` result close to the true
   mathematical sum (not a wrapped/garbage `Int`).
2. Regression: an all-Int sum that stays well within `i64` range continues
   to return `QueryValue::Int` (not lifted to float unnecessarily).
3. Regression: existing Dec/Big/F64 sum-lifting tests continue to pass
   unchanged.

---

## Fix 3 (Finding 13b) — `diff_secs` unchecked subtraction

### The bug

`crates/shamir-funclib/src/datetime.rs`, the `diff_secs` registration
(~lines 205-216):

```rust
reg.register(
    "diff_secs",
    FnEntry::pure(
        |a| {
            let x = arg_i64(a, 0)?;
            let y = arg_i64(a, 1)?;
            Ok(v_int(div_floor(x - y, 1000)))
        },
        2,
        Some(2),
    ),
);
```

`x - y` is unchecked millisecond-timestamp subtraction — two adversarial
or corrupted timestamps far enough apart overflow `i64`, panicking in
debug builds. Every other date-arithmetic function in this file that can
overflow already returns `Err(ScalarError::new("out_of_range"))` on
`checked_*` failure (see `add_days`'s `checked_add_signed` pattern
immediately above this function, ~lines 196-198) — mirror that exact
style.

### The fix

```rust
reg.register(
    "diff_secs",
    FnEntry::pure(
        |a| {
            let x = arg_i64(a, 0)?;
            let y = arg_i64(a, 1)?;
            let diff = x
                .checked_sub(y)
                .ok_or_else(|| ScalarError::new("out_of_range"))?;
            Ok(v_int(div_floor(diff, 1000)))
        },
        2,
        Some(2),
    ),
);
```

### Tests

1. `diff_secs` with two timestamps whose difference overflows `i64` must
   return `Err` with code `out_of_range`, not panic.
2. Regression: ordinary `diff_secs` cases (positive/negative/zero diffs)
   continue to return the correct value — re-run the existing
   `diff_secs` test in `crates/shamir-funclib/src/datetime/tests/datetime_tests.rs`
   and confirm it still passes.

---

## Fix 4 (Finding 14) — funclib `compare`'s Int↔Big loses precision via f64

### The bug

`crates/shamir-funclib/src/compare.rs`'s `compare_numeric` (~lines
100-119) — the module's OWN doc comment (~line 99: "Anything involving
Big: try Decimal first (if Big fits); else f64") does not match the
actual code, which sends EVERY Big-involving pair straight to f64:

```rust
fn compare_numeric(a: &QueryValue, b: &QueryValue) -> Ordering {
    match (a, b) {
        (QueryValue::Int(x), QueryValue::Int(y)) => return x.cmp(y),
        (QueryValue::Dec(x), QueryValue::Dec(y)) => return x.cmp(y),
        (QueryValue::F64(x), QueryValue::F64(y)) => return cmp_f64(*x, *y),
        (QueryValue::Big(x), QueryValue::Big(y)) => return x.cmp(y),
        _ => {}
    }
    match (a, b) {
        (QueryValue::Int(x), QueryValue::Dec(d)) => return Decimal::from(*x).cmp(d),
        (QueryValue::Dec(d), QueryValue::Int(y)) => return d.cmp(&Decimal::from(*y)),
        _ => {}
    }
    // Anything else (F64 or Big involved) -- convert to f64.
    let fa = to_f64(a);
    let fb = to_f64(b);
    cmp_f64(fa, fb)
}
```

`compare(Int(i64::MAX), Big(i64::MAX - 1))` converts both to `f64`, which
rounds to the SAME float value → reports `Equal` although the two values
genuinely differ. This affects `min`/`max`/`between`/`clamp` in funclib and
`count_distinct`/`mode` (two such values get counted as one) — the exact
same aggregators that Fix 3 of the PRIOR stage of this cleanup campaign
(`cleanup-tail-a`) fixed for Set/Map structural equality; this is the
analogous numeric-precision gap in the same comparator.

### The fix

Add an exact `Int`↔`Big` arm using `num_bigint::BigInt`'s own comparison —
this is UNCONDITIONALLY exact regardless of magnitude (no "does it fit"
fallback needed, unlike the `Int`↔`Dec` arm above it, because `BigInt` has
arbitrary precision and `BigInt::from(i64)` always succeeds):

```rust
match (a, b) {
    (QueryValue::Int(x), QueryValue::Dec(d)) => return Decimal::from(*x).cmp(d),
    (QueryValue::Dec(d), QueryValue::Int(y)) => return d.cmp(&Decimal::from(*y)),
    (QueryValue::Int(x), QueryValue::Big(y)) => return BigInt::from(*x).cmp(y),
    (QueryValue::Big(x), QueryValue::Int(y)) => return x.cmp(&BigInt::from(*y)),
    _ => {}
}
```

(`num_bigint::BigInt` is already imported at the top of this file — check
the existing `use` block.) **Scope this fix to Int↔Big only** — Dec↔Big
and F64↔Big are NOT part of finding 14's trigger and stay on the f64
fallback path for now (a full Dec↔Big exact path would need to handle
`Decimal`'s bounded range vs `BigInt`'s unbounded range, which is a bigger
change than this LOW-severity finding calls for). Since the module doc
comment currently claims broader Big-handling than the code implements,
update the doc comment's wording to accurately describe what's implemented
after your fix (Int↔Big exact via BigInt; Dec↔Big and F64↔Big still via
f64) — don't leave stale/inaccurate documentation in place, but don't
implement the broader Dec↔Big path just to make the OLD doc comment true.

### Tests

1. `compare(Int(i64::MAX), Big(BigInt::from(i64::MAX) - 1))` must return
   `Greater`, NOT `Equal` (the exact trigger from the audit).
2. `compare(Int(5), Big(BigInt::from(5)))` → `Equal`.
3. Regression: `Big`↔`Big` and `Dec`↔`Dec` comparisons unaffected.
4. `count_distinct` or `min`/`max` over a mix of `Int(i64::MAX)` and
   `Big(i64::MAX - 1)` now correctly treats them as distinct/orders them
   correctly (whichever this codebase's existing funclib agg tests are
   structured to check — mirror the style of the Set/Map tests added in
   the prior `cleanup-tail-a` stage,
   `crates/shamir-funclib/src/compare/tests/compare_tests.rs` and
   `crates/shamir-funclib/src/agg/tests/agg_tests.rs`).

---

## Fix 5 (Finding 15) — `cast_to_int`/`cast_to_dec` reject `Big` even when it fits

### The bug

`crates/shamir-funclib/src/cast.rs`'s `cast_to_int` (~lines 107-130) and
`cast_to_dec` (~lines 134-145) both fall through to `_ =>
Err(ScalarError::new("cast_failed"))` for `QueryValue::Big` unconditionally
— even when the `Big` value trivially fits in an `i64`/`Decimal`. The
CORRECT pattern already exists in this same crate:
`crates/shamir-funclib/src/agg.rs`'s `to_dec` helper (~lines 130-150,
already used as the reference pattern by earlier stages of this campaign)
tries `b.to_i64()` first (exact, for values that fit), then `b.to_f64()`
as a lossy last resort, then errors only if neither works. Mirror this
EXACT precedent — do not invent a different fallback order.

(Note: there is no separate `cast_to_float` function — the `"float"` cast
target dispatches straight to `cast_to_dec`, per the `"float" =>
cast_to_dec(value)` line near the top of this file. Fixing `cast_to_dec`
covers `"float"` too; you do not need to touch anything else for that
target name.)

### The fix

```rust
fn cast_to_int(v: &QueryValue) -> Result<QueryValue, ScalarError> {
    match v {
        QueryValue::Int(n) => Ok(v_int(*n)),
        QueryValue::Bool(b) => Ok(v_int(*b as i64)),
        QueryValue::Dec(d) => { /* unchanged */ }
        QueryValue::F64(f) => { /* unchanged */ }
        QueryValue::Big(b) => b
            .to_i64()
            .map(v_int)
            .ok_or_else(|| ScalarError::new("cast_failed")),
        QueryValue::Str(s) => parse_int_str(s),
        _ => Err(ScalarError::new("cast_failed")),
    }
}

fn cast_to_dec(v: &QueryValue) -> Result<QueryValue, ScalarError> {
    match v {
        QueryValue::Dec(d) => Ok(v_dec(*d)),
        QueryValue::Int(n) => Ok(v_dec(Decimal::from(*n))),
        QueryValue::Bool(b) => Ok(v_dec(Decimal::from(*b as i64))),
        QueryValue::F64(f) => { /* unchanged */ }
        QueryValue::Big(b) => {
            if let Some(n) = b.to_i64() {
                Ok(v_dec(Decimal::from(n)))
            } else if let Some(f) = b.to_f64() {
                Decimal::from_f64_retain(f)
                    .map(v_dec)
                    .ok_or_else(|| ScalarError::new("cast_failed"))
            } else {
                Err(ScalarError::new("cast_failed"))
            }
        }
        QueryValue::Str(s) => parse_dec_str(s),
        _ => Err(ScalarError::new("cast_failed")),
    }
}
```

(`cast_to_int`'s `Big` arm doesn't need the f64-fallback dance `to_dec`/
`cast_to_dec` use — an `i64` target that doesn't fit via `to_i64()` has no
better lossy fallback that stays an integer, so error directly, matching
this function's existing `F64` arm which also just errors on non-exact
values rather than truncating.)

### Tests

1. `cast(Big(BigInt::from(42)), "int")` → `Int(42)`.
2. `cast(Big(huge value that overflows i64), "int")` → `Err("cast_failed")`
   (still correctly rejected — this fix does not change behavior for
   values that genuinely don't fit).
3. `cast(Big(BigInt::from(42)), "dec")` → `Dec(Decimal::from(42))`.
4. `cast(Big(huge value that overflows i64 but fits as f64), "dec")` →
   succeeds via the f64 fallback (mirroring `to_dec`'s own behavior for the
   same case — check `to_dec`'s existing tests in
   `crates/shamir-funclib/src/agg/tests/agg_tests.rs` if any exist for this
   exact scenario, and mirror that expectation).
5. `cast(Big(...), "float")` → same as `"dec"` (since `"float"` dispatches
   to `cast_to_dec`) — confirm this explicitly with one test.
6. Regression: all existing `cast_to_int`/`cast_to_dec` tests for
   Int/Bool/Dec/F64/Str inputs continue to pass unchanged.

## Out of scope

- Do NOT implement a full Dec↔Big exact comparison path (Fix 4 scopes
  strictly to Int↔Big).
- Do NOT touch `numeric_binop`'s Add/Sub/Mul/Div handling — only `Mod`
  needed a fix (Fix 1).
- Do NOT touch `AggState::Avg`/`Min`/`Max` — only `Sum`'s accumulator has
  the unchecked-overflow issue (Fix 2); confirm this by reading `Avg`'s
  arm (it already accumulates in `f64` unconditionally, so it has no
  analogous overflow risk).
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, or the
  coercing-set-probes/ScalarResolver-threading/self-referential-FK work
  (tasks 1a-1e, 2a-2e, 3a, 3b, 3c, 3d) — this brief is scoped to findings
  12-15 only.

## Verification (MANDATORY before you report done, for ALL FIVE fixes)

- `./scripts/test.sh @engine --full` green (covers Fixes 1, 2).
- `./scripts/test.sh -p shamir-funclib --full` green (covers Fixes 3, 4, 5).
- `cargo fmt --all -- --check` clean (or scoped to touched crates, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the `$expr mod` fix falls back to the float path
  on `i64::MIN % -1` rather than returning `None`; (b) the Sum-overflow
  fix reuses the existing `has_float`/`sum_f` lane rather than introducing
  a new field; (c) the `compare_numeric` doc comment now accurately
  describes what's implemented for every numeric pairing.
