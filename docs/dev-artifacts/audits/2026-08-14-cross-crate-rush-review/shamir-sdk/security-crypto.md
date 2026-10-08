<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Boundary-hardening omissions remain. Several proposed exploit narratives require stronger evidence, and the scalar alias example is invalid although the purity guarantee is still unenforced.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 13 | 10 | 0 | 0 | 0 | 1 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `block_on` spin-loops forever on any `Poll::Pending` future (DoS / hang at the untrusted-guest boundary)

Status: `confirmed-open`. Current risk: `medium`.

An unresolved future still busy-polls and can exhaust its execution budget; native helper use has no built-in timeout. Any-Pending forever is false because subsequent polls can return Ready. WASM budget enforcement contains the resource abuse; this is not a sandbox escape.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](../../../../../crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:477](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L477); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:487](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L487); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:230](../../../../../crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L230).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `HttpRequest` performs zero validation of method / URL / header name / header value (CRLF & token-injection source at the egress boundary)

Status: `confirmed-open`. Current risk: `medium`.

SDK construction still accepts control characters, host decoding forwards strings, and curl escaping handles only backslash and quote. CR/LF propagation into config is source-proven. Actual curl directive/header/request injection is unverified; URL allowlist/SSRF checks run before config generation.

Evidence: [crates/shamir-sdk/src/http.rs:60](../../../../../crates/shamir-sdk/src/http.rs#L60); [crates/shamir-sdk/src/http.rs:80](../../../../../crates/shamir-sdk/src/http.rs#L80); [crates/shamir-sdk/src/http.rs:86](../../../../../crates/shamir-sdk/src/http.rs#L86); [crates/shamir-wasm-host/src/wasm/host_http.rs:36](../../../../../crates/shamir-wasm-host/src/wasm/host_http.rs#L36); [crates/shamir-db/src/shamir_db/curl_gateway.rs:49](../../../../../crates/shamir-db/src/shamir_db/curl_gateway.rs#L49); [crates/shamir-db/src/shamir_db/curl_gateway.rs:210](../../../../../crates/shamir-db/src/shamir_db/curl_gateway.rs#L210).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Guest ABI builds slices from host-returned `(ptr, len)` with no sanity check (undocumented unsafe trust assumption)

Status: `confirmed-open`. Current risk: `low`.

Eight response paths still construct unchecked slices after signed unpacking. Host writers currently check length, allocator pointer, and memory range; the residual issue requires a trusted-host contract bug or incompatible replacement. Signed low bits are not a legitimate negative-length wire field.

Evidence: [crates/shamir-sdk/src/host_imports.rs:70](../../../../../crates/shamir-sdk/src/host_imports.rs#L70); [crates/shamir-sdk/src/host_imports.rs:96](../../../../../crates/shamir-sdk/src/host_imports.rs#L96); [crates/shamir-sdk/src/host_imports.rs:224](../../../../../crates/shamir-sdk/src/host_imports.rs#L224); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:339](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L339); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:355](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L355).

Grouping/duplicate: `SUMMARY.md#3.2`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Encode/decode failures are silently swallowed, fail-open: a failed filter encode degrades to "query ALL rows"

Status: `confirmed-open`. Current risk: `low`.

The conditional chain remains: encode error becomes empty bytes; db_query treats empty filter bytes as None. No triggering serialization failure was established. Decode fallback remains too, but HTTP/insert fallback Null produces public errors rather than successful data exposure.

Evidence: [crates/shamir-sdk/src/__rt.rs:20](../../../../../crates/shamir-sdk/src/__rt.rs#L20); [crates/shamir-sdk/src/host_imports.rs:60](../../../../../crates/shamir-sdk/src/host_imports.rs#L60); [crates/shamir-sdk/src/host_imports.rs:170](../../../../../crates/shamir-sdk/src/host_imports.rs#L170); [crates/shamir-wasm-host/src/wasm/host_db.rs:136](../../../../../crates/shamir-wasm-host/src/wasm/host_db.rs#L136); [crates/shamir-sdk/src/http.rs:27](../../../../../crates/shamir-sdk/src/http.rs#L27).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Documented scalar "purity guarantee" is a token-match lint, trivially bypassed by a type alias

Status: `confirmed-open`. Current risk: `low`.

Purity remains unenforced because scalar bodies can construct the public Ctx::new. The stated two-argument alias example is refuted: scalar requires one argument and generates a Params-only wrapper, not Ctx::new. Access still depends on actual host gateways; no privilege escalation is established.

Evidence: [crates/shamir-sdk/src/context.rs:15](../../../../../crates/shamir-sdk/src/context.rs#L15); [crates/shamir-sdk/src/context.rs:64](../../../../../crates/shamir-sdk/src/context.rs#L64); [crates/shamir-sdk-macros/src/lib.rs:476](../../../../../crates/shamir-sdk-macros/src/lib.rs#L476); [crates/shamir-sdk-macros/src/lib.rs:525](../../../../../crates/shamir-sdk-macros/src/lib.rs#L525); [crates/shamir-sdk-macros/src/lib.rs:556](../../../../../crates/shamir-sdk-macros/src/lib.rs#L556); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:423](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L423).

Grouping/duplicate: `SUMMARY.md#3.4`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — User `Err` results surface as WASM panics mapped to `FunctionError::Compute`, blurring crash vs. user error

Status: `confirmed-open`. Current risk: `medium`.

Function/procedure/scalar Err arms still invoke panic-based trap, and the host maps traps to Compute. The User variant exists on the host but no guest result-envelope transport selects it. Validators return Validation directly, so the report's validator-Err example is not the validator macro contract.

Evidence: [crates/shamir-sdk/src/__rt.rs:64](../../../../../crates/shamir-sdk/src/__rt.rs#L64); [crates/shamir-sdk-macros/src/lib.rs:271](../../../../../crates/shamir-sdk-macros/src/lib.rs#L271); [crates/shamir-sdk-macros/src/lib.rs:398](../../../../../crates/shamir-sdk-macros/src/lib.rs#L398); [crates/shamir-sdk-macros/src/lib.rs:563](../../../../../crates/shamir-sdk-macros/src/lib.rs#L563); [crates/shamir-wasm-host/src/error.rs:30](../../../../../crates/shamir-wasm-host/src/error.rs#L30); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — `leak_result` truncates pointers on 64-bit host targets (latent UB in the host-testing ABI path)

Status: `confirmed-open`. Current risk: `low`.

The public helper still shifts a native pointer into a 32-bit pointer field without target gating. Native high bits are lost; the helper itself does not dereference the result. UB requires an unsupported native ABI consumer to dereference it; the production consumer is WASM-only.

Evidence: [crates/shamir-sdk/src/__rt.rs:25](../../../../../crates/shamir-sdk/src/__rt.rs#L25); [crates/shamir-sdk/src/lib.rs:18](../../../../../crates/shamir-sdk/src/lib.rs#L18); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:567](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L567).

Grouping/duplicate: `SUMMARY.md#3.6`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Unbounded per-call buffer leaks in the guest ABI are undocumented as a resource budget

Status: `confirmed-open`. Current risk: `nit`.

Request encodings and guest-allocated response bytes remain unreclaimed until Store destruction. Internal comments acknowledge invocation lifetime but public authoring docs do not explain cumulative loop cost. Default memory/fuel limits contain growth, not eliminate it.

Evidence: [crates/shamir-sdk/src/host_imports.rs:55](../../../../../crates/shamir-sdk/src/host_imports.rs#L55); [crates/shamir-sdk/src/host_imports.rs:64](../../../../../crates/shamir-sdk/src/host_imports.rs#L64); [crates/shamir-sdk-macros/src/lib.rs:239](../../../../../crates/shamir-sdk-macros/src/lib.rs#L239); [crates/shamir-sdk/src/context.rs:116](../../../../../crates/shamir-sdk/src/context.rs#L116); [crates/shamir-wasm-host/src/wasm/wasm_engine.rs:234](../../../../../crates/shamir-wasm-host/src/wasm/wasm_engine.rs#L234).

Grouping/duplicate: `SUMMARY.md#4.1`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `HttpResponse::from_value` truncating status cast and lenient header/body parsing

Status: `confirmed-open`. Current risk: `low`.

Unchecked status conversion and lossy parsing remain. Normal host encoding supplies the expected types; this is protocol-drift hardening, not a demonstrated remote-input type confusion.

Evidence: [crates/shamir-sdk/src/http.rs:134](../../../../../crates/shamir-sdk/src/http.rs#L134); [crates/shamir-sdk/src/http.rs:142](../../../../../crates/shamir-sdk/src/http.rs#L142); [crates/shamir-sdk/src/http.rs:150](../../../../../crates/shamir-sdk/src/http.rs#L150); [crates/shamir-wasm-host/src/wasm/host_http.rs:93](../../../../../crates/shamir-wasm-host/src/wasm/host_http.rs#L93).

Grouping/duplicate: `SUMMARY.md#1.6`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — Security-relevant HTTP envelope decoding has zero test coverage

Status: `confirmed-open`. Current risk: `low`.

SDK-local malformed-envelope and response-shape tests remain absent. Workspace e2e tests do exercise catchable denial and nominal response bodies, so literal zero coverage is false. The nominal test accepts some error strings and cannot reliably detect a broken success path.

Evidence: [crates/shamir-sdk/src/tests/mod.rs:1](../../../../../crates/shamir-sdk/src/tests/mod.rs#L1); [crates/shamir-sdk/src/http.rs:24](../../../../../crates/shamir-sdk/src/http.rs#L24); [crates/shamir-db/tests/functions_lifecycle.rs:762](../../../../../crates/shamir-db/tests/functions_lifecycle.rs#L762); [crates/shamir-db/tests/functions_lifecycle.rs:833](../../../../../crates/shamir-db/tests/functions_lifecycle.rs#L833); [crates/shamir-db/tests/functions_lifecycle.rs:873](../../../../../crates/shamir-db/tests/functions_lifecycle.rs#L873).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-notes-1"></a>

### Claim Notes.1 — No timing side-channels: the crate never compares secrets or credential material

Status: `not-applicable`. Current risk: —.

Non-finding supported narrowly: no authentication or secret-comparison mechanism is implemented here; Params compares parameter names. This is not a universal side-channel proof for guest functions or host dependencies.

Evidence: [crates/shamir-sdk/src/params.rs:29](../../../../../crates/shamir-sdk/src/params.rs#L29); [crates/shamir-sdk/src/lib.rs:19](../../../../../crates/shamir-sdk/src/lib.rs#L19); [crates/shamir-sdk/Cargo.toml:8](../../../../../crates/shamir-sdk/Cargo.toml#L8).

<a id="review-notes-2"></a>

### Claim Notes.2 — `Value`'s recursive `Deserialize` is depth-bounded by rmp-serde's default recursion limit

Status: `unverified`. Current risk: —.

The SDK recursively invokes the deserializer without its own explicit limit. The lockfile identifies rmp-serde 1.3.1, but the pinned implementation was unavailable; neither exact default-depth enforcement on deserialize_any nor the stronger no-stack-overflow conclusion was source-proven.

Evidence: [Cargo.lock:2949](../../../../../Cargo.lock#L2949); [crates/shamir-sdk/src/value.rs:121](../../../../../crates/shamir-sdk/src/value.rs#L121); [crates/shamir-sdk/src/value.rs:129](../../../../../crates/shamir-sdk/src/value.rs#L129); [crates/shamir-sdk/src/value.rs:140](../../../../../crates/shamir-sdk/src/value.rs#L140).

<a id="review-notes-3"></a>

### Claim Notes.3 — The host side of the ABI is properly defensive in both directions

Status: `not-applicable`. Current risk: —.

The cited memory-range guarantee is supported: host reads reject negative/range-invalid inputs, writers validate allocation ranges, and result unpacking checks against memory size. This supports host-side slice safety, not complete protocol validity or guest allocator provenance.

Evidence: [crates/shamir-wasm-host/src/wasm/wasm_function.rs:297](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L297); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:343](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L343); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:355](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L355); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:567](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L567).

## Corrections and qualified non-findings

- Qualify DoS as execution-budget consumption by an unresolved authored future, not inevitable infinite production execution.
- Reject controls at the host security boundary as well as optionally in the SDK; guest authors can bypass SDK constructors with custom WASM.
- The scalar alias example cannot compile as described; public Ctx::new in a scalar body explains the real guarantee gap.
- Host gateway presence/absence is an invocation capability restriction, not proof of enforced function-kind purity.
- Packed pointer and length fields are unsigned bit fields; checking only signed negativity is not a complete guest memory-range or allocation-validity check.
- Explicit as-u32 casts merely declare truncation and do not make native pointer packing correct. A target-gated implementation needs a native stub for macro compile-pass targets.
- Default rmp-serde depth bounding, even if verified, would not alone prove guest-stack safety.
- HTTP payloads are normalized by the host before SDK parsing; remote servers do not directly supply SDK Value variants.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk -- Security & crypto boundary

## Summary

This crate contains no auth, HMAC, SCRAM, or TLS code (those live in `shamir-connect` / `shamir-server` / `shamir-db`); its security surface is the guest-host WASM ABI (`__rt`, `host_imports`) and the HTTP-egress request builder (`http`). The dominant risks are at the untrusted-guest boundary: the `block_on` executor busy-spins forever on any genuinely `Pending` future (turning a guest foot-gun into a full fuel/wall-clock burn, or a hang on the host-target test path), and `HttpRequest` passes method/URL/header strings to the egress boundary with zero character validation, relying entirely on downstream host escaping (which, verified in `curl_gateway.rs`, does not strip CR/LF). The unsafe `(ptr,len)` slice constructions and fail-open encode/decode swallowing are low-severity given the trusted-host model, but are unenforced/undocumented assumptions. Tests thoroughly cover msgpack wire conformance and `Validation` shape, but the security-relevant HTTP envelope decoding has no coverage at all.

## Findings

### 1. `block_on` spin-loops forever on any `Poll::Pending` future (DoS / hang at the untrusted-guest boundary)
- File: `crates/shamir-sdk/src/__rt.rs:36-61` (spin loop at :50-59; consumed by macro-generated `shamir_call` via `shamir-sdk-macros/src/lib.rs:144,264,391,556`)
- Severity: medium
- Issue: Guest futures are driven on a no-op waker; `Poll::Pending` falls into `core::hint::spin_loop()` with a comment claiming "If a future genuinely needs async I/O (slice 4 host imports), this will spin. For now, a tight loop is correct." That comment is stale: slice-4 host imports exist and are `func_wrap_async` on the host (`shamir-wasm-host/src/wasm/wasm_function.rs:195-213`), so imports suspend *below* the poll and never surface as `Pending`. What *does* return `Pending` is any guest-local await (oneshot channel, timer, custom `Future`) -- and then the guest spins at 100% until the host's epoch-interruption / top-level wall-clock deadline kills it (`wasm_function.rs:542-559`), having burned the entire fuel budget for a generic "wall-clock deadline" error. On the host target ("This crate also works on the host target for testing", `lib.rs:15-16`) there is no epoch/timeout: a single such future hangs the native test runner until nextest's 180 s kill -- exactly the silent-hang class CLAUDE.md mandates hunting down.
- Failure scenario: A guest author writes `#[procedure]` awaiting a `tokio::sync::oneshot` (plausible -- the SDK advertises "plain async Rust"). Every invocation burns full fuel + hits the wall-clock deadline with an unhelpful error; a host-target unit test of that function deadlocks the suite.
- Suggested fix: Replace the spin with an immediate trap: `Poll::Pending => __rt::trap("guest function yielded Pending -- host imports suspend transparently; guest-local awaits are unsupported")`. This converts a silent full-budget burn / runner hang into a fast, diagnosable `FunctionError::Compute` and deletes the stale "for now" comment.

### 2. `HttpRequest` performs zero validation of method / URL / header name / header value (CRLF & token-injection source at the egress boundary)
- File: `crates/shamir-sdk/src/http.rs:80-95` (`header`, `method`, `body` setters; consumed by `Ctx::http_fetch` at `context.rs:116-119`)
- Severity: medium
- Issue: The SDK is the boundary through which fully guest-controlled strings enter egress, yet it imposes no constraints at all. Downstream today is the curl gateway (`shamir-db/src/shamir_db/curl_gateway.rs:71-89`), whose `escape_curl_value` (:210-220) escapes only the backslash and the double-quote characters -- CR, LF, and other control characters pass verbatim into curl-config `header = "name: value"` / `request = "method"` quoted strings, and from there into curl's `-H`/`-X` arguments. Whether an embedded LF becomes extra proxied headers or a smuggled request line depends on the installed curl version's `-H`/`-X` parsing -- a boundary must not depend on that. Any future host implementation (e.g. a raw-socket writer instead of curl) inherits the gap silently, and nothing in the SDK's docs states the host contract.
- Failure scenario: A guest sets `header("X-Trace", "a\r\nHost: internal-admin")` or `method("GET\r\nX-Priv: 1")`; on a curl build that tolerates embedded CRLF, the proxied request gains attacker-chosen headers/request line, reaching targets already passed by the allowlist/SSRF guard.
- Suggested fix: Validate at construction in `HttpRequest`: reject `\r`, `\n`, `\0` in method, URL, header names and values (optionally restrict method to RFC token characters, URL to `http`/`https`), and document the invariant. Keep the host-side guard too (defense in depth), and add a `strip/escape CRLF` step in `escape_curl_value`.

### 3. Guest ABI builds slices from host-returned `(ptr, len)` with no sanity check (undocumented unsafe trust assumption)
- File: `crates/shamir-sdk/src/host_imports.rs:96, 105, 130, 145, 161, 182, 206, 224` (all via `unpack_ptr_len` :70-77)
- Severity: low
- Issue: Every host-import result path executes `core::slice::from_raw_parts(ptr, len)` directly on the packed `i64`. A negative `len` (host bug / ABI drift) becomes a ~4-billion-byte slice -- instant UB in the guest; the per-site "Safety:" comments assert the invariant but nothing checks it. The trusted-host model is legitimate (and the host's mirror function does validate: `read_guest_mem`, `shamir-wasm-host/src/wasm/wasm_function.rs:297-313`, rejects negative and out-of-bounds pairs), but the guest side is where the `unsafe` lives and it is entirely unguarded.
- Failure scenario: A future host change returns a malformed packed pair (e.g. an error code stuffed into the low bits with the high bits zeroed, or a negative length); the guest constructs a wildly out-of-bounds slice instead of cleanly reporting "absent"/`Null`.
- Suggested fix: Centralize one `guest_slice(packed) -> Option<&[u8]>` helper that rejects `ptr <= 0 || len < 0` (and, ideally, `ptr + len` overflow via checked math) before `from_raw_parts`; all eight call sites shrink to one audited unsafe block.

### 4. Encode/decode failures are silently swallowed, fail-open: a failed filter encode degrades to "query ALL rows"
- File: `crates/shamir-sdk/src/__rt.rs:19-21` (`encode_value` -> empty vec), `crates/shamir-sdk/src/host_imports.rs:170-177` (`db_query`: len 0 == no filter) and `:16` (`decode_params` -> empty `Params`), plus `.ok()`/`unwrap_or` decodes at `host_imports.rs:97, 106, 131, 146, 162, 183, 207`
- Severity: low
- Issue: `encode_value` maps any rmp-serde failure to `Vec::new()`. In `Table::query(Some(f))` that leaks a zero-length buffer, which the ABI documents as "zero-length filter means no filter" -- i.e. the host returns the **whole table** where the author asked for a filtered subset (over-exposure inside the function's own actor permissions). The decode-side swallows (`from_slice(...).ok()`, `unwrap_or(Value::Null)`) similarly convert protocol violations into plausible-looking "absent"/empty results instead of errors. `rmp_serde::to_vec` is practically infallible for this `Value` today, so impact is low, but every failure direction is fail-open and invisible.
- Failure scenario: A future `Value` variant (or upstream rmp-serde change) makes serialization of filters fail; guests silently receive full-table scans and act on data their filter was supposed to exclude, with no error anywhere.
- Suggested fix: Make `encode_value`/`encode_leak` return/trap on error (per CLAUDE.md error-handling rules -- no silent fallbacks), reserve zero-length exclusively for the explicit `filter = None` case, and return `Error::user("protocol violation ...")` (not `None`) when host bytes fail to decode.

### 5. Documented scalar "purity guarantee" is a token-match lint, trivially bypassed by a type alias
- File: enforcement in `crates/shamir-sdk-macros/src/lib.rs:425-432` (`type_contains_ctx`); the guarantee is asserted in this crate's docs at `src/context.rs:15-16`, `src/db.rs:17-18`, `src/prelude.rs:14`
- Severity: low
- Issue: `#[scalar]` rejects only argument types whose *token string* contains the exact segment `Ctx`. `type CtxAlias = shamir_sdk::Ctx;` followed by `fn f(p: Params, c: CtxAlias)` passes the check, and the generated scalar export happily wires a `Ctx::new()` -- the alias path hands a "pure" scalar the full capability object (`db`, `call`, `http_fetch`). The real boundary is host-side per-invocation gating (db gateway only via `invoke_function_in_db`; missing net gateway traps -- `context.rs:92-94`, `shamir-wasm-host/src/wasm/wasm_function.rs:138-142`), which stays intact, so this is a documented-guarantee-vs-mechanism gap, not an exploitable escalation -- but the docs present it as a guarantee, which will mislead security reviewers and guest authors alike.
- Failure scenario: A marketplace "pure scalar" uses the alias to read globals/db when a misconfigured host wires the gateway for all kinds; reviewers who trusted the documented guarantee have no second line of defense in the SDK.
- Suggested fix: Either enforce structurally (scalars receive a zero-capability token type; only `#[procedure]`/`#[function]` generation constructs the capability-carrying `Ctx`) or reword the docs in this crate and the macros to "compile-time lint; runtime isolation is enforced by host gateway wiring", and add a macros test covering the alias bypass.

### 6. User `Err` results surface as WASM panics mapped to `FunctionError::Compute`, blurring crash vs. user error
- File: `crates/shamir-sdk/src/__rt.rs:64-69` (`trap` = `panic!`); generated match at `shamir-sdk-macros/src/lib.rs:271-273, 398-400` (self-acknowledged `TODO(slice 4)` at :251); host mapping `shamir-wasm-host/src/wasm/wasm_function.rs:593-600`
- Severity: low
- Issue: A deliberate `Error::user(...)` is funneled through `panic!("shamir function error: {msg}")` and re-typed by the host as `FunctionError::Compute("shamir_call trap: panicked at ...")`, indistinguishable from a genuine guest crash (bad for caller-visible semantics, audit logs, and retry policy). The macros crate already carries a TODO to fix this; flagging because it sits on this crate's error-boundary contract (`__rt::trap` is the mechanism) and deviates from CLAUDE.md's "avoid `panic!` outside invariant violations" rule.
- Failure scenario: An operator debugging an authorization-style rejection ("missing parameter: token") sees `Compute trap: panicked at ...`, chases a phantom engine bug, and cannot distinguish user errors from real crashes in monitoring.
- Suggested fix: Encode the `Err` through the normal result channel (a `[false, message]` envelope like `http_fetch` already uses) and have the host map it to `FunctionError::User`; keep `trap` for actual invariant violations.

### 7. `leak_result` truncates pointers on 64-bit host targets (latent UB in the host-testing ABI path)
- File: `crates/shamir-sdk/src/__rt.rs:25-30`
- Severity: low
- Issue: `bytes.as_ptr() as usize as u64 << 32` assumes 32-bit pointers. It is only correct under `wasm32`; on the x86_64 host target -- which this crate explicitly supports for testing (`lib.rs:15-16`) and on which the macro-generated `shamir_call` also compiles -- the upper 32 bits of the real pointer are silently dropped, so any host-side consumer unpacking `(packed >> 32)` gets garbage and `from_raw_parts` on it is UB. Today the only unpacker is the wasm host (`wasm_function.rs:567`) where guest pointers are genuinely 32-bit, so this is latent, but nothing gates the function to `wasm32`.
- Failure scenario: Someone writes a host-target test harness that calls the generated `shamir_call` and unpacks the packed result; dereferencing the truncated pointer crashes or corrupts the test process.
- Suggested fix: `#[cfg(target_arch = "wasm32")]`-gate `leak_result` (mirror `host_imports`' `host_only()` panic on other targets), or build the packed value from explicit `as u32` casts so the truncation is declared rather than accidental.

### 8. Unbounded per-call buffer leaks in the guest ABI are undocumented as a resource budget
- File: `crates/shamir-sdk/src/host_imports.rs:55-66` (`encode_leak`); plus the macro-generated `shamir_alloc` ("never freed -- the WASM module is short-lived", `shamir-sdk-macros/src/lib.rs:105-114`)
- Severity: nit
- Issue: Every host call leaks its request buffer (and `shamir_alloc` leaks every response buffer). Within a single invocation, a legitimate per-row pattern (`for row { ctx.db().table(..).get(..) }` over 100k rows) grows linear memory without bound until `memory.grow` fails -> trap. The host's memory limits and short-lived stores contain this, and bump-allocation is the standard WASM-SDK trade-off, but the SDK documents the mechanism nowhere as a per-call cost or budget.
- Suggested fix: Document the leak-per-host-call cost and the reliance on host memory caps (or export an arena-reset import so the host can reclaim between calls).

### 9. `HttpResponse::from_value` truncating status cast and lenient header/body parsing
- File: `crates/shamir-sdk/src/http.rs:130-137` (`*n as u16`), `:139-153` (malformed headers/body silently -> empty)
- Severity: nit
- Issue: Host-controlled input, so not directly exploitable, but the boundary decode is maximally lenient: a status of `-1` or `70000` wraps via `as u16`, and wrong-typed `headers`/`body` quietly become empty rather than errors -- hiding wire-format drift at exactly the place guests consume untrusted remote data.
- Suggested fix: Range-check status (`u16::try_from`), and return `Error::user` on wrong-typed `headers`/`body` instead of defaulting.

### 10. Security-relevant HTTP envelope decoding has zero test coverage
- File: `crates/shamir-sdk/src/http.rs:24-44` (`decode_fetch_envelope`), `:124-160` (`from_value`); test inventory `src/tests/` (value + validation only), `tests/*_compile_pass.rs` (compile-only)
- Severity: low
- Issue: The existing tests are excellent for msgpack conformance (`value_tests.rs` bidirectional host/guest checks) and `Validation` shape (`validation_tests.rs`), but the egress response-boundary code -- `[ok, payload]` envelope decoding, error-message extraction, status/headers/body coercion -- has no tests at all, and `params.rs`/`db.rs` wrappers are similarly uncovered. Per the repo's TDD protocol, the boundary parsing most exposed to remote data should not be the untested part.
- Suggested fix: Add `src/tests/http_tests.rs` covering: ok/failure envelopes, non-List/wrong-shape envelopes, missing status, non-string headers, `status` out of `u16` range, and empty-body responses.

## Notes (no finding)

- No timing side-channels: the crate never compares secrets or credential material (nothing to constant-time); `Params::get`'s linear string scan handles only non-secret parameter names.
- `Value`'s recursive `Deserialize` is depth-bounded by rmp-serde's default recursion limit, so deeply nested host payloads cannot blow the guest stack via this path.
- The host side of the ABI is properly defensive in both directions (`read_guest_mem` bounds checks guest-provided `ptr`/`len`, `wasm_function.rs:567-574` validates the guest result pointer against memory size before slicing) -- the guest-side gap in Finding 3 is the mirror image, not a live hole.

</details>
