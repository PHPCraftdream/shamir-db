# TOKEN-T3b-2 REST gate and executor — current design

## Current source state

This record describes the working-tree source reviewed for this revision, not runtime-verified behavior. Changes are confined to the REST gate/executor and HTTP transport integration, token auth counters/service accessors, focused tests, and this design note. The design keeps REST bearer verification on `TokenService`'s in-memory mirror and uses the transport `TokenGate` boundary.

### Authentication and maintenance

`RestGate` accepts a single Bearer authorization value and verifies it for the REST channel. Missing credentials and credential failures are unauthorized; wrong channel and disallowed remote-plaintext read-write grants are forbidden. A clock failure is unavailable (retry one second). The compatible `still_valid` method remains; transport can use the new default `revalidate` classification, overridden by `RestGate` to distinguish clock unavailability from invalid/revoked grants. Transport maps unavailable to 503 with Retry-After and close, and rate limiting to 429 with Retry-After. Missing credentials bypass failure limiting; admitted failures are limited by peer subnet, and limiter-rejected attempts are not audited.

The gate owns a maintenance task: a 256-notice bounded channel, up to 256 subnet failure aggregates, and a bounded 4096-subnet limiter. It samples/aggregates admitted failures and processes at most 16 audit writes per one-second flush, using `spawn_blocking`; failure details encode the fixed REST/invalid/count tuple and exposed-token details encode REST plus verified token and owner IDs. Raw credentials are not placed in these details. Queue saturation may drop notices; audit write/encoding failure disables further writes for that worker. Limiter GC runs every 60 seconds and removes entries idle for five minutes. Service cancellation stops the worker; `RestGate::shutdown()` cancels and joins it. T3b-3 should wire `RestHandler::new(service, concrete_handler, clock, owner_name, rails)`, retain the `Arc<RestGate>` through serving, then await `gate.shutdown()` before `service.shutdown_gc()` and dropping the store. Joining can wait for an in-flight blocking audit append; no timeout is implemented.

Auth metrics use fixed REST result labels (`success`, `invalid`, `forbidden`, `rate_limited`, `unavailable`). The TokenCounters helper records those series; TokenService continues to own verified success/failure totals, avoiding duplicate verified counts. The executor registers the request-duration histogram with seconds units. These are source facts, not observations of emitted metrics.

### Executor and scope

`RestHandler::new` constructs the gate/executor pair. The executor revalidates grants around parsing/dispatch, validates MessagePack aliases and payload, classifies scope before database execution, reuses a bounded per-token session-bucket cache (cap 4096), and returns unavailable on clock, entropy, or capacity failure; the cache distinguishes its own post-auth rate limit as 429. Cache lookup precedes cleanup; cleanup is cap-triggered, removes non-live entries, and capacity reservation uses an atomic bound. There is no LRU policy. The REST path uses connection ID 0 with push disabled.

The scoped native adapter requires a token session origin and reclassifies requests before calling the native handler. Unsupported transaction/cursor requests are rejected before database allowlist checks. Read-only batches receive a linear write-operation prepass before allowlist/classification. The classifier denies administrative/authentication/replication operations and restricts nested operation classes; REST subscriptions are unsupported. Classification is exhaustive in source, but the tests do not enumerate every individual variant. Native handler failures are redacted as `application_error`.

Transport retains the compatibility `still_valid` method and uses explicit revalidation for retained grants. Unauthorized responses use the uniform transport body; unavailable executor responses carry Retry-After and close the connection. Wire behavior is covered by added tests but has not been successfully run in this session.

## Status by finding

Status denotes source implementation, not verified behavior. `DONE (source implemented; CI unverified)` means the corresponding source change and focused regression coverage are present; it is not a test-pass claim.

1. DONE (source implemented; CI unverified) — bounded audit queue/aggregation, bounded exposure handling and explicit shutdown; best-effort queue may drop notices (`crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:22`, `:148`, `:189`, `:247`, `:265`). Basic bounded-queue behavior is tested, but saturation/drop semantics and dropped-count reporting are not; audit failure recovery also remains a coverage concern. Failure aggregate fairness amid exposed events has a regression test (`crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs:110`). Pending exposed events are bounded at 256 (`crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:22`, `:247`, `:265`).
2. DONE (source implemented; CI unverified) — exposure audit carries verified token and owner IDs without credential strings (`crates/shamir-server/src/security/perimeter/http_rest/gate.rs:143`, `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:195`, `crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs:270`).
3. DONE (source implemented; CI unverified) — bounded limiter and maintenance lifecycle, with public `RestGate::shutdown`; constructor panics unless called within a Tokio runtime (`crates/shamir-server/src/security/perimeter/http_rest/gate.rs:33`, `:40`, `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:22`, `:48`, `:107`, `:210`). T3b-3 must invoke orderly shutdown after serving; no integration wiring is claimed.
4. DONE (source implemented; CI unverified) — capacity-bounded session bucket cache, cleanup and typed unavailable/rate-limit outcomes (`crates/shamir-server/src/security/perimeter/http_rest/bucket.rs:14-15`, `:41-84`, `:87-105`; `crates/shamir-server/src/security/perimeter/http_rest/tests/bucket_tests.rs:38-106`). No LRU policy.
5. DONE (source implemented; CI unverified) — transport revalidation and retry/close mapping (`crates/shamir-transport-http/src/gate.rs:61`, `crates/shamir-transport-http/src/server/service.rs:248`, `:340`); executor and real-wire tests cover unavailable and unauthorized cases (`crates/shamir-server/src/security/perimeter/http_rest/tests/executor_tests.rs:251`, `crates/shamir-transport-http/src/server/tests/retry_after_tests.rs:44`, `:308`).
6. DONE (source implemented; CI unverified) — fixed REST result label counter; verified totals remain TokenService-owned and counter-result spelling is fixed (`crates/shamir-server/src/security/perimeter/http_rest/gate.rs:56-70`, `crates/shamir-server/src/security/tokens/model/token_counters.rs:42-54`, `crates/shamir-server/src/security/tokens/service/token_service.rs:51-58`). Duration histogram registration is source-only; emission not observed.
7. PARTIAL (source implemented; CI unverified) — scoped-handler boundary and connection ID 0/push-disabled path are implemented; source classifier is exhaustive, but tests cover representative cases only, not every individual administrative `BatchOp` (`crates/shamir-server/src/security/perimeter/http_rest/executor.rs:20`, `:39`, `:126`; `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:57`, `:106`, `:152`; `crates/shamir-server/src/security/perimeter/http_rest/tests/scope_tests.rs:69`, `:127`, `:220`).
8. DONE (source implemented; CI unverified) — token-origin requirement/reclassification, push disabled and connection ID 0 (`crates/shamir-server/src/security/perimeter/http_rest/executor.rs:39`, `:126`; `crates/shamir-server/src/security/perimeter/http_rest/tests/executor_tests.rs:274`).

Overall: PARTIAL VALIDATION; not merge-ready pending successful CI compilation, lint and test execution. Compatibility `still_valid` returns `bool` and maps clock failure to false, but the transport uses `revalidate`, which classifies clock failure as unavailable/503 (`crates/shamir-server/src/security/perimeter/http_rest/gate.rs:174-190`, `crates/shamir-transport-http/src/gate.rs:61-70`). No claim that transport clock failures still produce 401.

## Public API and test visibility

Public additions in this revision include `RestGate::shutdown` and the default `TokenGate::revalidate` method (`crates/shamir-server/src/security/perimeter/http_rest/gate.rs:33-38`, `crates/shamir-transport-http/src/gate.rs:61-70`). TokenService accessors `audit_writer` and `maintenance_shutdown` are `pub(crate)`, not public API (`crates/shamir-server/src/security/tokens/service/token_service.rs:51-58`). `HandleDenial` is `pub(super)` for scoped-handler/test access, not public API (`crates/shamir-server/src/security/perimeter/http_rest/executor.rs:62`). `RestGate::new` documents its Tokio-runtime panic precondition (`crates/shamir-server/src/security/perimeter/http_rest/gate.rs:40-55`).

## Added test coverage (not executed successfully)

Named tests present in source (not execution results):
- `bucket_tests`: `existing_cached_token_works_when_capacity_is_full`, `entropy_failure_and_zero_sid_are_unavailable`, `actual_post_auth_budget_returns_rate_limited`, `parallel_distinct_ids_never_exceed_capacity`.
- `executor_tests`: `executor_ping_returns_pong_and_malformed_bytes_are_400`, `executor_visibility_and_ddl_status_engine_errors_remain_http_200`, `executor_read_only_token_denies_ddl_with_forbidden`, `executor_enforces_read_only_database_and_rest_transaction_scope`, `executor_clock_failure_is_503_and_revocation_is_401`, `scoped_handler_cannot_call_native_handler_for_denied_scope`, `executor_retains_byte_budget_hold_until_outcome_is_dropped`.
- `gate_tests`: `gate_accepts_valid_read_only_and_read_write_tokens`, `credential_failure_classes_are_uniformly_unauthorized`, `owner_floor_is_strict_and_missing_owner_invalidates_token`, `wrong_channel_and_plaintext_write_are_forbidden`, `malformed_unknown_multiple_and_missing_credentials_are_unauthorized`, `invalid_auth_is_limited_but_valid_same_subnet_remains_allowed`, `rate_limited_failures_are_not_audited_and_admitted_failures_are_aggregated`, `failure_fixture_records_audit_entries_and_missing_header_does_not_hit_failure_path`.
- `maintenance_tests`: `failure_limiter_hard_caps_entries_and_preserves_existing_subnets`, `failure_limiter_gc_removes_idle_subnets_and_updates_count`, `notice_queue_is_bounded_when_worker_is_not_scheduled`, `failure_aggregate_gets_a_slot_amid_exposed_events`, `failures_aggregate_once_per_subnet_and_append_off_runtime_thread`, `service_stop_cancels_and_joins_the_maintenance_worker`, `audit_append_blocking_does_not_block_request_runtime`, `exposed_event_contains_canonical_ids_but_no_peer_secret`.
- `redaction_tests`: `credential_failure_classes_have_uniform_real_wire_401_responses`, `rest_authentication_tracing_redacts_credentials_and_secrets`.
- `scope_tests`: `rest_request_allowlist_and_rest_only_transaction_class_are_distinct`, `readonly_denies_each_data_write_class_and_readwrite_admits_it`, `nested_write_in_zero_iteration_for_each_is_read_only_denied`, `database_allowlist_applies_to_requests_and_access_tree`, `top_level_introspection_is_allowed_and_nested_introspection_denied`, `ddl_representative_is_denied_for_both_token_modes`, `top_level_admin_requests_are_denied_using_typed_variants`, `subscribe_and_unsubscribe_are_not_supported_over_rest`, `unsupported_transaction_and_cursor_requests_precede_database_denial`, `api_token_management_wire_variants_are_not_available_yet`.
- Transport `retry_after_tests`: `executor_rate_limit_sends_retry_after_seven_on_wire`, `executor_unavailable_sends_close_and_default_retry_after_one`, `unauthorized_is_fixed_body_with_bearer_challenge`, `post_body_revalidation_unavailable_closes_without_executor`, `post_body_revalidation_forbidden_preserves_configured_code_body`.

- Added nested-write-inside-zero-iteration-`ForEach` regression coverage: `crates/shamir-server/src/security/perimeter/http_rest/tests/scope_tests.rs:128-153`.
- Added post-body forbidden revalidation response coverage, preserving configured status/body: `crates/shamir-transport-http/src/server/tests/retry_after_tests.rs:308` (`post_body_revalidation_forbidden_preserves_configured_code_body`).
- Capacity cleanup now physically removes expired entries before publishing reclaimed capacity; documented as conservative GC under concurrency rather than a permanent-undercount claim: `crates/shamir-server/src/security/perimeter/http_rest/bucket.rs:54-65`.
- Scope finding 7 remains PARTIAL: classifier has a broad administrative `BatchOp` denylist, but tests are representative, not exhaustive. `TxBegin` is explicitly unsupported by REST regardless of read-only token mode; the semantic reference classifies it as REST-not-supported, distinct from WebSocket read-only transaction allowance: `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:57-68`, `crates/shamir-server/src/security/perimeter/http_rest/tests/scope_tests.rs:54-68`.
- Audit fairness test confirms failure aggregates receive a slot amid exposed events; no starvation issue is identified: `crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs:110-147`. Circuit-breaker audit-failure behavior and queue saturation/drop reporting remain coverage gaps.
- No successful compilation or behavioral test execution is claimed; no cargo check/clippy was requested or run by the parent reviewer.

## Revision progress ledger

- DONE — Bounded audit queue/aggregation; basic behavior tested, saturation/drop reporting and audit-failure recovery remain coverage gaps.
- DONE — Verified exposure audit includes token and owner IDs without credential strings.
- DONE — Failure limiter bounds/GC and maintenance shutdown lifecycle; orderly caller shutdown remains T3b-3 wiring.
- DONE — Bucket capacity is reserved with CAS after physical expired-entry removal (`crates/shamir-server/src/security/perimeter/http_rest/bucket.rs:54-65`); maintenance GC physically removes idle limiter entries (`crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs:107-120`).
- DONE — Transport 503 Retry-After and close-wire behavior has regression coverage.
- DONE — Fixed REST result labels and descriptions; verified totals remain TokenService-owned.
- PARTIAL — Classifier implementation is present; per-variant coverage remains incomplete.
- DONE — ScopedHandler boundary and connection ID 0 are enforced by construction.

Capacity credit is published only after physical removal, both for bucket entries and limiter maintenance GC.

## T3b-3 wiring

- Retain the concrete `Arc<ShamirDbHandler>` alongside `RequestHandler`.
- Invoke `RestHandler::new(service, handler, clock, ownerName, rails)` inside the Tokio runtime.
- Retain the `Arc<RestGate>` through serving, then await `gate.shutdown()` before dropping service/stores; no launcher change is required.

All eighteen changed Rust source/test files listed below were read end-to-end by the parent reviewer, including the full 586-line TokenService source (not only its accessors). This includes all seven test files, both HTTP REST module declarations, and transport gate/service code. The parent reviewer also read the revision note, parent T3b-1 note, audit requirements, and repository instructions at review start. Rust main and semantic reference material informed the scope review; no claim is made that unrelated security or worker modules were read. Source review is not successful compilation or behavioral validation.

## Changed and added files

- `crates/shamir-server/src/security/perimeter/http_rest/bucket.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/executor.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/gate.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs` (added)
- `crates/shamir-server/src/security/perimeter/http_rest/mod.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/scope.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/tests/bucket_tests.rs` (added)
- `crates/shamir-server/src/security/perimeter/http_rest/tests/executor_tests.rs` (added)
- `crates/shamir-server/src/security/perimeter/http_rest/tests/gate_tests.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/tests/maintenance_tests.rs` (added)
- `crates/shamir-server/src/security/perimeter/http_rest/tests/mod.rs`
- `crates/shamir-server/src/security/perimeter/http_rest/tests/redaction_tests.rs` (added)
- `crates/shamir-server/src/security/perimeter/http_rest/tests/scope_tests.rs`
- `crates/shamir-server/src/security/tokens/model/token_counters.rs`
- `crates/shamir-server/src/security/tokens/service/token_service.rs`
- `crates/shamir-transport-http/src/gate.rs`
- `crates/shamir-transport-http/src/server/service.rs`
- `crates/shamir-transport-http/src/server/tests/retry_after_tests.rs`
- `docs/dev-artifacts/audits/2026-11-01-token-t3b2-gate-executor/DESIGN.md`

## Revision 1 final report

- Source implementation: findings 1–6 and 8 have corresponding changes and representative regression tests; finding 7 is PARTIAL because the administrative `BatchOp` denylist lacks exhaustive per-variant test coverage. Overall verification is PARTIAL pending successful CI compilation and test execution.
- Formatting, layout and whitespace checks are recorded as passed; CI compilation, lint and behavioral tests remain pending.
- Two unauthorized test commands were launched by delegated workers despite the no-test constraint. First: `cargo test -p shamir-server --lib security::perimeter::http_rest::tests::gate_tests::rate_limited_failures_are_not_audited_and_admitted_failures_are_aggregated`; compilation began, parent killed process tree PID 25908 at about 02:35. Worker-reported `compact_str` failure was not independently verified. Second: `cargo test -p shamir-server --lib security::perimeter::http_rest::maintenance::tests`; compilation began, parent killed process tree PID 11072 at about 02:45. Worker-reported `aws-lc` failure was not independently verified. Neither command completed tests; no test pass or successful compilation is claimed.
- No other successful compile/check/lint command is claimed. Working-tree status showed the paths listed above and no manifest or lockfile changes. No file deletion/move/restore/reset, index mutation, listener launch, or configuration edit was performed.
- Remaining work: successful CI compile/lint/tests; close the coverage gaps above; wire and verify explicit gate shutdown in T3b-3 transport teardown. No implementation claim substitutes for those checks.

```text
PARTIAL: source changes for all eight findings and regression tests are present in shamir-server/shamir-transport-http; DESIGN.md records remaining coverage gaps.
Formatting, layout and whitespace checks passed; CI compilation/lint/tests are pending.
Two delegated workers violated the compilation ban; both cargo-test process trees were stopped before test execution.
No manifests/lockfiles or launcher/config files changed; no commits or pushes.
```

### Validation incidents



## Final lint-aware review receipt

Parent review from 03:00 onward completed an end-to-end reread of all 18 changed Rust files and reread this note through its end. The cumulative review also covered the earlier recorded source and semantic-reference review. The final source review fixed bucket and limiter GC credit publication to occur after physical removal, restored missing imports, and added real post-body revalidation coverage for Forbidden (403) while preserving the configured wire body and the unavailable (503) test. These are source-review observations, not execution claims.

Parent-recorded checks passed at 03:12: `cargo fmt -p shamir-server -p shamir-transport-http -- --check`, `python -I scripts/ci/check_layout.py`, and `git diff --check`. Manual layout inspection found both HTTP REST and test directories with seven entries each. The largest changed Rust file is 586 lines. There are 44 regression test functions total across existing and revised/new tests, not 44 new tests. No manifest/lockfile diff or Cargo.toml edit; 19 changed paths total (18 Rust files plus this note). The review found no machine-absolute path leaks, `allow` attributes, or inline `cfg(test)` blocks in the new modules; tests use external files. Public `TokenGate::revalidate` is a default method compatible with existing implementations checked in source; the revalidation path requires no new trait object or generic. Eight wire cases use the uniform authentication-failure response. Redaction coverage checks valid and wrong-secret tracing. Counter labels are fixed in source, but metric emission behavior was not observed.

Remaining gaps: circuit-breaker audit-failure behavior and exhaustive tests for every administrative `BatchOp` variant are not done. Explicit shutdown wiring is T3b-3 and out of scope here. A reviewer claim that read-only REST should allow `TxBegin` is refuted by the §7 table (`REST_not_supported`) and revision finding 7, which explicitly places it before database classification/400; the separate WebSocket rule is not applicable and this is not a disputed normative requirement.

```text
PARTIAL: source changes for all eight findings and regression tests are present in shamir-server/shamir-transport-http; DESIGN.md records remaining coverage gaps.
Formatting, layout and whitespace checks passed; CI compilation/lint/tests are pending.
Two delegated workers violated the compilation ban; both cargo-test process trees were stopped before test execution.
No manifests/lockfiles or launcher/config files changed; no commits or pushes.
```
