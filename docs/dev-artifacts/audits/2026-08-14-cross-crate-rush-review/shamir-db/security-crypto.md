# shamir-db -- Security & crypto boundary

## Summary

The crate delegates all password/SCRAM crypto to the injected `UserAdminPort` (Argon2id lives in the embedder, plaintext passwords enter only via `SecretString::reveal()`), contains zero `unsafe` blocks, and consistently applies fail-closed `resource_meta` resolution, auth-before-existence-probe (#995), path-traversal name validation (`validate_name_component`), and a DNS-rebind-pinned SSRF guard in the curl egress gateway. The main defects are at the untrusted-input boundaries: guest-controlled HTTP header values can inject raw newlines into the curl config file (`escape_curl_value` handles only `` and `"`), the ambient interner-delta attach leaks any repo's field-name dictionary without the Store-level Read check that `InternerDump` requires, and the validator Rust-source path triggers a host `cargo` compile while skipping the `WasmCompiler` Execute gate that task #607 established for the identical function-path operation. No timing-sensitive comparisons exist in this crate (no HMAC/SCRAM/TLS code lives here), so nothing to flag on that axis.

## Findings

### 1. Guest-controlled header values/method can inject arbitrary curl config directives (CRLF injection)

- **File:line:** `crates/shamir-db/src/shamir_db/curl_gateway.rs:83-89` (header interpolation), `:71-75` (url/method), `:210-220` (`escape_curl_value`)
- **Severity:** high
- **Issue:** `escape_curl_value` escapes only `` and `"`. Curl config files have no escape for line breaks, so a raw `\n` (or `\r`) inside a value terminates the config line and everything after it is parsed as a *new top-level directive*. `HttpRequest.headers` / `HttpRequest.method` are guest-controlled (WASM functions run third-party logic; a function's own request data is attacker-influenced), and header names/values reach the config file unfiltered for CR/LF. Quoted values can't be closed by the attacker (the `"` is escaped), but unquoted directive values need no quotes at all.
- **Failure scenario:** A guest calls `ctx.http_fetch()` with a header value containing `\nproxy = http://<internal>:8080\n`. curl then routes the request through the attacker-chosen proxy, bypassing the SSRF guard's `--resolve` IP pinning (which constrains *destination resolution*, not proxying) — enabling access to loopback/metadata endpoints and TLS-intercepted egress. Other injected directives (`output = <path>`, `config = <file>`) give file-write redirection and nested-config loading. The URL path is largely protected (the WHATWG parser used by the guard strips tab/newline), but the header/method path is not. `curl_gateway_tests.rs:6-18` covers only `` and `"` — the injection class is untested.
- **Suggested fix:** Reject (not escape) CR, LF, and other C0 control chars in every guest-supplied string written into the config file (url, method, header names, header values) before `escape_curl_value`, returning a typed egress error. Add a unit test asserting a newline-bearing header value is rejected outright, and one asserting the generated `curl.cfg` never contains a raw `\n` inside a value.

### 2. Ambient interner delta exposes any repo's field-name dictionary without Store-level authorization

- **File:line:** `crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:22-57`; called from `db_execute.rs:98-102`
- **Severity:** medium
- **Issue:** `execute_as` attaches `response.interner_delta` for every `(repo, epoch)` pair in the client's `request.interner_epochs`, resolving each repo's interner and returning its full id→name dictionary after the client-supplied epoch. The only authorization in `execute_as` is Database-level Read plus per-op table checks; the ambient attach performs **no** `authorize_access` on the named repo. The explicit admin op for the same data, `handle_interner_dump` (`admin_interner.rs:50-57`), requires `Action::Read` on `ResourcePath::Store` — so the delta path is an ACL-free side door to the same resource.
- **Failure scenario:** An actor with Read on database `app` but no access to repo `app/hr` sends any batch with `interner_epochs: {"hr": 0}` and receives `hr`'s complete interned field-name vocabulary (schema shape, internal attribute names) in the response — reconnaissance data the interner-dump gate exists to protect.
- **Suggested fix:** In `attach_interner_delta`, skip repos for which `authorize_access(actor, ResourcePath::store(db, repo), Action::Read)` fails (needs the actor threaded in — it is available at the `db_execute.rs` call site), mirroring `handle_interner_dump`'s gate. Add an ACL test that a Store-read-denied actor gets no delta entries for that repo.

### 3. Validator Rust-source path bypasses the WasmCompiler Execute gate (task #607)

- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:210-229` (`create_validator_inner`, `Source` arm); wire entry `execute/admin_validator.rs:36-43`; contrast with the gated function path `shamir_db/function_management.rs:161-177`
- **Severity:** medium
- **Issue:** `create_function_with_opts_as` guards `FunctionSource::Source` behind `authorize_access(WasmCompiler, Execute)` — task #607's gate for "compiling Rust source runs a host compiler process". The structurally identical validator path, `create_validator_from_source_as` → `create_validator_inner`, calls `compile_rust_source` with **no** authorization at all, and the wire handler `handle_create_validator` checks only `Create` on `FunctionNamespace`. `wasm_compiler_permission_tests.rs` covers the function path only; nothing covers the validator path.
- **Failure scenario:** An operator hardens `WasmCompiler` to `0o700` to deny untrusted users host-side Rust compilation. A user holding FunctionNamespace-Create (default `0o777`) still triggers arbitrary host toolchain builds (`cargo` executes build scripts/proc-macros — host code execution at compile time) simply by submitting `create_validator` with `source` instead of `wasm`.
- **Suggested fix:** Add the same `authorize_access(&actor, &ResourcePath::WasmCompiler, Action::Execute)` check inside `create_validator_inner`'s `Source` arm (or in `create_validator_from_source_as` before dispatch), and extend `wasm_compiler_permission_tests.rs` with the validator analogue of `create_function_from_source_denied_under_hardened_wasm_compiler`.

### 4. Egress response body read without any size cap

- **File:line:** `crates/shamir-db/src/shamir_db/curl_gateway.rs:100` (`max-time = 30` only), `:156-167` (`read_to_end`)
- **Severity:** low
- **Issue:** The gateway caps egress by *time* but not by *size*: `read_to_end` slurps the dumped response file into memory unbounded, and no `--max-filesize` is set in the generated config.
- **Failure scenario:** A function fetching from an allowlisted (or compromised allowlisted) host receives a multi-GB response; the host process OOMs / degrades, taking down the whole DB server — a guest-triggerable amplification with no guest-visible limit.
- **Suggested fix:** Add `max-filesize = "<cap>"` to the generated config and/or stream-copy with a byte budget into a bounded `Vec` (fail with a typed egress error when exceeded), consistent with the engine's `WasmLimits` memory posture.

### 5. Dead TLS/password-hash dependencies with a stale "kept compiling" rationale

- **File:line:** `crates/shamir-db/Cargo.toml:59` (`argon2`), `:64-68` (`rustls`, `tokio-rustls`, `rcgen`)
- **Severity:** low
- **Issue:** The manifest comment says the legacy `db/net/*` TLS module is "kept compiling so the obsolete code doesn't bit-rot before its planned deletion" — but `src/net` no longer exists (glob of `src/**/*.rs` shows no net module; no `rustls`/`tokio-rustls`/`rcgen`/`argon2` symbol is referenced anywhere under `src/`). An entire TLS stack, a certificate generator, and a password-hashing crate are compiled into the facade for nothing.
- **Failure scenario:** Pure supply-chain and attack-surface cost (four heavyweight crypto crates linked into every consumer of the facade), plus a misleading manifest that suggests TLS/auth code lives here when the boundary design explicitly places it in shamir-connect/shamir-server.
- **Suggested fix:** Delete the four dependencies and the stale comment block. If a future `net` module returns, re-add with features at that point.

### 6. `ShamirDb::execute` (System-actor, ACL-bypassing) is public and undiscoverable-hidden, unlike its #606 peers

- **File:line:** `crates/shamir-db/src/shamir_db/execute/db_execute.rs:16-22`; contrast `shamir_db/db_management.rs:14-33` and `table_management.rs:142-168`
- **Severity:** low
- **Issue:** `execute()` is exactly the "never call from a wire-reachable path — it stamps `Actor::System` and bypasses every ACL" hazard class that task #606 mitigated on `create_db`/`add_repo`/`rename_table` with `#[doc(hidden)]` + SAFETY doc comments. `execute` (and likewise `tx_begin`/`tx_execute`/`tx_commit`) remains plain `pub` with the bypass only described in `execute_as`'s doc. `facade_gateway_acl_tests.rs:1-3` itself documents that `execute()` "is `execute_as(Actor::System)`".
- **Failure scenario:** An embedder wires a new request handler to `db.execute(...)` out of convenience (it is the most discoverable name) and silently grants every session the admin-bypass actor.
- **Suggested fix:** Apply the same `#[doc(hidden)]` + wire-reachability SAFETY comment to `execute`/`tx_begin`/`tx_execute`/`tx_commit` (or `pub(crate)` them if the test blast radius permits renaming to `execute_system`).

### 7. `set_net_allowlist` mutates only one clone; other `ShamirDb` clones keep the old allowlist

- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:599-605` (with `net_allowlist: Arc<Vec<String>>` declared at `core.rs:79`)
- **Severity:** low
- **Issue:** `ShamirDb` is a cheap-clone (`Arc`-fielded) type, but `set_net_allowlist(&mut self)` *replaces* the `Arc` rather than mutating shared state, so only the instance it is called on sees the new allowlist. The doc's "must be called before any function invocation" is the only guard; there is no compile-time or runtime protection against a clone being made first (the codebase's own pattern clones eagerly, e.g. `ShamirAdminExecutor { shamir: self.clone(), .. }`).
- **Failure scenario:** An operator tightens the egress allowlist at runtime through one handle; concurrently-cloned handles keep serving functions with the old (possibly broader) allowlist — silently widening egress relative to the operator's intent. The inverse (accidental tightening on a test clone) also misleads.
- **Suggested fix:** Store `Arc<ArcSwap<Vec<String>>>` (or an `AtomicBool`-gated `ArcSwapOption`) so every clone observes the same allowlist, matching the lock-free pillar; or document + assert single-clone ownership.

### 8. `wasm_hash` uses non-cryptographic FxHash and is never verified

- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:187-189`, `validator_management.rs:236-238`
- **Severity:** nit
- **Issue:** The catalogue stores a 64-bit `rustc_hash::FxHasher` digest of the WASM artifact. Today it is write-only (no consumer recomputes or compares it), so it is metadata, not a control — but if it is ever promoted to an integrity/attestation check, FxHash is trivially collidable and the single stored value is attacker-writable by anyone who can rewrite the catalogue row.
- **Suggested fix:** Comment the field as a non-security change-detection hint, or switch to SHA-256 (workspace already has crypto-capable deps elsewhere) before any consumer starts trusting it.

### 9. `SECURITY DEFINER` grants the guest the owner-actor raw DB gateway, including admin ops

- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:285-294` (raw-byte `execute` passthrough) + `access_control.rs:990-1017` (`effective_fn_actor` escalation)
- **Severity:** nit
- **Issue:** A `SECURITY DEFINER` function legitimately runs DB access as its owner, and `invoke_function_in_db_as` builds the `FacadeDbGateway` with that effective actor. The gateway's `execute(&[u8])` accepts an *arbitrary* `BatchRequest`, so the definer-owner actor applies to **all** `BatchOp`s the batch planner supports — including admin ops (chmod/chown/user-lifecycle), not just DML. With the default `owned_enforced` mode (`0o700`) a System-owned function is not invokable by ordinary users, so this requires an operator to both create a function as System and chmod it open — but that combination silently turns the function into a full-admin oracle for any caller who can Execute it. The `effective_fn_actor` doc discusses Definer escalation but not the admin-op breadth of the gateway.
- **Suggested fix:** Document (or warn at create time) that `SECURITY DEFINER` + open visibility exposes owner-privileged *admin* operations through the gateway; consider a gateway flag that restricts definer-context guests to data ops unless the owner is also the caller.

## Test-coverage notes (theme lens)

Coverage of this surface is unusually strong where it exists: `facade_gateway_acl_tests.rs` (actor-threading through the function DB gateway), `create_function_gating.rs` (secret_grants `Manage(Root)` gate), `wasm_compiler_permission_tests.rs` (#607 gate, functions only), `admin_access_validation_tests.rs` / `enforcement_tests.rs` / `sec1_ddl_gate_e2e.rs` / `enforcement_dml_e2e.rs` (ACL enforcement), and `curl_gateway_tests.rs` (SSRF `--resolve` pinning, escaping of ``/`"`). The three gaps that mirror findings 1–3: no CRLF-injection test for the curl config, no validator-path WasmCompiler test, and no interner-delta ACL test.
