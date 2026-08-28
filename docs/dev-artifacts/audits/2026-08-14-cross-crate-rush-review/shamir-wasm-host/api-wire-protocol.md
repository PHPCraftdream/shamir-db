# shamir-wasm-host -- API & wire-protocol design

## Summary

The guest ABI (packed `ptr<<32|len` returns, `0 = absent`, msgpack `QueryValue` payloads) is consistent between host and `shamir-sdk`, the import surface is a single auditable `SANCTIONED_HOST_IMPORTS` const kept in sync with the linker by a dedicated test, and the crate is fully builder-rule compliant (no `serde_json` anywhere; queries/filters stay opaque msgpack delegated to the guest's query builder). The main wire-protocol weakness is the HTTP egress codec: headers are encoded as a string-keyed map, which silently collapses duplicate headers (`Set-Cookie`) on both request and response sides, and the codec's strictness is asymmetric (strict `method`/`url`, silently-defaulting `headers`/`body`). Secondary issues are security-relevant doc/contract mismatches (`CreateFunctionOptions::net_grants`, `FnCtx::global_get` secret gating), stringly-typed errors across the public gateway traits, and an inconsistent trap-vs-envelope error contract across sibling host imports.

## Findings

### 1. HTTP wire codec collapses duplicate headers (`Set-Cookie` loss on both directions)
- **File:line:** `crates/shamir-wasm-host/src/wasm/host_http.rs:86-97` (encoder), `:39-65` (decoder); peer codec `crates/shamir-sdk/src/http.rs:97-111`, `:139-148`
- **Severity:** high
- **Issue:** `encode_http_response` serialises response headers into a `QueryValue::Map` (string-keyed `TMap`), so repeated header names from a real HTTP response — most importantly multiple `Set-Cookie`, but also duplicated `WWW-Authenticate`/`Via` — are silently collapsed to the last value before the guest ever sees them. The request decoder accepts headers as Map *or* List-of-pairs, but the SDK's `HttpRequest::to_value` emits a Map, so guest-set duplicate request headers are collapsed guest-side too. The wire shape (`Map<Str,Str>`) cannot represent valid HTTP traffic, and the loss is silent and unfixable from the guest.
- **Failure scenario:** a function calls an API that responds `Set-Cookie: a=1` + `Set-Cookie: b=2` (session + CSRF token); the guest's `resp.headers()` contains only one cookie; cookie-jar auth silently breaks with no error anywhere.
- **Suggested fix:** encode response headers as `QueryValue::List` of `[name, value]` pairs (the decoder already accepts this shape for requests); switch the SDK encoder to the same list shape; keep accepting the Map form for back-compat but document it as legacy/deprecated, and add a round-trip test with duplicate header names.

### 2. `CreateFunctionOptions` doc states the opposite of the actual empty-`net_grants` semantics
- **File:line:** `crates/shamir-wasm-host/src/meta.rs:185-210` (vs. `:79-92` same file, and consumer `crates/shamir-db/src/shamir_db/shamir_db/core.rs:805-851`)
- **Severity:** medium
- **Issue:** the options-bag doc says "no net grants (empty `net_grants` = full DB-wide `net_allowlist`, see `FunctionMeta::net_grants`)" — but the doc it points to says the exact opposite ("EMPTY/absent `net_grants` means NO egress", task #609), and the implementation (`build_net_gateway`: `Some(grants) if grants.is_empty() => Vec::new()`, plus the `net_grants_empty_denies_all_egress` test) confirms restrictive-by-default. The `CreateFunctionOptions` comment is stale pre-#609 text.
- **Failure scenario:** an operator or reviewer reading the public options docs concludes user functions get full DB-wide egress by default and reasons about the security posture (or downstream ports/re-implements the option) from the wrong contract.
- **Suggested fix:** rewrite the `CreateFunctionOptions` doc to match #609 (empty = deny all egress; non-empty = intersect with the DB ceiling); the doc already names `FunctionMeta::net_grants` as the reference — make it agree with it.

### 3. `FnCtx` docs promise secret-grant gating on `global_get` that only the WASM host import enforces
- **File:line:** `crates/shamir-wasm-host/src/context.rs:289-297` and `:390-397` (docs) vs. `:426-428` (ungated `FnCtx::global_get`); scope note in `src/wasm/host_globals.rs:17-24`
- **Severity:** medium
- **Issue:** the `FnCtx` type doc ("`global_get(\"env.X\")` returns absent when `X` is not in `secret_grants`") and `with_secret_grants`'s doc ("Only env variable names listed here can be read via `global_get`") attribute the enforcement to `FnCtx::global_get`. It is not there: `FnCtx::global_get` reads `GlobalVars` unguarded; gating exists only in the guest-facing `shamir_host::global_get` import. `host_globals.rs` explicitly documents this split, but the `FnCtx` docs contradict it — the public native API's documented contract is not its implemented contract.
- **Failure scenario:** a native (compiled-in) `ShamirFunction` author relies on the documented `ctx.global_get("env.X")` gating (or on grants making secrets "absent") and gets the secret anyway; conversely a security audit of the native path reads the wrong guarantee.
- **Suggested fix:** either enforce the grant check in `FnCtx::global_get`/`global_keys` (making the docs true for both native and guest paths), or correct the `FnCtx` docs to state that `secret_grants` are enforced only at the guest host-import boundary and that `FnCtx::secret_grants()` is provided for native impls to self-enforce.

### 4. Stringly-typed errors across the public gateway traits and egress guards
- **File:line:** `crates/shamir-wasm-host/src/db_gateway.rs:56-87`; `crates/shamir-wasm-host/src/net_gateway.rs:55-61, 69, 110, 157-209`
- **Severity:** medium
- **Issue:** `DbGateway::{get,insert,query,execute}` and `NetGateway::fetch` return `Result<_, String>`, and the exported guard fns (`check_host_allowed`, `check_url_allowed`, `check_url_allowed_resolved`) are `Result<_, String>`. This is a library crate whose own `FunctionError` (`thiserror`) is the house style per CLAUDE.md ("`thiserror` for library error enums"); the gateway boundary discards all structure — callers (and the host imports that re-wrap them into trap messages) cannot distinguish deny-vs-unavailable-vs-transport failure, and no variant can be added without string-format coupling.
- **Failure scenario:** a `db_execute` batch failure's structured error codes become a formatted trap string (`format!("db_execute: {e}")`); a guest or embedder wanting to retry on timeout but fail on allowlist denial must substring-match English error text.
- **Suggested fix:** introduce a small `thiserror` enum per gateway (e.g. `DbGatewayError`, `NetGatewayError::{Denied, DnsBlocked, Transport}`) with `Display` used only at the trap/format boundary; keep the `String`-returning fns as thin wrappers if external callers depend on them.

### 5. Inconsistent guest-facing error contract across sibling host imports (envelope vs uncatchable trap)
- **File:line:** `crates/shamir-wasm-host/src/wasm/host_http.rs:99-114` (catchable envelope) vs. `src/wasm/host_db.rs:12-58, 160-190` and `src/wasm/host_call.rs:19-27` (traps)
- **Severity:** medium
- **Issue:** `http_fetch` deliberately returns runtime failures as a catchable `[false, "error"]` envelope and traps only for config bugs, while `db_get`/`db_insert`/`db_query`/`db_execute`/`call` trap on every gateway failure. Within one ABI, identical failure classes (denied, not-found-at-runtime, transport error) are catchable for HTTP and fatal for DB. The SDK docs do say "Traps on error", but as API design the asymmetry means guest code can gracefully handle an egress failure yet cannot handle a `db_execute` batch rejection — on `wasm32-unknown-unknown` with `panic=abort`, a trap terminates the whole function invocation.
- **Failure scenario:** a function runs a validation batch via `db_execute` that fails a uniqueness check; the guest cannot inspect the failure or return `FunctionError::User`-style feedback — the entire invocation traps as `Compute`, and the wire client sees an opaque host error instead of a structured batch error.
- **Suggested fix:** adopt the `http_fetch` envelope convention for `db_execute` at minimum (its `BatchResponse` already has an error channel — return it as payload instead of converting to a trap), and document the per-import error contract in one place (the `wasm_function.rs` ABI doc block).

### 6. `decode_http_request` silently coerces malformed `headers`/`body` to empty while `method`/`url` are strict
- **File:line:** `crates/shamir-wasm-host/src/wasm/host_http.rs:39-70`
- **Severity:** medium
- **Issue:** a `headers` value of the wrong shape and a `body` that is not `Bin` (e.g. the very plausible `Value::Str` body) are silently replaced with empty defaults (`_ => Vec::new()`), whereas `method`/`url` of the wrong type are hard errors. A decoded-but-wrong request is sent with no body/headers instead of failing the fetch.
- **Failure scenario:** a guest builds `{"method": "POST", "url": ..., "body": Str(json)}`; the host sends a body-less POST; the remote API returns 400/empty and the function's error handling blames the remote service — the actual protocol mistake is invisible.
- **Suggested fix:** reject non-`Bin` `body` and wrong-shaped `headers` with the same `Err` used for `method`/`url` (or explicitly accept `Str` body via UTF-8 encoding, but then document it); add codec unit tests for the malformed-input matrix.

### 7. `compile_rust_source` hardwires the SDK path to the build machine's `CARGO_MANIFEST_DIR`
- **File:line:** `crates/shamir-wasm-host/src/compile.rs:484-497`
- **Severity:** medium
- **Issue:** the public `compile_rust_source`/`compile_rust_source_with_timeout` API resolves `shamir-sdk` via `env!("CARGO_MANIFEST_DIR")/../shamir-sdk` with no parameter or environment override. The compiled binary retains a path into the developer's source tree; on any deployment where that layout doesn't exist, every `CREATE FUNCTION ... SOURCE` fails at `canonicalize` with `resolving sdk path`. The function is public API whose only working environment is a dev checkout, and that constraint is undocumented.
- **Failure scenario:** the single shipped binary (project goal: self-contained, no external runtime deps) is installed on a server; the first user attempts a source-based function and gets `resolving sdk path: ...` with no recourse.
- **Suggested fix:** allow an override (function parameter or `SHAMIR_SDK_PATH`-style env checked before the manifest-relative default), and document in the function's doc comment that the default only works in a source checkout.

### 8. `ResolvedPin::pinned_ips` uses an empty-Vec sentinel for "do not pin"
- **File:line:** `crates/shamir-wasm-host/src/net_gateway.rs:119-131, 165-175`
- **Severity:** low
- **Issue:** "empty means do not pin" overloads a `Vec` with a second meaning (exact-allowlist path). An `Option<Vec<IpAddr>>` (`None` = no pin) would make the two paths unambiguous at the type level; as written, a future caller that forgets the sentinel treats "no pin" and "validated set of IPs" uniformly and may pin nothing when it believed it had validated addresses.
- **Suggested fix:** change `pinned_ips: Vec<IpAddr>` to `Option<Vec<IpAddr>>` (pre-release, no compat constraint), or at minimum rename/document the sentinel at the type.

### 9. `glob_matches` duplicated in two security-relevant matchers
- **File:line:** `crates/shamir-wasm-host/src/env_policy.rs:75-106` and `crates/shamir-wasm-host/src/net_gateway.rs:487-514`
- **Severity:** low
- **Issue:** the `*`-glob matcher is copy-pasted between `EnvPolicy` and the egress allowlist (the latter's comment even claims it "reuses the same logic as EnvPolicy" — it doesn't; it's a duplicate). A future fix to one (e.g. the unanchored-middle-segment behaviour) silently leaves the other behind, diverging env-seeding policy from egress policy semantics.
- **Suggested fix:** hoist one `pub(crate) fn glob_matches` into a small shared module and have both call it (the `net_gateway.rs` copy already claims to be "the same logic").

### 10. Catalogue record decoding has silent fallbacks and no format versioning
- **File:line:** `crates/shamir-wasm-host/src/meta.rs:110-148`
- **Severity:** low
- **Issue:** `FunctionMeta::from_record` silently coerces unknown `visibility`/`security` strings to Private/Invoker and silently drops non-string entries in `secret_grants`/`net_grants` (`filter_map`). The fallback direction is fail-safe, but there is no schema/version marker on the persisted record, so a future enum variant (or a corrupt field) is indistinguishable from a default — a newer node's `Security` variant read by an older node silently downgrades with nothing in logs, and corrupt grant arrays truncate silently.
- **Suggested fix:** at minimum `log::warn!` on any fallback/dropped-entry path; consider a `format_version` field injected by `inject_into` so forward-compat decisions are explicit.

### 11. Wire codecs, `db_*` host imports, and the `call` depth limit have no in-crate tests
- **File:line:** `crates/shamir-wasm-host/src/tests/` (whole tree; cf. `src/wasm/host_http.rs`, `src/wasm/host_db.rs`, `src/wasm/host_call.rs:96-101`)
- **Severity:** low
- **Issue:** the `tests/` directory is otherwise exemplary (sanitizer↔linker sync test, SSRF/inet_aton matrix, aggregate-fuel, compile-timeout), but nothing in-crate exercises `decode_http_request`/`encode_http_response` (the exact shape contract from findings 1/6), the `db_get/insert/query/execute` borrow-dance imports, or the `next_depth > depth_limit` trap (the nested-call tests set `depth_limit(1000)` so fuel always exhausts first). These contracts are only pinned by integration tests elsewhere (if at all), which is how findings 1 and 6 survived unnoticed.
- **Suggested fix:** add `tests/host_http_wire_tests.rs` (encode/decode round-trips incl. duplicate headers and malformed shapes), a depth-limit trap test, and a `db_query` zero-length-filter≡`None` test.

### 12. Internal audit-tracking references leaked into public API docs
- **File:line:** `crates/shamir-wasm-host/src/net_gateway.rs:105-109, 118, 133, 148, 224` (e.g. "see finding 2c", "finding 2c DNS-rebind TOCTOU fix")
- **Severity:** low
- **Issue:** exported items (`check_url_allowed`, `check_url_allowed_resolved`, `ResolvedPin`) carry doc comments referencing "finding 2c" — identifiers from an internal audit that mean nothing to a crate consumer reading generated docs.
- **Suggested fix:** keep the substance (the TOCTOU/pinning explanation is genuinely good) but phrase it without the audit-tracking shorthand, or move the tracking references to non-doc comments.

### 13. Duplicated doc-comment block on `host_call`
- **File:line:** `crates/shamir-wasm-host/src/wasm/host_call.rs:16-27`
- **Severity:** nit
- **Issue:** the summary paragraph ("Host implementation of `call(...)` ... `FunctionError::Compute`.") appears twice back-to-back in the same doc comment — a copy-paste remnant that renders duplicated text in rustdoc.
- **Suggested fix:** delete the duplicated paragraphs.

### 14. Unused `serde` dependency in Cargo.toml
- **File:line:** `crates/shamir-wasm-host/Cargo.toml:14`
- **Severity:** nit
- **Issue:** `serde = { version = "1.0.217", features = ["derive"] }` is declared but no source file references `serde`; the wire format is msgpack via `shamir-types::QueryValue`. (For the record: the absence of `serde_json`/`json!` anywhere in the crate also means the builder-only query-construction rule is satisfied by construction.)
- **Suggested fix:** drop the dependency (or annotate why it must stay).
