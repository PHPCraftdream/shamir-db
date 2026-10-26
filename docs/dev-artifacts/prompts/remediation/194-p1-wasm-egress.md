# P1 stage G4 — p1-wasm-egress

## Goal

WASM egress/permission guards: SSRF ranges, globs, compiler gate, http decode. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G4): The SSRF guard blocks 0.0.0.0 and every non-routable range; one shared, correct glob matcher; `net_grants` default documented once and fail-closed; `FnCtx` docs match enforcement; validator source creation requires the WasmCompiler permission; malformed http headers/body are errors; gateway errors are typed.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-WASM-HOST-E62F8E2C — SSRF guard misses `0.0.0.0` (and other non-routable IPv4 ranges) — wildcard allowlist reaches loopback services

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wasm-host/src/gateway/net_gateway.rs:459; crates/shamir-wasm-host/src/gateway/net_gateway.rs:186; changed by: none
- R0 note (includes the fix site): OBSERVED: literal and resolved-address guards still share an IPv4 predicate that omits unspecified addresses; wildcard matching therefore leaves 0.0.0.0 unclassified, unlike IPv6 unspecified. Actual localhost connection behavior remains unverified, and other special ranges require explicit policy. Fix site: reject IPv4 unspecified destinations in the shared predicate in crates/shamir-wasm-host/src/gateway/net_gateway.rs.

### RV-WASM-HOST-1CF6A3BC — `glob_matches` duplicated in two security-relevant matchers

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/gateway/env_policy.rs:75; crates/shamir-wasm-host/src/gateway/net_gateway.rs:487; changed by: none
- R0 note (includes the fix site): OBSERVED: environment inclusion and network authorization still call separate implementations of the same star matcher, both retaining first-occurrence suffix handling. Fix site: consolidate the implementations in crates/shamir-wasm-host/src/gateway/env_policy.rs and crates/shamir-wasm-host/src/gateway/net_gateway.rs without preserving their suffix defect.

### RV-WASM-HOST-935F15D1 — Both glob matchers reject valid repeated-suffix matches

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/gateway/env_policy.rs:90; crates/shamir-wasm-host/src/gateway/net_gateway.rs:500; changed by: none
- R0 note (includes the fix site): OBSERVED: both algorithms select the first final-literal occurrence and then require end-of-text. INFERRED directly from those branches: *_KEY rejects X_KEY_KEY despite a valid terminal match; no execution was performed. Fix site: repair final-segment anchoring in both gateway matcher files, retaining a repeated-suffix oracle.

### RV-WASM-HOST-5377DABE — Contradictory fail-open vs fail-closed documentation of the `net_grants` default

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/builtin/meta.rs:188; crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:349; changed by: none
- R0 note (includes the fix site): OBSERVED: CreateFunctionOptions still says empty net_grants grants the full DB allowlist, while the effective gateway explicitly converts empty user-function grants into deny-all; the builtin fallback is a separate branch. Fix site: correct the default-policy documentation in crates/shamir-wasm-host/src/builtin/meta.rs.

### RV-WASM-HOST-57CD974C — `FnCtx` docs promise secret-grant gating on `global_get` that only the WASM host import enforces

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/runtime/context.rs:291; crates/shamir-wasm-host/src/runtime/context.rs:447; changed by: none
- R0 note (includes the fix site): OBSERVED: the type and builder still promise secret-grant filtering, but native global_get directly reads globals; only the WASM binding checks grants. Trusted native ownership limits the security scope but does not resolve the documentation mismatch. Fix site: reconcile the promise and native getter contract in crates/shamir-wasm-host/src/runtime/context.rs.

### RV-WASM-HOST-9D8C5F97 — Validator source creation bypasses the separate WasmCompiler permission gate

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs:194; crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs:215; changed by: none
- R0 note (includes the fix site): OBSERVED: actor-aware source creation still reaches compilation without WasmCompiler Execute authorization; offloading changes scheduling, not permission enforcement. The prior ordinary-user ForEach wire witness is refuted by current recursive is_admin classification, but the public actor-aware facade remains exposed. Fix site: crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs must authorize the Source branch before compilation.

### RV-WASM-HOST-D0B90CF6 — `decode_http_request` silently coerces malformed `headers`/`body` to empty while `method`/`url` default to `""`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-wasm-host/src/wasm/bindings/http.rs:26; crates/shamir-wasm-host/src/wasm/bindings/http.rs:64; changed by: none
- R0 note (includes the fix site): OBSERVED: missing method/url still default to empty strings, unsupported headers/body containers still become empty, and the live import dispatches the decoded request; #290 changed response headers, while #273's downstream control-character guard does not validate these types. Fix site: reject malformed or missing required fields in decode_http_request in crates/shamir-wasm-host/src/wasm/bindings/http.rs.

### RV-WASM-HOST-DF4AD1DA — Stringly-typed errors across the public gateway traits and egress guards

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-wasm-host/src/gateway/db_gateway.rs:60; crates/shamir-wasm-host/src/gateway/net_gateway.rs:60; changed by: none
- R0 note (includes the fix site): OBSERVED: public DB and network gateway traits still return String errors, as do egress guards, leaving callers without typed failure classification or error sources. Fix site: define and propagate typed gateway failures at crates/shamir-wasm-host/src/gateway/db_gateway.rs and crates/shamir-wasm-host/src/gateway/net_gateway.rs.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
