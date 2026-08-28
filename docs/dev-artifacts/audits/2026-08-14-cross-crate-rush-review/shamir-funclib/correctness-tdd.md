# shamir-funclib -- Correctness & TDD-coverage

## Summary

The crate is in strong shape against CLAUDE.md's Red/Green/Refactor discipline: every category module carries a per-function test file with at least one happy-path and one error/edge assert, known-answer crypto vectors, regression tests that name the bug they pin (NaN Hash/Eq dedup, Int-vs-Big precision at i64::MAX, map/set canonicalization, chrono strftime panic conversion), and even an anti-vacuous assertion (`peak == cap`) in the argon2id concurrency-cap test. The real gaps are at the edges, not the happy paths: two query-reachable panic/abort paths (unguarded allocations, one `unwrap()` on user-parsed dates), a one-ulp F64→i64 boundary check that silently saturates instead of returning `out_of_range`, an unchecked subtraction in `age` that contradicts the guarded `diff_secs` right next to it, and a split equality semantic (`==` vs `compare`) that makes the same pair of values compare differently depending on which function you call. `scalar_resolver.rs` is the only module with no tests of its own.

## Findings

### 1. Query-reachable unbounded allocations / panics with no `ScalarError` path
- **File:line:** `crates/shamir-funclib/src/gen.rs:75`; `crates/shamir-funclib/src/strings.rs:237`; `crates/shamir-funclib/src/strings.rs:406`
- **Severity:** medium
- **Issue:** `gen/random_bytes(n)` does `vec![0u8; n as usize]` and `strings/repeat(s, n)` does `s.repeat(n as usize)` with no upper bound on `n` (only `n < 0` is rejected). `pad_left`/`pad_right` have the same shape via `repeat_n(ch, target - cur)` (`strings.rs:406`). `str::repeat` panics with "capacity overflow" on `len * n` overflow and aborts (allocation failure) on large-but-valid sizes — neither is a `ScalarError`, so the crash escapes the `fn(&[QueryValue]) -> ScalarResult` contract.
- **Failure scenario:** a guest/filter expression calls `strings/repeat('a', 9223372036854775807)` → capacity-overflow panic in the query thread, or `gen/random_bytes(2^40)` → allocation failure → process abort. This is exactly the resource-exhaustion class crypto.rs capped for `argon2id` (`A2_MAX_LENGTH = 256`, audit §2b, `crypto.rs:59-62`) — the same posture is not applied to these three functions.
- **Suggested fix:** add explicit caps (e.g. `random_bytes` ≤ 1 MiB, `repeat` output ≤ a few MB, `pad` target width ≤ a few MB) returning `ScalarError("out_of_range")`, mirroring the `argon2id` parameter-ceiling pattern; add boundary tests alongside `random_bytes_negative_is_error`.

### 2. `datetime/parse` can panic on a valid-input-format date (unguarded `unwrap`)
- **File:line:** `crates/shamir-db/crates/shamir-funclib/src/datetime.rs:153`
- **Severity:** medium
- **Issue:** the date-only fallback does `NaiveDate::parse_from_str(s, pattern).map(|d| d.and_hms_opt(0, 0, 0).unwrap())`. `and_hms_opt` returns `None` for dates inside `NaiveDate`'s range but outside `NaiveDateTime`'s (e.g. year -262144, which chrono's signed `%Y` parses), so the `unwrap` panics on user-supplied input — violating the error-handling rule (panic only for programmer bugs).
- **Failure scenario:** `datetime/parse('-262144-01-01', '%Y-%m-%d')` panics the query thread instead of returning a machine-readable error.
- **Suggested fix:** replace with `and_hms_opt(0,0,0).ok_or_else(|| ScalarError::new("out_of_range"))?`. Note the module already models the right discipline one function over: `validate_pattern` + the dedicated `format_with_malformed_pattern_returns_err_not_panic` test convert chrono's other panic into `ScalarError("parse")` — this site got no such treatment or test.

### 3. F64→i64 range check is off by one ulp: silently saturates instead of `out_of_range`
- **File:line:** `crates/shamir-funclib/src/registry.rs:218` (`arg_i64`); `crates/shamir-funclib/src/cast.rs:121` (`cast_to_int`)
- **Severity:** low
- **Issue:** the guard `*f <= i64::MAX as f64` accepts `f = 2^63.0`, because `i64::MAX as f64` rounds *up* to exactly 2^63. The subsequent `*f as i64` saturating cast then yields `i64::MAX` (9223372036854775807) — a wrong value — where the honest answer is an error. The `Dec` arm right above it does exactly the honest thing (`to_i64()` → `None` → error), so the two numeric paths disagree at the same boundary.
- **Failure scenario:** `math/round`-adjacent plumbing or `cast/to_int(F64(9223372036854775808.0))` returns 9223372036854775807 instead of `out_of_range`/`cast_failed`.
- **Suggested fix:** use a roundtrip check (`*f as i64 as f64 == *f && *f > -(9.223372036854776e18)`) or reject `f >= 9.223372036854776e18` explicitly; add a boundary test (none of the suites test the `i64::MAX as f64` edge today, even though `compare_tests` covers the *sibling* precision bug for Int↔Big).

### 4. `datetime/age`: unchecked subtraction and truncating division, inconsistent with sibling `diff_secs`
- **File:line:** `crates/shamir-funclib/src/datetime.rs:78`
- **Severity:** low
- **Issue:** `(Utc::now().timestamp_millis() - then) / 1000` performs an unchecked `i64` subtraction (panics in debug / wraps in release for extreme `then`) and truncates toward zero, whereas `diff_secs` (line 266) uses `checked_sub` + `div_floor` — and even has a dedicated overflow test (`datetime_tests.rs:177 diff_secs_overflow_returns_error`).
- **Failure scenario:** `age(Int(i64::MIN))` panics a debug-build test/runtime or silently wraps in release; `age` of a future timestamp rounds differently (−1 vs −2 s) than the crate's documented floor-division convention (`div_floor`, line 312).
- **Suggested fix:** reuse `diff_secs`' shape: `checked_sub` → `out_of_range`, and route through `div_floor` for the /1000; test the `i64::MIN` input the way `diff_secs` does.

### 5. Two coexisting equality semantics: `==` (variant-strict) vs `compare` (value-strict)
- **File:line:** `crates/shamir-funclib/src/arrays.rs:85,99,170` vs `crates/shamir-funclib/src/null.rs:70`, `crates/shamir-funclib/src/agg.rs:205`, `crates/shamir-funclib/src/compare.rs:59-64`
- **Severity:** low
- **Issue:** `arrays/contains`, `arrays/index_of` (and hash-based `arrays/distinct`, whose comment consciously pins "matches the legacy == behaviour") use `QueryValue`'s `PartialEq`/`Hash`, which is variant-strict: `Int(5) != Dec(5.0)`, `Int(5) != F64(5.0)`. Everything else in the crate (`nullif`, `count_distinct`, `DistinctWrapper`, `math/min`/`max`, `arrays/sort`, `mode`, `median`) uses `compare`, under which those pairs are `Equal`. The ±0.0 case diverges three ways: `compare` says Equal, `Value::hash` (`shamir-types/src/types/value.rs:697-710`) canonicalizes NaN but *not* −0.0 (so `arrays/distinct([0.0, -0.0])` keeps both), while `canonical_hash` normalizes −0.0 (`canonical.rs:95-96`).
- **Failure scenario:** `arrays/contains([Dec(5.0)], Int(5))` → `false`, while `null/nullif(Int(5), Dec(5.0))` → `Null` and `count_distinct` counts the pair as 1 — the same "equality" answers three different ways depending on the function; a CHECK constraint and a filter over the same data can disagree.
- **Suggested fix:** either route `contains`/`index_of`/`distinct` through `compare`-equality (matching `count_distinct`), or document the split as a deliberate semantic in the arrays module doc and add cross-module parity tests; separately, canonicalize −0.0 in `Value::hash` the way NaN already is (shamir-types side, but it manifests here).

### 6. `compare`'s documented "total (transitive)" order is not strictly guaranteed on the lossy fallback paths
- **File:line:** `crates/shamir-funclib/src/compare.rs:3-6` (doc), `compare.rs:101-102,124-127` (f64 fallback)
- **Severity:** low
- **Issue:** the module doc promises a total, transitive order, but paths mixing exact comparisons (Int/Dec, Int/Big, Dec/Dec, Big/Big) with the lossy f64 conversion (Dec↔Big, anything×F64) can make `Equal` non-transitive: two adjacent huge `Dec`s compare `Less` exactly yet both compare `Equal` to a `Big` whose f64 rounding collapses them. `compare_tests.rs` tests transitivity/totality only for Sets/Maps and a value matrix whose numerics don't straddle the rounding boundary, so the invariant is asserted where it holds and untested where it doesn't.
- **Failure scenario:** canonicalization guarantees phrased on top of `compare` ("structurally-equal containers compare Equal", `compare_sets` doc) can mis-group near-boundary numeric sets; sort results are stable but "equal" runs are not interchangeable across paths.
- **Suggested fix:** either soften the module doc to "total up to documented f64-lossy approximation" (the `compare_numeric` doc already flags lossiness — the *top-level* doc overclaims), or implement an exact Dec↔Big comparison path; add a boundary triple test (Dec a < Dec b exact, both == Big c via f64).

### 7. `scalar_resolver.rs` has zero in-crate tests
- **File:line:** `crates/shamir-funclib/src/scalar_resolver.rs` (whole file; no `tests/` dir, no `#[cfg(test)] mod tests;`)
- **Severity:** low
- **Issue:** the 2-layer resolver's core contract — user layer shadows builtins, `get()` layering, arity/error-code parity with `ScalarRegistry::call` (duplicated in `dispatch_entry`, lines 135-145), and `builtins_only()`'s process-wide `OnceLock` sharing — is exercised only *indirectly* by shamir-engine tests (`resolver_with_user_scalar` helpers in `shamir-engine/src/query/read/tests/select_projection_tests.rs`, `exec_tests.rs`, `query/batch/tests/for_each_tests.rs`). Against CLAUDE.md's "one tests/ directory per module" + per-module Red/Green discipline, this module's red tests live in someone else's crate and assert engine behavior, not resolver semantics (e.g. nothing anywhere asserts that a user scalar whose name collides with a builtin wins, or that `UserScalarLayer::register` replaces an entry).
- **Failure scenario:** a future change to `dispatch_entry` or the shadowing order that keeps engine happy-paths green could still regress shadowing/arity parity with no failing test in this crate to catch it.
- **Suggested fix:** add `src/scalar_resolver/tests/resolver_tests.rs`: user-shadows-builtin, builtin-fallback, unknown_function, arity parity vs `ScalarRegistry::call`, `register`-replace semantics, and `builtins_only()` returning the same `Arc`/registry instance.

### 8. `strings/substring` doc comment contradicts implemented (and tested) behavior
- **File:line:** `crates/shamir-funclib/src/strings.rs:129-130` vs `crates/shamir-funclib/src/tests/strings_tests.rs:73-81`
- **Severity:** low
- **Issue:** the inline comment says "Out-of-range start/negative args -> out_of_range", but the code only rejects negatives; `start` past the char count silently yields `""`, and the test *pins* that ("start past end -> empty string (not an error)"). Also inconsistent with the sibling `arrays/get`, which errors `out_of_range` past the end.
- **Failure scenario:** none at runtime — but the comment is the contract the next editor will "fix" the code toward (or vice versa), and `arrays/get` vs `strings/substring` diverge for the same conceptual mistake.
- **Suggested fix:** correct the comment to match the tested behavior (or change behavior + test to error, matching `arrays/get`), and state the cross-module indexing rule in one place.

### 9. Nits (test organization / dead code / minor drift)
- **File:line:** various
- **Severity:** nit
- **Issue / Suggested fix:**
  - `src/math/tests/registry_tests.rs` tests `crate::registry` (dispatch, extractors, constructors) but lives under the `math` module's test dir — move to a `registry` tests home or rename the concern.
  - Five modules (`strings`, `object`, `encode`, `value_nav`, shared `register_builtins`) keep tests in the crate-root `src/tests/` instead of per-module `src/<mod>/tests/` dirs like their 12 siblings — CLAUDE.md's "one tests/ directory per module" is met only loosely here.
  - `BoolAndAgg`/`BoolOrAgg` carry a dead `any` field kept alive by `let _ = self.any;` (`agg.rs:724,760`) — delete it or use it (e.g. to distinguish empty input if the SQL-NULL-on-empty convention is ever adopted).
  - `validate/matches` compiles the user regex fresh on every call and uses error code `bad_pattern` (`validate.rs:342`) where the strings family caches via `compile()` and uses `bad_regex` (`strings.rs:427`) — two cache policies and two machine codes for one operation; unify and add the code to the frontend localization map.
  - `math.rs:4-5` (and several sibling module headers) still say "plain names, no folder prefix" while `register_builtins` folder-qualifies everything — `gen.rs`/`null.rs` docs state the folder-qualification correctly; align the rest.
  - `canonical.rs:187 serialise_key` silently maps serialization failure to empty bytes (`unwrap_or_default`), which would collapse distinct keys into one sort bucket — infallible for `String` keys today, but a latent nondeterministic-hash hazard if a non-string key type with a fallible `Serialize` ever flows in; prefer `expect`/error.
  - `value_nav type_of` tests cover only int/string/list/map/bool/null — the `f64`/`dec`/`big`/`bytes`/`set` variant names are unasserted.

