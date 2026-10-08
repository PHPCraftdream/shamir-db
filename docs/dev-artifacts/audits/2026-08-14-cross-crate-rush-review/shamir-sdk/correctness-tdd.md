<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Substantial branch-coverage and boundary-hardening gaps remain. The report overstates zero coverage, universal Pending livelock, and absence of compilation elsewhere.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 7 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — No tests for http.rs, params.rs, db.rs mapping logic, or `__rt` ABI helpers (TDD protocol not followed for most of the crate)

Status: `confirmed-open`. Current risk: `medium`.

Only value_tests and validation_tests are registered. HTTP parsing, getter failures, packing, and DB result arms remain uncovered locally; non-WASM imports still panic without an injection seam. However, i64/str getters, decode_params success, encode_value success, and block_on Ready already have tests.

Evidence: [crates/shamir-sdk/src/lib.rs:62](../../../../../crates/shamir-sdk/src/lib.rs#L62); [crates/shamir-sdk/src/tests/mod.rs:1](../../../../../crates/shamir-sdk/src/tests/mod.rs#L1); [crates/shamir-sdk/src/tests/value_tests.rs:400](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L400); [crates/shamir-sdk/src/host_imports.rs:235](../../../../../crates/shamir-sdk/src/host_imports.rs#L235); [crates/shamir-sdk/src/db.rs:86](../../../../../crates/shamir-sdk/src/db.rs#L86).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Fail-silent msgpack decode fallbacks in the wasm host imports

Status: `confirmed-open`. Current risk: `medium`.

All seven fallback sites remain. Getters conflate corruption with absence, query with no rows, and call with Null. Insert and HTTP subsequently return errors for fallback Null, so not every public API returns success. Reachability requires malformed/incompatible host bytes, not directly an untrusted network response.

Evidence: [crates/shamir-sdk/src/host_imports.rs:97](../../../../../crates/shamir-sdk/src/host_imports.rs#L97); [crates/shamir-sdk/src/host_imports.rs:131](../../../../../crates/shamir-sdk/src/host_imports.rs#L131); [crates/shamir-sdk/src/host_imports.rs:183](../../../../../crates/shamir-sdk/src/host_imports.rs#L183); [crates/shamir-sdk/src/host_imports.rs:207](../../../../../crates/shamir-sdk/src/host_imports.rs#L207); [crates/shamir-sdk/src/http.rs:27](../../../../../crates/shamir-sdk/src/http.rs#L27).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `Ctx::call` accepts non-map `args`, but the host traps on them -- contract enforced only on the far side, doc says "should"

Status: `confirmed-open`. Current risk: `medium`.

Ctx::call still forwards arbitrary Value and documents only should. The host requires Params::from_value to receive a map and turns rejection into an import error; the caller receives a Compute trap rather than a catchable SDK error.

Evidence: [crates/shamir-sdk/src/context.rs:80](../../../../../crates/shamir-sdk/src/context.rs#L80); [crates/shamir-sdk/src/context.rs:86](../../../../../crates/shamir-sdk/src/context.rs#L86); [crates/shamir-wasm-host/src/wasm/host_call.rs:91](../../../../../crates/shamir-wasm-host/src/wasm/host_call.rs#L91); [crates/shamir-wasm-host/src/params.rs:30](../../../../../crates/shamir-wasm-host/src/params.rs#L30); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: `SUMMARY.md#1.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `__rt::block_on` is a guaranteed livelock for any future that yields `Pending`; its justifying doc comment is stale

Status: `refuted`. Current risk: —.

The universal claim is false: the loop polls again after Pending and returns on a subsequent Ready, even with a no-op waker. The residual defect is continuous CPU consumption for an unresolved future, plus stale documentation; those remain open under concurrency finding 1.

Evidence: [crates/shamir-sdk/src/__rt.rs:34](../../../../../crates/shamir-sdk/src/__rt.rs#L34); [crates/shamir-sdk/src/__rt.rs:50](../../../../../crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-sdk/src/__rt.rs:52](../../../../../crates/shamir-sdk/src/__rt.rs#L52); [crates/shamir-sdk/src/__rt.rs:57](../../../../../crates/shamir-sdk/src/__rt.rs#L57).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — No compile-pass tests for `#[function]` and `#[validator]`; flagship doc examples never compiled anywhere

Status: `confirmed-open`. Current risk: `low`.

SDK-local function/validator compile-pass targets are still absent and doctests remain disabled. The anywhere claim is false: a registered wasm-host test compiles/invokes the flagship double shape, while standalone function and validator example crates exist. The host test can skip on unavailable toolchain.

Evidence: [crates/shamir-sdk/tests/scalar_compile_pass.rs:7](../../../../../crates/shamir-sdk/tests/scalar_compile_pass.rs#L7); [crates/shamir-sdk/tests/procedure_compile_pass.rs:7](../../../../../crates/shamir-sdk/tests/procedure_compile_pass.rs#L7); [crates/shamir-sdk/Cargo.toml:28](../../../../../crates/shamir-sdk/Cargo.toml#L28); [crates/shamir-wasm-host/src/tests/mod.rs:2](../../../../../crates/shamir-wasm-host/src/tests/mod.rs#L2); [crates/shamir-wasm-host/src/tests/compile_tests.rs:8](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L8); [examples/fn-validator/src/lib.rs:15](../../../../../examples/fn-validator/src/lib.rs#L15).

Grouping/duplicate: `SUMMARY.md#1.5`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `HttpResponse::from_value` silently drops data and truncates status

Status: `confirmed-open`. Current risk: `low`.

Unchecked i64-to-u16 casting, filter_map removal of non-string header values, and empty-body fallback remain. The normal host encoder emits a u16 status, string headers, and Bin body, so malformed-boundary handling is the issue, not demonstrated remote control of these types.

Evidence: [crates/shamir-sdk/src/http.rs:134](../../../../../crates/shamir-sdk/src/http.rs#L134); [crates/shamir-sdk/src/http.rs:139](../../../../../crates/shamir-sdk/src/http.rs#L139); [crates/shamir-sdk/src/http.rs:150](../../../../../crates/shamir-sdk/src/http.rs#L150); [crates/shamir-wasm-host/src/wasm/host_http.rs:86](../../../../../crates/shamir-wasm-host/src/wasm/host_http.rs#L86).

Grouping/duplicate: `SUMMARY.md#1.6`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — `Value` edge cases: `visit_u64` wrap-around and NaN/Infinity untestable under `PartialEq`

Status: `confirmed-open`. Current risk: `low`.

visit_u64 still wraps values above i64::MAX, and non-finite float tests are absent. NaN requires bit/byte assertions because guest PartialEq is non-reflexive; infinity does not. Host u64 promotion was fixed independently, but the SDK visitor remains unchanged.

Evidence: [crates/shamir-sdk/src/value.rs:26](../../../../../crates/shamir-sdk/src/value.rs#L26); [crates/shamir-sdk/src/value.rs:97](../../../../../crates/shamir-sdk/src/value.rs#L97); [crates/shamir-sdk/src/tests/value_tests.rs:14](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L14); [crates/shamir-sdk/src/tests/value_tests.rs:83](../../../../../crates/shamir-sdk/src/tests/value_tests.rs#L83); [crates/shamir-types/src/types/value.rs:142](../../../../../crates/shamir-types/src/types/value.rs#L142).

Grouping/duplicate: `SUMMARY.md#1.7`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Convention nits

Status: `confirmed-open`. Current risk: `nit`.

All four cited source facts remain: stale test path, type_name implemented in params.rs, raw-filter docs without builder-exception rationale, and message-only Error. Private type_name placement is cosmetic, not independently a demonstrated structural violation; Error taxonomy is tracked separately.

Evidence: [crates/shamir-sdk/src/value.rs:9](../../../../../crates/shamir-sdk/src/value.rs#L9); [crates/shamir-sdk/src/params.rs:96](../../../../../crates/shamir-sdk/src/params.rs#L96); [crates/shamir-sdk/src/db.rs:94](../../../../../crates/shamir-sdk/src/db.rs#L94); [crates/shamir-sdk/src/error.rs:7](../../../../../crates/shamir-sdk/src/error.rs#L7).

## Corrections and qualified non-findings

- Replace zero-tests wording with missing branch coverage; existing encode_value, decode_params, i64/str, and Ready-path tests are wired. Source does not establish the historical Red/Green sequence.
- Table::get returns Option<Value>, not Result or Ok(None); its own Ok(None) documentation is inaccurate.
- A no-op waker does not prevent progress under continuous polling. Pending-once futures can complete; unresolved futures still burn CPU.
- WASM spinning is bounded by fuel, memory policy, and epoch/deadline mechanisms; it is not an indefinitely wedged production worker under default limits.
- Function compilation exists outside this crate; examples alone are not proof of execution or a mandatory compile-test target.
- Infinity is comparable with PartialEq. Neither the NaN coverage gap nor idiomatic float PartialEq alone proves incorrect serialization.
- High severity for missing tests is not source-proven runtime impact.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk -- Correctness & TDD-coverage

## Summary

The crate's two test-bearing modules (`src/tests/value_tests.rs`, `src/tests/validation_tests.rs`) are genuinely strong -- byte-identity wire-conformance tests against the host `QueryValue` are exactly the right shape. However, the rest of the crate's API surface (`http.rs`, `params.rs`, `db.rs` response-mapping, `__rt.rs` ABI helpers) ships with zero tests despite being pure, host-testable logic -- a direct miss against CLAUDE.md's Red/Green/Refactor protocol -- and the wasm-side host-import decoders fail *silent* (garbage reply indistinguishable from "no rows" / "no record" / "callee returned null"). Two cross-crate contracts (`Ctx::call` map-only args, `block_on` never-Pending assumption) are enforced only on the far side of the ABI and are understated or stale in the SDK's own docs.

## Findings

### 1. No tests for http.rs, params.rs, db.rs mapping logic, or `__rt` ABI helpers (TDD protocol not followed for most of the crate)
- File:line: `crates/shamir-sdk/src/http.rs:24-44,98-160`; `src/params.rs:26-93`; `src/db.rs:86-109`; `src/__rt.rs:11-30`; `src/tests/` (only `value_tests.rs`, `validation_tests.rs` exist)
- Severity: high
- Issue: CLAUDE.md's development protocol is Red (failing test) -> Green -> Refactor. Every module listed above contains non-trivial pure logic that is fully host-testable, yet has no test at all:
  - `decode_fetch_envelope` (both `[true, map]` and `[false, msg]` branches, wrong-shape/wrong-flag branches).
  - `HttpRequest::to_value` -- the exact wire map (`method`/`url`/`headers`/`body` with `Bin` body) the host's `decode_http_request` depends on; no test pins it.
  - `HttpResponse::from_value` -- including the three silent-loss branches (see finding 6).
  - All `Params` typed getters and their error paths (`i64`/`f64`/`str`/`bytes`/`bool`; `bytes`'s Str fallback is untested).
  - `__rt::decode_params` non-map fallback (`-> Params::new()`), `__rt::leak_result` packed `(ptr<<32)|len` convention (the single most fragile cross-crate invariant; the host unpacks it at `shamir-wasm-host/src/wasm/wasm_function.rs:567-568`, but the packing side is never tested), and `encode_value`'s empty-vec fallback.
  - `Table::insert`'s `Value::Null -> Err` and `Table::query`'s non-List -> `Err` mapping are fused to the panicking non-wasm stubs, so they are *untestable by construction* -- there is no seam (e.g. an internal `fn`-parameter or trait) to substitute a fake import.
- Failure scenario: a getter regression (e.g. someone drops `Value::Str` acceptance from `bytes()`), a wire-shape drift in `to_value`, or a packing-convention change on either side of `leak_result` ships green; only host-crate or e2e tests could catch it.
- Suggested fix: add `src/tests/http_tests.rs`, `params_tests.rs`, `rt_tests.rs` (following the documented `tests/` directory layout) covering the branches above; introduce a minimal internal seam for `db.rs`'s import calls so the Null/non-List mappings get red-test coverage.

### 2. Fail-silent msgpack decode fallbacks in the wasm host imports
- File:line: `crates/shamir-sdk/src/host_imports.rs:97,106,131,146,162,183,207`
- Severity: medium
- Issue: every reply decode uses `.ok()` / `.unwrap_or(Value::Null)` / `.unwrap_or(Value::List(vec![]))`. A truncated or malformed host reply is silently converted into `None` / `Value::Null` / an empty list. Downstream: `Table::query` then returns `Ok(vec![])` (db.rs:103-104) -- corrupt reply indistinguishable from "no matching rows"; `Table::get` returns `Ok(None)`; `Ctx::call` returns `Value::Null`.
- Failure scenario: today unreachable from the shipped host (`host_db_*` / `host_call` / `write_value_to_guest` always write exactly what they encoded or trap), but any future host-side change that writes a variant the guest visitor rejects becomes a *silent wrong answer* on a read path instead of a loud failure. This is the fail-open direction on the crate's core data path.
- Suggested fix: propagate decode failure to the APIs that already return `Result` (`Table::query`, `Table::insert`, `http_fetch`) instead of substituting empty values; for `Option`-returning `get`/`batch_get`/`global_get`, at minimum document that decode failure reads as "absent", or trap.

### 3. `Ctx::call` accepts non-map `args`, but the host traps on them -- contract enforced only on the far side, doc says "should"
- File:line: `crates/shamir-sdk/src/context.rs:78-88` (doc: "`args` **should** be a `Value::Map`"); `src/host_imports.rs:121-132` (no validation); host side `shamir-wasm-host/src/wasm/host_call.rs:91-94` (`Params::from_value(... "call: params not a map")` -> trap)
- Severity: medium
- Issue: the guest API accepts any `Value` and returns `Value` (not `Result`), doing no shape check. If a guest passes e.g. `Value::Int(5)` or a `Value::List`, the host import traps and the *entire calling function* dies with an uncatchable `FunctionError::Compute` at runtime. Nothing in the guest, and no test, pins this contract; "should" invites the bug.
- Failure scenario: `ctx.call("double", Value::Int(5))` compiles, passes every guest-side test (there are none), and only detonates in production inside the WASM runtime.
- Suggested fix: either validate in `Ctx::call` (fail fast with a clear message at the call site) or strengthen the doc to "MUST be a `Value::Map`" with a `/// # Panics/Traps` section; add a doc-level example to `prelude.rs`. Ideally both.

### 4. `__rt::block_on` is a guaranteed livelock for any future that yields `Pending`; its justifying doc comment is stale
- File:line: `crates/shamir-sdk/src/__rt.rs:32-61` (no-op waker + `spin_loop`, comment "pure functions (the only kind this slice supports) are `Ready` on the first poll")
- Severity: medium
- Issue: the stale premise no longer holds -- all four attribute macros (`scalar`, `function`, `procedure`, `validator`; see `shamir-sdk-macros/src/lib.rs:144,264,391,556`) route through `block_on`, and user code inside a `#[function]`/`#[procedure]` may `.await` anything (a channel receiver, a hand-rolled `Pending` future, a timer). With the no-op waker, a `Pending` future is *never* woken: the guest busy-spins forever (bounded only by host fuel/timeout), producing a misleading trap.
- Failure scenario: a guest author writes `ctx.call(...)` inside a loop awaiting an `mpsc` for throttling -- infinite spin, fuel exhaustion, confusing `Compute` error, 100% CPU in the instance.
- Suggested fix: at minimum, update the comment to state the real invariant ("guest code must not await futures that are not immediately ready; host imports are synchronous FFI and never yield") and surface that warning in `context.rs`/`prelude.rs` docs where users actually read. A test cannot pin a spin (it hangs), which is itself a TDD gap -- document it as a known-unsupported pattern next to the macro docs.

### 5. No compile-pass tests for `#[function]` and `#[validator]`; flagship doc examples never compiled anywhere
- File:line: `crates/shamir-sdk/tests/` (only `scalar_compile_pass.rs`, `procedure_compile_pass.rs`); `src/lib.rs:5-13`, `src/prelude.rs:21-38` (all examples ` ```ignore `; `doctest = false` in Cargo.toml:28)
- Severity: medium
- Issue: half the exported macro surface (`function` -- the crate's front-door example in the lib doc -- and `validator`) has no expansion smoke test, unlike `procedure`/`scalar`. Combined with `doctest = false` + `ignore` fences, none of the documented usage in `lib.rs`/`prelude.rs`/`context.rs` is compile-checked by any target of this crate.
- Failure scenario: a signature change in the macro (e.g. arg-count check, return-type normalisation in `is_result_value_return`) breaks every real `#[function]` guest while this crate's suite stays green.
- Suggested fix: add `tests/function_compile_pass.rs` and `tests/validator_compile_pass.rs` mirroring the two existing files (separate integration-test crates so the `#[no_mangle]` symbols don't collide, per the existing files' own rationale). Optionally a `ui`-style compile-fail test for `#[scalar] fn x(ctx: Ctx, ...)`.

### 6. `HttpResponse::from_value` silently drops data and truncates status
- File:line: `crates/shamir-sdk/src/http.rs:130-153`
- Severity: low
- Issue: three fail-silent branches: (a) non-`Int` status -> misleading "missing status field" error; (b) non-`Str` header values silently `filter_map`ped away; (c) non-`Bin` body (e.g. `Str`) silently becomes an empty body. Also `Value::Int(n) => Some(*n as u16)` truncates any out-of-range value (e.g. 70_000 -> 4_464) instead of erroring.
- Failure scenario: currently unreachable from the shipped host (`encode_http_response` in `shamir-wasm-host/src/wasm/host_http.rs:86-97` always writes `Int` status in range, `Str` headers, `Bin` body), but any envelope evolution turns into silent data loss in guest code; and these branches are exactly the untested ones (finding 1).
- Suggested fix: make (b)/(c) either strict (`Err`) or explicitly documented as lossy; replace `as u16` with `u16::try_from(n)` mapping to `Err`.

### 7. `Value` edge cases: `visit_u64` wrap-around and NaN/Infinity untestable under `PartialEq`
- File:line: `crates/shamir-sdk/src/value.rs:97-99` (`Ok(Value::Int(v as i64))`), `:26` (`#[derive(PartialEq)]` over `F64(f64)`)
- Severity: low
- Issue: (a) a msgpack `u64 > i64::MAX` silently wraps negative (unreachable from the host today -- `QueryValue::Int` is `i64` -- but reachable if a guest ever decodes third-party msgpack, e.g. an HTTP response body decoded as `Value`); (b) `Value::F64(NaN) != Value::F64(NaN)`, so the otherwise-excellent `assert_bidirectional` conformance harness structurally cannot cover NaN/Inf F64 -- the "every shared variant" claim (value.rs doc) is untested for exactly those inputs.
- Failure scenario: a guest that decodes an external payload stores a silently-wrapped id; a NaN round-trip regression would be invisible to the conformance suite.
- Suggested fix: for (a) document the wrap or use a checked conversion; for (b) add a NaN/Inf test that compares *bytes* (and decoded bit patterns) instead of relying on `PartialEq`.

### 8. Convention nits
- File:line: `src/value.rs:9`; `src/params.rs:96-109`; `src/db.rs:94-97`; `src/error.rs:6-23`
- Severity: nit
- Issue:
  - Stale doc path: `value.rs:9` points to conformance tests at "`tests/value_tests.rs`"; they live at `src/tests/value_tests.rs`.
  - `impl Value { fn type_name }` is defined in `params.rs` rather than beside `Value` -- bends the "one file = one primary export" rule (it is private, so cosmetic).
  - `Table::query`'s doc invites hand-assembled filter `Value::Map`s; CLAUDE.md's "builder only" rule asks for a one-line "why no builder" comment where the builder does not apply -- `db.rs` has none (the `query-builder` feature is the sanctioned path; a pointer in the doc would do).
  - `Error` is a hand-rolled struct; CLAUDE.md prefers `thiserror` for library errors. Defensible here (single variant, guest dependency minimisation), noted for the record only.
- Suggested fix: one-line doc/comment updates; no functional change.


</details>
