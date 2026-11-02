# TOKEN-T3b-2 design and implementation record

## Design

- `RestGate` verifies only the single strict Bearer header using `TokenService::verify(..., Endpoint::Rest, now)`; verification class distinctions collapse to unauthorized, while channel/plaintext policy failures use their TOKEN_API codes. `still_valid` rechecks token mirror revocation, expiration and owner floor without storage.
- `RestHandler` is the T3b-3 constructor entry point; it requires the concrete `Arc<ShamirDbHandler>` for the typed decode-once seam and returns `(RestGate, RestHandler)`. It creates a random token session with the verified roles snapshot, applies alias caps and a closed REST allowlist, then delegates to the existing native handler path. Response-byte guard is carried in `ExecOutcome::hold` from `byte_budget::run_with_handoff`.
- The HTTP transport maps executor rate-limit/unavailable statuses to `429`/`503` and adds conservative `Retry-After: 1` because `ExecOutcome` has no retry duration.
- Auth-failure subnet rate limiter is not wired: the brief's requested constructor inputs omit the existing limiter, and the T2b service currently has no failure-audit/rate-limit API. The limiter is demonstrated in a small source test only; it does not protect the production gate.

## Progress

- Read full committed T3b-2 brief, parent T3b, TOKEN_API §§1–9/11/13, REST/WS design, transport traits/server service, TokenService/token core/owner floors/counters, native `RequestHandler`, `Session::for_token`, post-auth bucket, TokenSessionStore, batch visitor/classifier/coarse gate, project `CLAUDE.md`, and Rust Intel SKILL.md plus security module.
- Added `TokenService::still_valid`, gate, REST scope classifier, typed handler seam, executor, focused gate/scope tests, and executor status `Retry-After` transport behavior.
- Formatting and layout checks passed. No compile, build, clippy or tests were executed as required.
- End-to-end review found the narrow test set does not yet cover all allowed operations, full failure table, owner-floor/revoke/expiry, all status mapping or error/log redaction. No audit event writer use and no production auth-failure limiter are wired in this slice. T3b-3 must retain concrete handler separately from its trait object and pass TokenService/clock/username provider/rails into `RestHandler::new`.

## Final report

### Scope status

- PARTIAL — gate: `/crates/shamir-server/src/security/perimeter/http_rest/gate.rs:58` implements TokenGate, strict Bearer parsing, service verification, mapped channel/plaintext denials, uniform unauthorized, and cached validity. Missing production failure limiter, audit queue/sampling, 429 failures and complete gate tests.
- PARTIAL — executor/allowlist: `/crates/shamir-server/src/security/perimeter/http_rest/executor.rs:64` implements RestExecutor, decode-once/alias cap, session principal/roles snapshot, native handler delegation, status/body and hold. `/crates/shamir-server/src/security/perimeter/http_rest/scope.rs:12` classifies requests and nested batch leaves exhaustively. `TxBegin` read-only allowance is not implemented (all transaction requests are REST-not-supported per §7); allowlist/property-specific operation tests are incomplete. `AccessTree` database scope is checked; typed admin fields are not otherwise transformed.
- PARTIAL — observability/audit: executor records `shamir_token_request_duration_seconds{endpoint=rest}`; gate records `shamir_token_auth_total{channel,result}` with fixed values and non-secret warning. No T2b failure audit/sampling writer API is connected; no counter-backed metric sampling/registration pass.
- DONE — native handler seam: `/crates/shamir-server/src/engine/db_handler/handler.rs:352-374,378-571` decodes native bytes once and uses shared typed dispatch, preserving the native coarse/per-operation gates.
- DONE — transport Retry-After: `/crates/shamir-transport-http/src/server/service.rs:294-308` emits `Retry-After: 1` on executor `429/503`.
- DONE — design note exists at `/docs/dev-artifacts/audits/2026-11-01-token-t3b2-gate-executor/DESIGN.md`.

### Tests added (not run)

- `token_gate_returns_uniform_unauthorized_for_all_credential_failures`
- `read_write_token_is_forbidden_on_remote_plaintext_without_rail`
- `auth_failure_subnet_limiter_is_available_without_scram_lockout` (API availability only; not gate integration)
- `every_rest_top_level_request_is_explicitly_classified`
- `database_scope_denies_outside_and_allows_inside`
- `read_only_mode_rejects_writes_only_after_allowlist_classification` (currently exercises a denied admin operation; does not prove write-mode result)

### New public items

- `RestGateRails` and its public booleans
- `RestGate::new`
- `RestGate` (implements transport `TokenGate`)
- `RestHandler::new` and `RestHandler` (implements transport `RestExecutor<Verified>`)
- `TokenService::still_valid`

### Files read end-to-end / source review

Task brief, parent brief, TOKEN_API, REST/WS design note, `CLAUDE.md`, Rust Intel SKILL.md and security module; transport `gate.rs` and `server/service.rs`; token service/model/owner adapter and token record/verify/mirror; Session, TokenSessionStore and dispatch interfaces; DbRequest, BatchOp, BatchTreeVisitor, native handler and coarse admin gate; byte-budget handoff; relevant config/dependency declarations. Source review only; no compilation claim.

### T3b-3 wiring / CI

- Keep `Arc<ShamirDbHandler>` alongside `Arc<dyn RequestHandler>` and use the concrete clone for `RestHandler::new`.
- Provide owner->username lookup, clock, listener plaintext rails and shared TokenService; wire gate and executor into the existing transport serve call.
- CI must compile and lint all changed crates, run gate failure matrix and full allowlist tests, add response mapping/redaction/audit coverage, and integration-test listener body auth/revocation/hold lifetimes. Specifically test every BatchOp allow/deny, nested Batch/ForEach, database-scoped List/AccessTree, mode boundaries, owner floor, expired/revoked/channel cases, and verify token-shaped secrets are absent from logs/audit/errors.

### Changed files

- `/crates/shamir-server/src/security/mod.rs`
- `/crates/shamir-server/src/security/perimeter/mod.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/mod.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/gate.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/executor.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/scope.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/tests/mod.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/tests/gate_tests.rs`
- `/crates/shamir-server/src/security/perimeter/http_rest/tests/scope_tests.rs`
- `/crates/shamir-server/src/security/tokens/service.rs`
- `/crates/shamir-server/src/engine/db_handler/handler.rs`
- `/crates/shamir-transport-http/src/server/service.rs`
- `/docs/dev-artifacts/audits/2026-11-01-token-t3b2-gate-executor/DESIGN.md`

## Final review update (01:23 CET)

- Corrected duplicate `ScopedHandler` declaration, added the `OwnerName` closure type alias, and fixed DashMap vacant-entry cloning by cloning the inserted entry's value.
- Gate failure test now varies IPv4 /24 subnets (rather than hosts within one subnet), so the separate test inputs do not trip the same subnet rate bucket. `AllowedOperation` re-export is test-only.
- **PARTIAL — not ready to merge.** REST gate/executor, scope classifier, native decode-once seam, response-budget handoff and transport status/retry support added. Still missing queued/sampled audit and exposed-token event, limiter lifecycle/shared wiring, metrics registration/redaction validation, and the required test matrix. Compilation, clippy and tests were not run; scoped formatting, layout and whitespace checks passed. Cargo manifests/lockfile and listener/config/launch files untouched. T3b-3 must retain concrete handler and wire constructor/rails/clock/owner lookup after remaining slice gaps are resolved.
