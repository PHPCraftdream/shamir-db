<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Codec sources are discarded and user Result failures trap. Invocation allocation retention remains. Some transport choices are intentional contracts; exact dependency source makes allocation-failure handling more concrete than the prior conditional narrative.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 11 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Host-response decode failures silently become wrong data (empty results / `None` / `Null`)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Seven fallback sites remain. Query corruption becomes success; HTTP/insert subsequently fail with generic diagnostics. A malformed nonzero payload versus sentinel oracle is missing.

Evidence: [crates/shamir-sdk/src/host_imports.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L97); [crates/shamir-sdk/src/host_imports.rs:183](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L183); [crates/shamir-sdk/src/http.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L27).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — No error taxonomy: single-message `Error`, `Error::user` used for infra failures, and trap transport flattens user errors into `Compute`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Message-only Error and three Err-to-trap arms erase categories at the host Compute boundary. Validators have a separate Validation return convention.

Evidence: [crates/shamir-sdk/src/error.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/error.rs#L7); [crates/shamir-sdk-macros/src/lib.rs:272](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L272); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: [SUMMARY.md#6.2](SUMMARY.md#review-6-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — No free path for ABI buffers: unbounded guest-memory growth within a single long-running call

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Outgoing encoded Vecs and incoming allocated bytes are forgotten with no reclaim protocol. Dropping decoded values does not reclaim them. Store limits/drop contain the scope; no separate error cleanup exists.

Evidence: [crates/shamir-sdk/src/host_imports.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L64); [crates/shamir-sdk-macros/src/lib.rs:239](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L239); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:457](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L457).

Grouping/duplicate: [SUMMARY.md#4.1](SUMMARY.md#review-4-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — `Ctx::call` failures are uncatchable, and callee-result decode failure is conflated with `Value::Null`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Callee traps are documented intentional behavior; catchable composition is optional. Decode corruption still becomes Null. Normal successful Null is encoded distinctly, and callee failure does not normally return zero.

Evidence: [crates/shamir-sdk/src/context.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/context.rs#L83); [crates/shamir-sdk/src/host_imports.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L131); [crates/shamir-wasm-host/src/wasm/host_call.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_call.rs#L134).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Missing error-path tests: all suites cover happy paths and wire conformance only

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Getter-error, malformed-envelope, packing, sentinel, and DB mapping assertions remain absent. Workspace denial exists; recursion's generic-trap allowance cannot prove a specific depth-limit mechanism.

Evidence: [crates/shamir-sdk/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/mod.rs#L1); [crates/shamir-db/tests/functions_lifecycle.rs:559](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L559); [crates/shamir-db/tests/functions_lifecycle.rs:762](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/functions_lifecycle.rs#L762).

Grouping/duplicate: [SUMMARY.md#1.1](SUMMARY.md#review-1-1). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — `Table::insert` error path conflates absent, decode-failure, and genuine null

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Zero/decode failure both produce db_insert returned null. A genuine null record is not established under the Map contract; the needed oracle identifies the actual protocol/decode fault.

Evidence: [crates/shamir-sdk/src/host_imports.rs:157](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L157); [crates/shamir-sdk/src/db.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L85); [crates/shamir-sdk/src/db.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L89).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — `__rt::decode_params` masks a params decode failure as an empty `Params`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The fallback permits parameter-ignoring bodies to run and makes getters report missing. Validator extraction also maps getter errors to Null, so lazy error storage alone is insufficient.

Evidence: [crates/shamir-sdk/src/__rt.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L14); [crates/shamir-sdk-macros/src/lib.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L129); [crates/shamir-sdk/src/tests/value_tests.rs:400](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/value_tests.rs#L400).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — `__rt::encode_value` maps encode failure to empty bytes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Published rmp-serde 1.3.1 FallibleWriter can fail try_reserve for supported payloads. The SDK discards this resource error; it is not exclusively hypothetical future serialization behavior.

Evidence: [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [crates/shamir-sdk/src/__rt.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L20).

Grouping/duplicate: [SUMMARY.md#6.1](SUMMARY.md#review-6-1). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — `__rt::block_on` busy-spins forever on `Pending` with a no-op waker

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

An unresolved future lacks scheduling/diagnostics and is repolled continuously. Later Ready succeeds; finite default fuel bounds WASM execution, unlike direct native use.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:477](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L477).

Grouping/duplicate: [SUMMARY.md#2.1](SUMMARY.md#review-2-1). This is not an additional independent defect.

<a id="review-10"></a>

### Claim 10 — `HttpResponse::from_value`: truncating status cast plus silent leniency on malformed headers/body

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Narrowing and dropped wrong-typed fields remain. Normal host encoding supplies correct types; public malformed-value decoding is directly reachable.

Evidence: [crates/shamir-sdk/src/http.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L134); [crates/shamir-sdk/src/http.rs:147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/http.rs#L147); [crates/shamir-wasm-host/src/wasm/host_http.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/host_http.rs#L93).

Grouping/duplicate: [SUMMARY.md#1.6](SUMMARY.md#review-1-6). This is not an additional independent defect.

<a id="review-11"></a>

### Claim 11 — Silent truncations in wire helpers: `visit_u64` wraparound and `leak_result` len mask

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

External uint64 wraps in the SDK; native packing loses pointer/length bits. Oversized buffers are excluded by default WASM limits, and a native unpacking consumer is not established.

Evidence: [crates/shamir-sdk/src/value.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L98); [crates/shamir-sdk/src/__rt.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L29); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L234).

Grouping/duplicate: [SUMMARY.md#1.7](SUMMARY.md#review-1-7). This is not an additional independent defect.

## Evidence and recipe corrections

- Allocation failure in the exact codec is a present resource-error path; unconditional invariant panic is an unsafe diagnostic recipe.
- Preserve encoded Null results separately from absent packed zero, and do not authorize Null-document insertion merely to remove a fallback symptom.
- Returning catchable callee errors changes an explicitly documented trap contract; coordinate an optional API/wire change rather than claiming existing catchability.
- An unmarked [false,message] result envelope collides with legitimate guest list data; validator transport must retain its distinct convention.
- A function's DB host calls autocommit independently. A later trap/OOM does not prove earlier writes rolled back as one enclosing batch transaction.
- Native compile-pass targets do not invoke the packed-pointer ABI and therefore provide no pointer/ownership oracle.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk -- Error handling & resource lifecycle

## Summary

The pure-Rust surface (`Params`, `Validation`, the http envelope decoder) follows the documented `Result`/`?` discipline well, but the host-import boundary does not: seven decode sites silently map msgpack failures to `None` / `Value::Null` / `Ok(vec![])`, converting a host/guest wire violation into plausible-looking wrong data (an empty query result is indistinguishable from "no rows"). The library `Error` is a single-message struct with no taxonomy, and the macro transport flattens even `Error::user` into a WASM trap (`FunctionError::Compute`), so user errors are indistinguishable from guest panics — acknowledged only as a TODO in the sibling macro crate. Resource lifecycle on the error path has no reclaim story at all (every ABI buffer in both directions is leaked, with no free import), and error paths are essentially untested: the existing suites cover wire conformance and happy paths only.

## Findings

### 1. Host-response decode failures silently become wrong data (empty results / `None` / `Null`)
- **File:line:** `crates/shamir-sdk/src/host_imports.rs:97, 106, 131, 146, 162, 183, 207`
- **Severity:** high
- **Issue:** Every host-import response goes through `rmp_serde::from_slice(bytes).ok()` (line 97 `batch_get`, 106 `global_get`, 146 `db_get`) or `.unwrap_or(Value::Null)` (131 `call`, 162 `db_insert`, 207 `http_fetch`) or worst of all `.unwrap_or(Value::List(Vec::new()))` (183 `db_query`). A decode failure of host-written bytes is silently swallowed instead of propagated, contrary to CLAUDE.md's error rules ("Return `Result<T, E>`", "Use `?` to propagate").
- **Failure scenario:** After any wire drift between `shamir_types::QueryValue` and the guest mirror `Value` (the exact drift `value.rs` exists to track), or a truncated host write, `Table::query` returns `Ok(vec![])` — the guest function reports "no matching rows" with no error anywhere. `db_get`/`global_get`/`batch_get` conflate "corrupt response" with "absent" (`None`); `call`/`db_insert` conflate it with a legitimate `Null` result, and `Table::insert` then reports the misleading "db_insert returned null".
- **Suggested fix:** Propagate a decode error at the `Table`/`Ctx` API layer (these already return `Result`) with the underlying rmp-serde message, e.g. `Error::decode("db_query", e)`; for the `Option`-returning getters keep the ABI's packed==0 "absent" path separate from decode failure (decode failure should trap or return a distinguishable error, not `None`).

### 2. No error taxonomy: single-message `Error`, `Error::user` used for infra failures, and trap transport flattens user errors into `Compute`
- **File:line:** `crates/shamir-sdk/src/error.rs:6-23`; `crates/shamir-sdk/src/__rt.rs:64-69`; `crates/shamir-sdk/src/db.rs:141, 144, 147`; `crates/shamir-sdk/src/http.rs:27, 30, 34, 41, 127, 137`; transport: `crates/shamir-sdk-macros/src/lib.rs:271-273` (TODO at :251)
- **Severity:** high
- **Issue:** CLAUDE.md mandates "`thiserror` for library error enums (with `#[from]` where natural)". This crate hand-rolls a one-field struct whose only constructor is `Error::user`, then uses it for everything: http allowlist/curl/timeout failures, host contract violations ("host returned unexpected value"), and internal encode/decode failures in `Db::execute` ("execute: encode batch: {e}"). The doc positions `Error::user` as "a deliberate, user-surfaced error" (error.rs:5), which none of those are. Worse, the macro-generated entrypoint turns every user `Err(e)` into `__rt::trap(&e.to_string())` → WASM trap → host `FunctionError::Compute`, so a guest panic and a deliberate user error are indistinguishable on the host — the `Error` type's entire semantic content is destroyed at the ABI boundary (acknowledged as `TODO(slice 4)` in the macro crate).
- **Failure scenario:** A `#[validator]`-style "field `zip` is invalid" user error reaches the client as `FunctionError::Compute` ("compute/internal error"), while a genuine guest bug also reaches the client as `Compute`; the host cannot route, filter, or i18n user errors, and guest authors cannot branch on callee failure kinds.
- **Suggested fix:** Convert `Error` to a `thiserror` enum (`User { message }`, `Host { op, message }`, `Decode { context, #[source] }`), keep `Error::user` as the constructor for the user variant, stop using it for infra errors, and complete the planned result-envelope transport (as `http_fetch` already does) so user errors arrive as `FunctionError::User` and stay catchable.

### 3. No free path for ABI buffers: unbounded guest-memory growth within a single long-running call
- **File:line:** `crates/shamir-sdk/src/host_imports.rs:60-66` (`encode_leak`; extern block :29-53 has no dealloc import), `crates/shamir-sdk/src/__rt.rs:25-30` (`leak_result`); allocator: `crates/shamir-sdk-macros/src/lib.rs:238-244`
- **Severity:** medium
- **Issue:** Every guest→host call leaks its encoded key/doc/filter/request buffer, and every host→guest response buffer (written via the guest's leak-everything `shamir_alloc` bump allocator) is never reclaimed — there is no `shamir_dealloc` import or export anywhere in the ABI. The comments justify correctness ("host reads synchronously; the Store is dropped after `shamir_call` returns"; "the WASM module is short-lived") but not reclamation: that assumption covers the per-module result buffer, not the intra-call loop.
- **Failure scenario:** A `#[procedure]` that loops — inserting/updating 100k rows one by one, or paginating via `db_query` — leaks both directions monotonically inside one `shamir_call`; wasm32 linear memory grows until allocation fails and the function dies with an opaque OOM trap mid-batch, far from the root cause.
- **Suggested fix:** Add a dealloc import (or guest-exported free) and release each buffer after the synchronous host read completes on both sides; alternatively document an explicit "N host calls per invocation" ceiling per SDK slice until reclamation exists.

### 4. `Ctx::call` failures are uncatchable, and callee-result decode failure is conflated with `Value::Null`
- **File:line:** `crates/shamir-sdk/src/context.rs:86-88`; `crates/shamir-sdk/src/host_imports.rs:121-132`
- **Severity:** medium
- **Issue:** `call` returns `Value`, not `Result`. Per its own doc, a missing callee, depth-limit excess, or callee error traps the whole guest function — the caller cannot catch or branch on callee failure. Inconsistent with `Ctx::http_fetch`, which deliberately returns catchable `Err` via an `[ok, payload]` envelope (context.rs:103-119). Additionally `unpack_ptr_len(packed) == None → return Value::Null` and `.unwrap_or(Value::Null)`: if the host signals failure with 0 instead of trapping, or the success payload fails to decode, the caller silently receives `Value::Null`, indistinguishable from a callee that legitimately returned `Null`.
- **Failure scenario:** A procedure composing N sub-functions via `ctx.call` loses the entire batch on the first sub-function error with no recovery path; or a decode hiccup silently turns a callee's Map result into `Null` and downstream `params.get("x")`-style reads start failing with "missing parameter" far from the real fault.
- **Suggested fix:** Mirror the `http_fetch` envelope for `call` (`Result<Value>`), or at minimum document the `Null` ambiguity and make decode failure a distinct, loud outcome (finding 1).

### 5. Missing error-path tests: all suites cover happy paths and wire conformance only
- **File:line:** `crates/shamir-sdk/src/tests/value_tests.rs`, `crates/shamir-sdk/src/tests/validation_tests.rs`, `crates/shamir-sdk/tests/procedure_compile_pass.rs`, `crates/shamir-sdk/tests/scalar_compile_pass.rs`
- **Severity:** medium
- **Issue:** Zero tests exercise any error branch: `decode_fetch_envelope` (http.rs:24-44, `pub(crate)` and directly unit-testable — all five error branches untested), `HttpResponse::from_value` error/leniency branches (http.rs:124-160), `Table::insert`/`query` error arms (db.rs:89, 105), `Params` error messages (only happy path tested, value_tests.rs:400-411), `decode_params`' malformed-input fallback (__rt.rs:14), `unpack_ptr_len` 0-absent semantics. The two integration tests assert only that the macros compile.
- **Failure scenario:** Regressions in the exact paths findings 1/2/4 concern (envelope shape changes, message drift, conflation of absent vs corrupt) land silently; per TDD protocol (CLAUDE.md 🔴 Red) these error branches have no failing-first test to protect them.
- **Suggested fix:** Add a `tests/error_path_tests.rs` per the repo's test-organisation layout covering `decode_fetch_envelope` (ok/non-bool/wrong-shape/non-map/missing-status), `from_value` leniency matrix, and `Params` typed-getter failures; extract the pure decode logic from the host-import shims (which panic off-wasm) so table/db error arms become host-testable.

### 6. `Table::insert` error path conflates absent, decode-failure, and genuine null
- **File:line:** `crates/shamir-sdk/src/db.rs:86-92`
- **Severity:** low
- **Issue:** `Value::Null => Err(Error::user("db_insert returned null"))` treats three distinct situations identically: host returned packed==0 (per host_imports.rs:17 ABI doc that means "absent" — a host contract violation for an insert), guest decode failure (finding 1's fallback `Value::Null`), or a legitimately null stored record. The message tells the guest author none of this.
- **Failure scenario:** During a wire-drift incident, every insert surfaces as "db_insert returned null", pointing the author at the table/record rather than the decode layer.
- **Suggested fix:** Distinguish packed==0 as a contract error naming the host op, propagate decode errors (finding 1), and reserve `Ok(Value::Null)` for a genuine null record.

### 7. `__rt::decode_params` masks a params decode failure as an empty `Params`
- **File:line:** `crates/shamir-sdk/src/__rt.rs:11-16`
- **Severity:** low
- **Issue:** On decode failure or non-map input, `_ => Params::new()` — every subsequent `params.get("x")` then fails with "missing parameter: x", hiding the real cause (malformed host bytes).
- **Failure scenario:** A host-side encoding change breaks param delivery; the guest author debugs "why is my param missing" instead of "why did decoding fail", because the observable symptom is per-key missing-parameter errors.
- **Suggested fix:** Keep the failure in-band but truthful: store the decode error in `Params` and have the first accessor report "params failed to decode: {e}" instead of "missing parameter".

### 8. `__rt::encode_value` maps encode failure to empty bytes
- **File:line:** `crates/shamir-sdk/src/__rt.rs:19-21`
- **Severity:** low
- **Issue:** `rmp_serde::to_vec(value).unwrap_or_else(|_| Vec::new())` silently substitutes empty output for a serialization error. For the self-contained `Value` enum this is near-unreachable today, which makes it precisely the kind of silent fallback that will never be noticed if it ever becomes reachable.
- **Failure scenario:** If an encode failure ever occurs, the host receives 0 bytes and the failure surfaces as a baffling host-side decode error with no guest-side trace.
- **Suggested fix:** Treat it as the programmer-bug invariant it is: `unreachable!()`-style panic with the error message (sanctioned by CLAUDE.md for invariant violations), or thread a `Result` to the macro entrypoint.

### 9. `__rt::block_on` busy-spins forever on `Pending` with a no-op waker
- **File:line:** `crates/shamir-sdk/src/__rt.rs:36-61`
- **Severity:** low (documented slice-3 limitation)
- **Issue:** A future that legitimately yields `Pending` (e.g. a guest author awaits a channel, or an async host op not yet wired) is polled in a `spin_loop` tight loop at 100% CPU; wakeups are discarded by the no-op waker, so it can never make progress — it burns the host thread and manifests as a silent hang rather than a diagnosable error.
- **Failure scenario:** A guest function ported from a tokio context dead-calls into `block_on` and wedges the runtime thread; the symptom (host SLOW/TIMEOUT) points nowhere near the guest spin.
- **Suggested fix:** Count yields and `trap("future yielded Pending — async host ops are not supported by this runtime slice")` after a small bound, converting the hang into a named, greppable failure.

### 10. `HttpResponse::from_value`: truncating status cast plus silent leniency on malformed headers/body
- **File:line:** `crates/shamir-sdk/src/http.rs:130-137, 139-153`
- **Severity:** nit
- **Issue:** `Value::Int(n) => Some(*n as u16)` truncates (host `Int(70000)` → status 4464; negative values wrap), where `status` is otherwise strictly validated (missing → error). Conversely a present-but-wrong-typed `headers` (e.g. `Value::Str`) or `body` silently degrades to empty instead of erroring — two different strictness policies in one decoder.
- **Failure scenario:** A host change in the response encoding produces a garbage status code or silently dropped headers, with `Ok(HttpResponse)` hiding it.
- **Suggested fix:** Range-check the int (`u16::try_from` → error on failure) and either validate the headers map shape or document the leniency deliberately.

### 11. Silent truncations in wire helpers: `visit_u64` wraparound and `leak_result` len mask
- **File:line:** `crates/shamir-sdk/src/value.rs:98`; `crates/shamir-sdk/src/__rt.rs:25-30`
- **Severity:** nit
- **Issue:** `visit_u64` wraps `u64 > i64::MAX` into a negative `Value::Int` (`v as i64`) instead of erroring; `leak_result` masks the length to 32 bits (`len & 0xFFFF_FFFF`), silently corrupting the packed result for a >4 GiB buffer (unreachable under wasm32, but the helper also compiles for the host target where 64-bit pointers make the `ptr << 32` packing lossy).
- **Failure scenario:** Only reachable via host bug or non-wasm misuse of the helper — but each would corrupt data silently rather than fail loudly.
- **Suggested fix:** Map out-of-range u64 to an error (or a documented lossy variant) and `debug_assert!` the packing preconditions (ptr fits 32 bits, len fits 32 bits) so violations fail in host-target tests.

</details>
