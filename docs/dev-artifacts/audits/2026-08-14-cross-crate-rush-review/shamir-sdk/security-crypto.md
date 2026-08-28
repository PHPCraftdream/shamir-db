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
