<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Finite wire fixtures are meaningful, but boundary/error registrations remain incomplete. Source confirms numeric and fallback defects while disproving universal Pending livelock and compilation-nowhere claims.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 7 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — No tests for http.rs, params.rs, db.rs mapping logic, or `__rt` ABI helpers (TDD protocol not followed for most of the crate)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Registered utility success tests exist, but getter failures, malformed HTTP, native packing, and DB result arms lack local oracles. A Str-bytes regression would escape them.

Evidence: [crates/shamir-sdk/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/mod.rs#L1); [crates/shamir-sdk/src/tests/value_tests.rs:400](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/value_tests.rs#L400); [crates/shamir-sdk/src/host_imports.rs:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L235).

Grouping/duplicate: [SUMMARY.md#1.1](SUMMARY.md#review-1-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Fail-silent msgpack decode fallbacks in the wasm host imports

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Malformed nonzero query bytes become an empty success; getters become absent. Insert/HTTP fail later, losing the source. This needs incompatible/malformed host data, not direct HTTP envelope control.

Evidence: [crates/shamir-sdk/src/host_imports.rs:183](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L183); [crates/shamir-sdk/src/http.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L27).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `Ctx::call` accepts non-map `args`, but the host traps on them -- contract enforced only on the far side, doc says "should"

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Int arguments are rejected by the host as required-map failures. Enforcement is correct; the residual is incomplete should/trap documentation, not an authorization bypass.

Evidence: [crates/shamir-sdk/src/context.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/context.rs#L80); [crates/shamir-wasm-host/src/wasm/host_call.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L93).

Grouping/duplicate: [SUMMARY.md#1.3](SUMMARY.md#review-1-3). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — `__rt::block_on` is a guaranteed livelock for any future that yields `Pending`; its justifying doc comment is stale

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

A Pending-once/Ready-next future completes because polling repeats. Stale comments and unresolved-future polling remain separately open.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50).

Grouping/duplicate: [SUMMARY.md#2.1](SUMMARY.md#review-2-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — No compile-pass tests for `#[function]` and `#[validator]`; flagship doc examples never compiled anywhere

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

SDK integration targets cover only scalar/procedure. Host function compilation has an exact result oracle but may skip; standalone examples alone do not establish mandatory coverage.

Evidence: [crates/shamir-sdk/tests/procedure_compile_pass.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/tests/procedure_compile_pass.rs#L7); [crates/shamir-sdk/Cargo.toml:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/Cargo.toml#L28); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19).

Grouping/duplicate: [SUMMARY.md#1.5](SUMMARY.md#review-1-5). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — `HttpResponse::from_value` silently drops data and truncates status

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Status narrowing and wrong-type header/body dropping remain. Shipped host fields are normalized; exact malformed-value rejection is the needed oracle.

Evidence: [crates/shamir-sdk/src/http.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L134); [crates/shamir-sdk/src/http.rs:150](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L150); [crates/shamir-wasm-host/src/wasm/host_http.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L93).

Grouping/duplicate: [SUMMARY.md#1.6](SUMMARY.md#review-1-6). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — `Value` edge cases: `visit_u64` wrap-around and NaN/Infinity untestable under `PartialEq`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

External uint64 maximum wraps in SDK decoding. NaN is non-reflexive, infinity is comparable; neither edge has fixtures. Host Big promotion does not alter SDK visit_u64.

Evidence: [crates/shamir-sdk/src/value.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L98); [crates/shamir-sdk/src/tests/value_tests.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/value_tests.rs#L83); [crates/shamir-types/src/types/value.rs:142](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L142).

Grouping/duplicate: [SUMMARY.md#1.7](SUMMARY.md#review-1-7). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Convention nits

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The stale path and missing builder rationale remain. Private helper placement is permitted closely-coupled organization; message-only Error is the separately tracked category issue, not proof that thiserror is required for correctness.

Evidence: [crates/shamir-sdk/src/value.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L9); [crates/shamir-sdk/src/params.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/params.rs#L96); [crates/shamir-sdk/src/db.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L94); [crates/shamir-sdk/src/error.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/error.rs#L7).

## Evidence and recipe corrections

- Absent assertions cannot establish whether historical Red/Green development was followed.
- Table::get returns Option, not Ok(None); its own documentation also contains that inaccurate phrase.
- Compile-pass integration targets are excluded by the default --lib runner.
- The existing conformance helper compares genuinely separate host/guest encoders, but its ordinary map fixtures do not cover recognized-prefix keys or duplicates.
- A no-op waker does not prevent later polls from observing progress; production fuel contradicts an indefinite default WASM loop.

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
