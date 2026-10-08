<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk-macros — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Malformed decoding and unchecked guest ABI preconditions remain robustness concerns under a trusted-host boundary. Absolute coverage and structural-purity guarantees are false; trap-text disclosure is unverified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 4 | 0 | 0 | 1 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `#[validator]` silently coerces undecodable params into `record = Null`, `old_record = None`

Status: `confirmed-open`. Current risk: `medium`.

decode_params still converts malformed/non-map input into empty Params, and validator extraction still substitutes Null/None. The normal engine adapter explicitly supplies both keys, so UPDATE-as-INSERT requires host corruption, schema drift, or an alternative caller; ordinary record input alone does not establish this bypass.

Evidence: [crates/shamir-sdk/src/__rt.rs:11](../../../../../crates/shamir-sdk/src/__rt.rs#L11); [crates/shamir-sdk-macros/src/lib.rs:129](../../../../../crates/shamir-sdk-macros/src/lib.rs#L129); [crates/shamir-sdk-macros/src/lib.rs:135](../../../../../crates/shamir-sdk-macros/src/lib.rs#L135); [crates/shamir-engine/src/validator/wasm_record_validator.rs:60](../../../../../crates/shamir-engine/src/validator/wasm_record_validator.rs#L60); [crates/shamir-engine/src/validator/wasm_record_validator.rs:65](../../../../../crates/shamir-engine/src/validator/wasm_record_validator.rs#L65).

<a id="review-2"></a>

### Claim 2 — `unsafe` slice construction from unverified `(ptr, len)`; negative `len` is immediate UB

Status: `confirmed-open`. Current risk: `low`.

All four exports still construct slices without checking signed lengths or pointer validity. Negative lengths violate Rust slice preconditions. The standard host checks input length and memory range before calling; guest sandbox isolation does not make Rust UB valid or prove one deterministic trap outcome.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:122](../../../../../crates/shamir-sdk-macros/src/lib.rs#L122); [crates/shamir-sdk-macros/src/lib.rs:255](../../../../../crates/shamir-sdk-macros/src/lib.rs#L255); [crates/shamir-sdk-macros/src/lib.rs:383](../../../../../crates/shamir-sdk-macros/src/lib.rs#L383); [crates/shamir-sdk-macros/src/lib.rs:549](../../../../../crates/shamir-sdk-macros/src/lib.rs#L549); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:518](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L518); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:533](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L533).

<a id="review-3"></a>

### Claim 3 — No tests cover the two boundary-bearing macros or any runtime behavior of the generated ABI

Status: `refuted`. Current risk: —.

The registered compile_and_invoke_double test compiles #[function] through the SDK and invokes the resulting WASM ABI, asserting 42. This predates the review, not a fix. Validator, malformed-input, negative-length, and user-error generated-ABI coverage remain absent.

Evidence: [crates/shamir-wasm-host/src/tests/compile_tests.rs:11](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L11); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L19); [crates/shamir-wasm-host/src/tests/compile_tests.rs:35](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L35); [crates/shamir-wasm-host/src/tests/mod.rs:2](../../../../../crates/shamir-wasm-host/src/tests/mod.rs#L2); [crates/shamir-wasm-host/src/compile.rs:511](../../../../../crates/shamir-wasm-host/src/compile.rs#L511).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — User error strings passed verbatim into the trap channel

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

Passing e.to_string() to the panic helper is proven. Passing that panic text verbatim into a Wasmtime trap or exposing it to callers is not: the inspected path formats the runtime error, with no explicit panic-text transport. Conditional information disclosure needs target/runtime evidence.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:272](../../../../../crates/shamir-sdk-macros/src/lib.rs#L272); [crates/shamir-sdk/src/__rt.rs:64](../../../../../crates/shamir-sdk/src/__rt.rs#L64); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:739](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L739); [Cargo.lock:4740](../../../../../Cargo.lock#L4740).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — String-comparison return-type checks: `#[validator]`/`#[function]` reject valid spellings; all accept same-named foreign types

Status: `confirmed-open`. Current risk: `nit`.

False rejection of qualified SDK types remains. Bare imported foreign names can pass lexical checks but encounter concrete SDK wrapper type checking. Arbitrary qualified my_crate::Validation is not accepted by the validator's exact Validation comparison.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:66](../../../../../crates/shamir-sdk-macros/src/lib.rs#L66); [crates/shamir-sdk-macros/src/lib.rs:98](../../../../../crates/shamir-sdk-macros/src/lib.rs#L98); [crates/shamir-sdk-macros/src/lib.rs:197](../../../../../crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:411](../../../../../crates/shamir-sdk-macros/src/lib.rs#L411).

Grouping/duplicate: `api-wire-protocol.md#1`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — "Only one macro per crate" constraint documented but unenforced

Status: `confirmed-open`. Current risk: `nit`.

Targeted duplicate-entrypoint diagnostics remain absent. Duplicate fixed ABI symbols already prevent a valid multiple-entrypoint binary; no security bypass is established.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:15](../../../../../crates/shamir-sdk-macros/src/lib.rs#L15); [crates/shamir-sdk-macros/src/lib.rs:108](../../../../../crates/shamir-sdk-macros/src/lib.rs#L108); [crates/shamir-sdk-macros/src/lib.rs:238](../../../../../crates/shamir-sdk-macros/src/lib.rs#L238).

Grouping/duplicate: `correctness-tdd.md#4`. This row is not another independent defect.

## Corrections and qualified non-findings

- Refuted non-finding: lack of a Ctx parameter does not structurally guarantee scalar purity. Public construction reaches host-backed methods, and the linker does not select imports by macro kind: crates/shamir-sdk/src/context.rs:64; crates/shamir-wasm-host/src/wasm/wasm_function.rs:178.
- Confirmed non-findings: the macro file contains only its four generated unsafe slice blocks; allocator output is initialized; generated identifier construction uses the parsed function Ident; no cryptographic/authentication primitive exists here.
- Malformed decoding is a fail-quiet boundary defect, but whether it permits a write depends on the validator body. UPDATE bypass is not proven reachable through the normal adapter.
- Do not reject every nonempty encoding of an empty map: an empty parameter map is valid. Distinguish decode failure from valid emptiness.
- Two length-check lines do not eliminate every pointer/provenance/null precondition violation.
- Params::i64 formats a key and type name, not the offending value's contents: crates/shamir-sdk/src/params.rs:38.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk-macros -- Security & crypto boundary

## Summary

This crate contains no cryptographic primitives, auth, HMAC/SCRAM/TLS, or timing-sensitive code; its entire security surface is the proc-macro-generated WASM guest ABI (four `shamir_alloc`/`shamir_call` export pairs, one per macro flavor) and the crossing of host-supplied msgpack params into author code. Two boundary weaknesses: the `#[validator]` entry silently coerces undecodable/absent parameters into `record = Null`, `old_record = None` (fail-quiet at the write-gate boundary, with "UPDATE silently presenting as INSERT" as the concrete scenario), and the crate's only `unsafe` blocks construct slices from unverified `(ptr, len)` where a negative `len` is immediate UB — sandbox-contained, and a two-line guard would eliminate the class. Generated-ABI test coverage reaches only two of the four macros and no runtime behavior. No timing side-channels or injection vectors were identified.

## Findings

### 1. `#[validator]` silently coerces undecodable params into `record = Null`, `old_record = None`
**File:** `crates/shamir-sdk-macros/src/lib.rs:129-139` (generated inside `validator`'s `shamir_call`); enabling factor in `crates/shamir-sdk/src/__rt.rs:11-16`
**Severity:** medium

**Issue:** The generated extraction swallows both getter errors: `params.get("record") → Err(_) => Value::Null` and `params.get("old_record") → Err(_) => None`. `__rt::decode_params` returns an *empty* `Params` for any decode failure or non-map payload, so the `Err` arm conflates three distinct situations: key genuinely absent, key absent because decoding failed, and key absent because of host↔guest contract drift (schema change, key rename). For `record`, the documented authoring pattern (`record == Value::Null → record_error`, lib.rs:23-28) happens to be fail-closed — but for `old_record`, `None` is conventionally "insert", so an undecodable UPDATE payload presents to the validator as an INSERT and every old-record comparison is silently skipped.

**Failure scenario:** Host-side parameter-schema drift or a corrupted parameter buffer → the module does not trap or log anything; the validator receives `(Null, None)` instead of a failure. Either all writes fail validation with "empty_record" (fail-closed DoS), or — for validators that treat `old_record: None` as an insert — all old-record checks are bypassed. No signal reaches either the host or the module author that validation ran on the wrong data.

**Suggested fix:** Make the decode failure loud: have `__rt::decode_params` return `Result` and `trap!` on `Err` (mirroring how `#[function]`/`#[procedure]`/`#[scalar]` already trap on user `Err`), or trap when `len > 0` but the decoded `Params` is empty. Keep absent-key → `Null`/`None` only for genuinely absent keys; at minimum `old_record`'s `Err(_) => None` must distinguish "absent" from "undecodable".

### 2. `unsafe` slice construction from unverified `(ptr, len)`; negative `len` is immediate UB
**File:** `crates/shamir-sdk-macros/src/lib.rs:120-124` (`validator`), `253-257` (`function`), `381-385` (`procedure`), `547-551` (`scalar`)
**Severity:** low

**Issue:** All four generated `shamir_call`s do `core::slice::from_raw_parts(ptr as *const u8, len as usize)` with no validation. `len: i32` negative wraps to a ~4 GiB `usize` on wasm32 (`usize` = `u32`), violating `from_raw_parts`'s safety contract the moment the slice is formed; `ptr` is taken entirely on faith. The safety comment asserts "the host wrote `len` msgpack bytes at `ptr` via shamir_alloc" — a precondition the guest cannot verify, and which a buggy host (sign/truncation bug when unpacking the `(ptr, len)` i64, or a `-1` sentinel) violates silently. WebAssembly's sandbox bounds-checks the eventual reads, so the realistic worst case is a guest trap or a garbage decode feeding finding 1 — no corruption outside the module; hence low. The sibling `shamir_alloc` (lib.rs:109-114 and parallels) with negative `len` → `vec![0u8; huge]` → allocator OOM abort → trap, which is fail-closed and acceptable.

**Suggested fix:** Two lines per site before the `unsafe`: reject `len < 0` (e.g. `if len < 0 { shamir_sdk::__rt::trap("shamir_call: negative len"); }`) or route through `usize::try_from(len)` with an empty-slice fallback. Defensive guard consistent with the project's "checksums everywhere" reliability posture; turns UB into a clean trap.

### 3. No tests cover the two boundary-bearing macros or any runtime behavior of the generated ABI
**File:** `crates/shamir-sdk-macros/src/` (no `tests/` directory exists); sibling coverage limited to `crates/shamir-sdk/tests/scalar_compile_pass.rs` and `procedure_compile_pass.rs`
**Severity:** low

**Issue:** Compile-pass integration tests exist for `#[scalar]` and `#[procedure]` only — there is no compile-pass *or* behavioral test for `#[validator]` or `#[function]`, precisely the two macros carrying the extra boundary logic (the extraction coercion of finding 1, and the divergent return-type check of finding 5). Nothing anywhere invokes the generated `shamir_call` with malformed msgpack, a negative `len`, or a user function returning `Err`, so the trap paths and the fail-quiet path are behaviorally unexercised. CLAUDE.md's TDD protocol (red → green) is unfulfilled for the security boundary itself; finding 1 has survived precisely because nothing observes it.

**Suggested fix:** Add `validator_compile_pass.rs` / `function_compile_pass.rs` in `shamir-sdk/tests/` (the separate-crate pattern is already established to avoid `#[no_mangle]` symbol collisions), plus a test that feeds `shamir_call` corrupt/non-map msgpack and asserts a trap or observable failure rather than `Null`/`None` coercion. Write the latter red first — it should fail until finding 1 is fixed.

### 4. User error strings passed verbatim into the trap channel
**File:** `crates/shamir-sdk-macros/src/lib.rs:272` (`function`), `399` (`procedure`), `564` (`scalar`); sink at `crates/shamir-sdk/src/__rt.rs:64-69`
**Severity:** low

**Issue:** On user `Err(e)`, the generated code calls `shamir_sdk::__rt::trap(&e.to_string())`, embedding arbitrary author text — which may interpolate untrusted parameter content (cf. `Params::i64` formatting the offending value's type in `crates/shamir-sdk/src/params.rs:35-43`) — into the wasm trap message the host maps to `FunctionError::Compute`. If the host ever surfaces trap text to the untrusted DB caller, guest-internal details and echoed param data cross the module boundary unfiltered. Conditional on host policy (sibling crate's decision), hence low; the TODO at lib.rs:251 already plans the proper `FunctionError::User` envelope.

**Suggested fix:** Land the planned Result envelope so user errors travel as structured data, not trap text. Until then, document that `e.to_string()` is host-visible and keep trap messages to fixed category strings.

### 5. String-comparison return-type checks: `#[validator]`/`#[function]` reject valid spellings; all accept same-named foreign types
**File:** `crates/shamir-sdk-macros/src/lib.rs:63-72` (`validator`), `193-202` (`function`), vs `411-420` (`is_result_value_return`)
**Severity:** nit

**Issue:** `#[validator]` matches only the literal string `Validation` and `#[function]` only `Result<Value>` / `core::result::Result<Value,Error>`, while `#[procedure]`/`#[scalar]` use the prefix-normalizing `is_result_value_return`. Consequences: `-> shamir_sdk::Validation` and `-> shamir_sdk::Result<Value>` (the spelling used in this repo's own tests) are spuriously rejected by the first two macros, and `std::result::Result<...>` is rejected by the helper (only `core::result::` is stripped). Conversely, all the checks accept a same-named type from another crate (`my_crate::Validation`), which the generated coercion then rejects at type-check — a downstream compile error, never a runtime hole. No security impact (every false-accept is compile-caught), but boundary validation that both over- and under-matches undermines the "compile-time checks" the doc comments advertise (lib.rs:302-305, 457-462).

**Suggested fix:** Use `is_result_value_return` (or structural `syn` matching) uniformly in all four macros; extend the strip list to `std::result::` if `std` spellings should be accepted.

### 6. "Only one macro per crate" constraint documented but unenforced
**File:** `crates/shamir-sdk-macros/src/lib.rs:15`, `157`, `283`, `437`
**Severity:** nit

**Issue:** The doc comments state "Only one `#[x]` per crate is supported (single entrypoint)" for all four macros, but nothing detects a second application. The ABI safety story (exactly one `shamir_alloc`/`shamir_call` pair per module) rests on prose. Failure mode is loud — duplicate `#[no_mangle]` symbol definitions fail compilation — so this is not a security hole, but the constraint the guest ABI depends on has no enforcement or error message pointing at the cause.

**Suggested fix:** Detect the duplicate emission and produce a targeted error (e.g. an emitted `static _SHAMIR_ENTRYPOINT_TAKEN: () = ();` name collision, or a `compile_error!` pattern), or at minimum document that a second application fails at link time with duplicate symbols.

## Verified clean for this theme

- No `unsafe` outside the four noted `from_raw_parts` blocks (generated code) — and none in the macro code itself.
- `shamir_alloc` zero-initializes (`vec![0u8; len]`) before `core::mem::forget` — no uninitialized-memory disclosure across the ABI.
- `type_contains_ctx` (lib.rs:425-432) is advisory only, but the `#[scalar]` purity guarantee holds structurally: the generated `shamir_call` never constructs a `Ctx`, and the arity/type coercion makes any alias-based bypass a compile error.
- No string interpolation of external input into generated identifiers beyond the author's own function name (`Ident` is safe by construction) — no injection surface.
- No HMAC/SCRAM/TLS/timing-sensitive code exists in this crate to audit for side channels.

</details>
