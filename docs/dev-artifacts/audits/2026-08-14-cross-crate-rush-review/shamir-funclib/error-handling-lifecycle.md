<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-funclib — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Recursion and output amplification can escape unwind isolation; reductions and age can instead produce unwindable panics or wrong arithmetic. Native duplicate registration violates its documented replacement contract. Safe invariant sites should not remain classified as repair obligations.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `validate/is_json` hand-rolled parser recurses without a depth limit — query-reachable stack overflow aborts the process

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

value recursively calls array/object, which call value again, with no depth admission. Nested JSON is carried inside Str and bypasses structural codec limits. Shallow tests offer no depth-bound oracle; precise failing depth is unknown.

Evidence: [crates/shamir-funclib/src/validate.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/validate.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L154); [crates/shamir-funclib/src/validate.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L178); [crates/shamir-funclib/src/validate/tests/validate_tests.rs:183](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate/tests/validate_tests.rs#L183).

<a id="review-2"></a>

### Claim 2 — Unbounded-allocation scalar paths: `strings/repeat`, `strings/pad_left`/`pad_right`, `gen/random_bytes`

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Only negativity is rejected before result allocation. Checked multiplication, checked platform-size conversion and byte ceilings are absent; pad additionally builds intermediate fill/output strings. Existing tests do not assert upper-bound rejection.

Evidence: [crates/shamir-funclib/src/gen.rs:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/gen.rs#L71); [crates/shamir-funclib/src/strings.rs:233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L233); [crates/shamir-funclib/src/strings.rs:406](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L406); [crates/shamir-funclib/src/tests/strings_tests.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/tests/strings_tests.rs#L168).

<a id="review-3"></a>

### Claim 3 — `UserScalarLayer::register` discards scc's `Err` — documented "(or replace)" silently never replaces

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Published scc 3.8.4 leaves occupied keys unchanged; the discarded Err retains the old native callable. Reopen tests create a fresh layer. A same-layer A-then-B invocation oracle is missing; WASM replacement is a different registry.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L39); [crates/shamir-funclib/src/scalar_resolver.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L41); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123); [crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs:401](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs#L401); [crates/shamir-wasm-host/src/registry.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/registry.rs#L50).

<a id="review-4"></a>

### Claim 4 — `rust_decimal` arithmetic panics on overflow in agg and array reductions

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Published rust_decimal 1.40.0 operators panic on overflow. Sum/avg retain unchecked addition, variance unchecked sum/subtraction/squaring and range unchecked subtraction. Native MAX pairs and query Int deviations ±10^15 provide distinct witnesses; no corresponding error-code oracle is registered.

Evidence: [crates/shamir-funclib/src/agg.rs:286](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L286); [crates/shamir-funclib/src/agg.rs:322](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L322); [crates/shamir-funclib/src/agg.rs:514](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L514); [crates/shamir-funclib/src/agg.rs:852](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L852); [crates/shamir-funclib/src/arrays.rs:276](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/arrays.rs#L276); [Cargo.lock:2979](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2979).

<a id="review-5"></a>

### Claim 5 — `datetime/age` performs unchecked `i64` subtraction — inconsistent with the file's own checked discipline

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

now_ms-i64::MIN overflows at the current positive epoch. Debug arithmetic can panic and unchecked release arithmetic can wrap. The ordinary-past test cannot discriminate this; checked subtraction and an explicit duration-rounding policy are needed.

Evidence: [crates/shamir-funclib/src/datetime.rs:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/datetime.rs#L78); [crates/shamir-funclib/src/datetime.rs:264](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/datetime.rs#L264); [crates/shamir-funclib/src/datetime/tests/datetime_tests.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/datetime/tests/datetime_tests.rs#L47).

<a id="review-6"></a>

### Claim 6 — `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Builtin code spellings lack a central catalog and errors own Strings. bad_regex and bad_pattern are distinct established outputs. Error/Display are implemented correctly; absent detail and thiserror conversion are optional, and client localization failure is unproved.

Evidence: [crates/shamir-funclib/src/registry.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/registry.rs#L20); [crates/shamir-funclib/src/registry.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/registry.rs#L31); [crates/shamir-funclib/src/strings.rs:427](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L427); [crates/shamir-funclib/src/validate.rs:342](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L342).

<a id="review-7"></a>

### Claim 7 — Argon2id semaphore: poison-propagating `expect`s, and the in-flight counter leaks on panic

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Poison propagation and non-RAII telemetry remain conditional hardening concerns. The current wait critical section supplies no demonstrated panic trigger. Normal KDF errors decrement correctly; permit Drop still releases on unwind.

Evidence: [crates/shamir-funclib/src/crypto.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L139); [crates/shamir-funclib/src/crypto.rs:141](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L141); [crates/shamir-funclib/src/crypto.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L178); [crates/shamir-funclib/src/crypto.rs:225](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L225); [crates/shamir-funclib/src/crypto.rs:230](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L230).

<a id="review-8"></a>

### Claim 8 — Canonical-hash path silently degrades serialization failures to empty key bytes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A native custom Serialize key can return Err; public canonical APIs silently encode it as empty. Different single-key maps can then have identical canonical bytes. Published rmp-serde 1.3.1 explicitly permits user serialization failure; ordinary String keys do not trigger it.

Evidence: [crates/shamir-funclib/src/canonical.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/canonical.rs#L199); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

Grouping/duplicate: [security-crypto.md#8](security-crypto.md#review-8). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — `ScalarResolver` / `UserScalarLayer` have zero tests — their error paths are unverified

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No dedicated suite exercises duplicate registration, shadowing, resolver unknown/arity parity or concurrency. The registered facade test proves a successful dispatch seam, not these independent obligations.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L135); [crates/shamir-funclib/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/tests/mod.rs#L1); [crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs#L48).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-10"></a>

### Claim 10 — Unannotated panicky/fallible primitives in library code

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

chrono 0.4.43 makes midnight conversion infallible; rust_decimal 1.40.0 to_f64 returns Some for valid Decimal values; the Reduce branch is excluded by its enclosing match. No rule requires replacing safe invariant expressions. The cited agg unreachable site is absent.

Evidence: [crates/shamir-funclib/src/datetime.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/datetime.rs#L153); [crates/shamir-funclib/src/agg.rs:472](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L472); [crates/shamir-funclib/src/arrays.rs:248](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/arrays.rs#L248); [Cargo.lock:655](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L655); [Cargo.lock:2979](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2979).

## Evidence and recipe corrections

- Safe invariant annotation and Decimal-to-f64 readability changes are optional, not remaining runtime error paths.
- Atomic replacement is not sufficient safety for indexed scalars: persisted functional indexes resolve functions by live name. Redefinition must preserve semantics or coordinate index rebuilding/versioning.
- Do not use remove-then-insert replacement; it exposes absence and concurrent insertion windows.
- Decimal and capacity-overflow panics are not automatically process aborts under panic=unwind.
- Checked sum can still reject a representable final average because an intermediate sum overflows; document that numerical/error policy rather than claiming universal mathematical equivalence.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-funclib -- Error handling & resource lifecycle

## Summary

The crate's `Result`/`ScalarError` discipline is largely exemplary for its documented "machine-code-only" error model: `?` propagation is uniform, panics are rare and mostly invariant-annotated, datetime defensively pre-validates strftime patterns to dodge a chrono panic, the argon2id permit is proper RAII, and per-function error-code assertions (`unwrap_err().code`) are pervasive across every `tests/` dir. The real gaps are at the extremes of the input space: one query-reachable unbounded-recursion validator that aborts the process via stack overflow (uncatchable, defeating the workspace's `panic = "unwind"` isolation design), several unbounded-allocation i64 paths that panic/abort instead of returning `out_of_range`, and `rust_decimal` panic-on-overflow arithmetic in the agg/array reductions. The one lifecycle-specific bug is `UserScalarLayer::register` discarding scc's `Err`-on-existing-key so its documented "(or replace)" semantics silently never happen — untested because `ScalarResolver`/`UserScalarLayer` have no tests at all.

## Findings

### 1. `validate/is_json` hand-rolled parser recurses without a depth limit — query-reachable stack overflow aborts the process
- File:line: `crates/shamir-funclib/src/validate.rs:123-191` (`TextFormatParser::value`/`object`/`array`), registered at `validate.rs:349-356`
- Severity: critical
- Issue: `is_text_encoded_str` is a recursive-descent JSON validator with **no recursion-depth cap**: `value()` → `object()`/`array()` → `value()` per nesting level, bounded only by input length. Every other JSON path in the crate uses `serde_json` (default 128-level recursion limit, see `encode.rs:160` `parse_json`), but this bespoke parser bypasses it.
- Failure scenario: `SELECT validate/is_json('[[[[[...')` with ~10⁵–10⁶ `[` characters (one unprivileged scalar call) exhausts the thread stack. A stack overflow is a hard abort — `catch_unwind` cannot intercept it, so the panic-isolation boundary documented in the root `Cargo.toml` (F-68 cluster C, #895: "one connection's panic can't take down another's") does not apply; the whole database process dies. Tests only cover shallow inputs (`validate/tests/validate_tests.rs:186-195`).
- Suggested fix: thread a `depth: usize` through `TextFormatParser` (increment in `value()`, return `false` past a sane limit such as serde_json's 128), and add a TDD red test with deep nesting asserting `is_json` returns `false` (not a crash) for depth ≫ 128.

### 2. Unbounded-allocation scalar paths: `strings/repeat`, `strings/pad_left`/`pad_right`, `gen/random_bytes`
- File:line: `crates/shamir-funclib/src/strings.rs:229-242` (`s.repeat(n as usize)`), `strings.rs:406` (`std::iter::repeat_n(ch, target - cur)`), `crates/shamir-funclib/src/gen.rs:70-78` (`vec![0u8; n as usize]`)
- Severity: high
- Issue: each takes an `i64` length argument, rejects only negatives, and then allocates `arg` bytes with no upper bound. Huge-but-valid `i64` values hit `String::repeat`'s `capacity overflow` panic or an allocator OOM-abort.
- Failure scenario: `SELECT strings/repeat('a', 100000000000)` (or `gen/random_bytes(9223372036854775807)`, or `strings/pad_left('x', 1000000000000, 'a')`) → panic/abort from a pure `fn(&[QueryValue]) -> ScalarResult` that promised an error, not a crash. The crate's own crypto module caps the analogous input (`A2_MAX_LENGTH = 256`, `crypto.rs:62`), so the omission is an inconsistency, not a policy.
- Suggested fix: add a per-call result-size ceiling (e.g. a few MiB, mirroring the argon2 style: `const MAX_RESULT_BYTES: i64`) enforced with `return Err(ScalarError::new("out_of_range"))` before allocating in all three paths; add `#[test]`s for `n = i64::MAX` asserting the error code.

### 3. `UserScalarLayer::register` discards scc's `Err` — documented "(or replace)" silently never replaces
- File:line: `crates/shamir-funclib/src/scalar_resolver.rs:39-42`
- Severity: high
- Issue: `let _ = self.fns.insert_sync(name.into(), entry);` swallows the return of scc's `insert_sync`, which returns `Err((K, V))` when the key already exists (it does **not** overwrite). The doc comment says "Register (or replace) a user scalar under `name`" — that contract is false. The workspace's own code proves the semantics: `shamir-engine/src/repo/repo_instance.rs:1724` matches `if let Err((_, attempted)) = ...insert_sync(...)`, and `shamir-tx/src/tx_context.rs:731-733` explicitly comments that the `Err` on duplicate is being discarded (there, deliberately idempotent — here, silently wrong). Contrast `ScalarRegistry::register` (`registry.rs:129-136`, IndexMap-backed, genuinely last-wins) and `AggRegistry::register` (`agg.rs:72-75`, "last-wins on collision") — the two registries disagree.
- Failure scenario: an embedder re-registers a user scalar to redefine it (`CREATE OR REPLACE FUNCTION`-style flow); scc returns `Err`, the `let _` eats it, and the **old** implementation keeps executing with no error, no log, no test failure (there are no tests — see finding 9).
- Suggested fix: honour the documented contract: on `Err((k, v))` fall through to `update_sync(&k, |_, e| *e = v)` (or `remove_sync` + re-`insert_sync`); if replace is not intended, fix the doc, comment the discard like `tx_context.rs` does, and add a re-registration test pinning whichever semantics is chosen.

### 4. `rust_decimal` arithmetic panics on overflow in agg and array reductions
- File:line: `crates/shamir-funclib/src/agg.rs:286` (`SumAgg: self.acc += to_dec(v)?`), `agg.rs:322` (`AvgAgg`), `agg.rs:512-517` (`compute_variance`: `sum::<Decimal>()`, `(*x - mean) * (*x - mean)`, `sum_sq / n`), `agg.rs:852` (`RangeAgg`: `hi - lo`), `crates/shamir-funclib/src/arrays.rs:274-279` (`reduce` Sum/Avg: `acc +=` / `acc /=`)
- Severity: medium
- Issue: rust_decimal's `Add`/`Sub`/`Mul`/`Div` impls are `checked_*(...).expect(...)` — they **panic** on overflow (independent of `overflow-checks` profile settings). These paths feed user-supplied `Dec` values (magnitude up to ~7.9e28) straight into unchecked operators, so the panic escapes a pure scalar/`Aggregator` that returns `Result`.
- Failure scenario: `SELECT arrays/sum(...)` over a list containing two `Dec` values near `Decimal::MAX`, or `sum`/`variance` aggregates over such a column → addition/multiplication overflow panics the executing task. Unwinding contains it at the connection boundary (root `Cargo.toml` `panic = "unwind"`), but it still violates the crate's own `ScalarError` contract and differs from `math/pow`/`exp`, which carefully map non-finite results to `"domain"`.
- Suggested fix: use `checked_add`/`checked_sub`/`checked_mul`/`checked_div` (or `try_sum`) at these six sites and map `None` to `ScalarError::new("out_of_range")`; add an overflow regression test (e.g. `sum` of `[Dec(Decimal::MAX), Dec(Decimal::MAX)]` asserting the error code).

### 5. `datetime/age` performs unchecked `i64` subtraction — inconsistent with the file's own checked discipline
- File:line: `crates/shamir-funclib/src/datetime.rs:76-79`
- Severity: medium
- Issue: `Ok(v_int((Utc::now().timestamp_millis() - then) / 1000))` — `then` is an unvalidated `i64`; `now_ms - i64::MIN` overflows. Every sibling in the same file is checked (`from_epoch_s` `checked_mul`, `add_secs`/`add_days` `checked_add_signed`, `diff_secs` `checked_sub`), and `datetime/tests/datetime_tests.rs:177-188` explicitly asserts `diff_secs` "must return out_of_range, not panic" — `age` is the lone miss and has no such test.
- Failure scenario: `SELECT datetime/age(-9223372036854775808)` → debug builds (all lib tests) panic on overflow; release builds silently wrap, returning a garbage "age".
- Suggested fix: `then.checked_sub(...)` symmetric to `diff_secs` — `now_ms.checked_sub(then).ok_or_else(|| ScalarError::new("out_of_range"))? / 1000` — plus the mirrored non-panic test.

### 6. `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context
- File:line: `crates/shamir-funclib/src/registry.rs:19-37` (struct), e.g. `strings.rs:427` `"bad_regex"` vs `validate.rs:342` `"bad_pattern"`, `crypto.rs:212/229/289` `"bad_params"`/`"compute"`/`"bad_key"`
- Severity: medium
- Issue: per `CLAUDE.md` ("thiserror for library error enums"), the natural shape here would be a closed enum of codes; instead `ScalarError` is a `String`-holding struct and every site hand-types its code as an inline literal. The registry's own docs promise a stable, frontend-localisable code set, but the set is nowhere enumerated, codes for the *same* failure class have already drifted (`bad_regex` vs `bad_pattern` for an invalid user regex), and several codes (`compute`, `bad_params`, `bad_key`) appear in no module-doc error list. Errors also carry no payload (no arg index / offending type), so the frontend can only ever print a generic localised sentence. Every error allocates a fresh `String` on a library hot path where a `&'static str` (or small enum) would not.
- Failure scenario: a frontend keyed on `"bad_regex"` silently falls back to a generic error for `validate/matches` failures; a typo in a future code (`"tyoe_mismatch"`) compiles and ships undetected.
- Suggested fix: keep the wire shape (code string) but back it with a `#[non_exhaustive] pub enum ScalarErrorCode` + `thiserror` `Display`, or at minimum a `pub mod codes` of `&'static str` constants used at every call site; dedupe `bad_regex`/`bad_pattern` consciously and document the full set in the registry module docs.

### 7. Argon2id semaphore: poison-propagating `expect`s, and the in-flight counter leaks on panic
- File:line: `crates/shamir-funclib/src/crypto.rs:139` (`lock.lock().expect("semaphore mutex poisoned")`), `crypto.rs:141` (`cvar.wait(guard).expect(...)`), `crypto.rs:225-230` (manual `fetch_add`/`fetch_sub` pair)
- Severity: low
- Issue: (a) the semaphore's critical section contains no panicking code, so poisoning is near-unreachable — but if it ever poisons, every subsequent `argon2id()` call on every connection panics forever, and this contradicts the poison-tolerant idiom the crate itself uses ten metres away (`strings.rs:418-423` `cache.lock().unwrap_or_else(|e| e.into_inner())` with an explanatory comment). (b) `A2_IN_FLIGHT.fetch_sub(1)` sits *after* the fallible KDF call without a guard: if `hash_password_into` ever panics (or the code between add/sub is later extended), the counter leaks upward permanently; the RAII permit is fine, the counter is not.
- Failure scenario: mostly theoretical; (b) is a latent tripwire for future edits inside the add/sub window.
- Suggested fix: (a) adopt the `unwrap_or_else(|e| e.into_inner())` pattern with the same rationale comment; (b) fold the counter into the existing `SemaphorePermit` RAII (inc in `acquire`, dec in `Drop`) so it shares the permit's panic-safety.

### 8. Canonical-hash path silently degrades serialization failures to empty key bytes
- File:line: `crates/shamir-funclib/src/canonical.rs:180-188` (`serialise_key`: `rmp_serde::to_vec(key).unwrap_or_default()`), `canonical.rs:192-196` (`key_is_prev_hash`: `unwrap_or(false)`)
- Severity: low
- Issue: a key that fails to serialise becomes `Vec::new()`. Today both are unreachable for the actual instantiations (`String` keys and interned ids serialise infallibly), but the generic bound is `K: Serialize`, so a future key type that *can* fail would silently collapse all such keys to one byte string: distinct map entries would then sort as duplicates and two different records could hash identically — a silent CAS-integrity break on exactly the path whose module doc promises "any change to data changes the hash".
- Failure scenario: latent; triggers only when the generic is instantiated with a fallible `Serialize` key.
- Suggested fix: return `Result<Vec<u8>, ScalarError>` from `serialise_key` (or `debug_assert!` non-empty output) so a future key type fails loudly instead of colliding; mirror the decision in `key_is_prev_hash`.

### 9. `ScalarResolver` / `UserScalarLayer` have zero tests — their error paths are unverified
- File:line: `crates/shamir-funclib/src/scalar_resolver.rs` (entire module; no `tests/` dir, no `#[cfg(test)]` — every other category module has one)
- Severity: low
- Issue: the 2-layer dispatch — the piece wired into the engine's `FilterContext` — is the only non-trivial module with no test file. Untested: `unknown_function` from the fallback layer, user-layer shadowing priority, arity gating via `dispatch_entry`, and (directly enabling finding 3) re-registration over an existing name.
- Failure scenario: the replace-vs-insert contract bug of finding 3 shipped precisely because no test could go red; any future refactor of `builtins_only()`'s `OnceLock` or the shadowing order is likewise unguarded.
- Suggested fix: add `src/scalar_resolver/tests/{mod.rs,resolver_tests.rs}` per the test-organisation convention: shadowing-priority test, `unknown_function` test, arity-propagation test, and a re-registration test asserting the intended semantics.

### 10. Unannotated panicky/fallible primitives in library code
- File:line: `crates/shamir-funclib/src/datetime.rs:153` (`.and_hms_opt(0, 0, 0).unwrap()`), `crates/shamir-funclib/src/agg.rs:472` (`variance.to_f64().unwrap_or(f64::NAN)`), `crates/shamir-funclib/src/agg.rs:268` (`_ => unreachable!()` in `arrays`-style reduce, `arrays.rs:268`)
- Severity: nit
- Issue: the `.unwrap()` is safe only by the invariant "0:0:0 is always a valid time" — per the project's own convention that deserves an inline `expect("...")` naming the invariant, since nothing else in the file uses `unwrap`. The `unwrap_or(f64::NAN)` in `StddevAgg::finalize` masks a `Decimal→f64` conversion failure into NaN, which only *later* surfaces as `"out_of_range"` from `from_f64_retain` — correct outcome, needlessly obscured failure chain (map the `None` to `"out_of_range"` directly instead).
- Failure scenario: none today; both are readability/audit-noise issues on error paths.
- Suggested fix: `expect("0:0:0 is valid for any NaiveDate")`; propagate the `to_f64` failure as `"out_of_range"` immediately.

</details>
