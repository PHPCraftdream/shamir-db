# TOKEN-T3b-2 REST gate and executor — current design

This note records Revision 2 source observations, not compile or test results.

## Current architecture

`RestGate` verifies one Bearer credential through `TokenService` for REST. Missing credentials do not enter the failure limiter; admitted invalid failures are grouped by peer subnet. Channel denial and prohibited remote-plaintext read-write grants are forbidden. Revalidation distinguishes clock failure (unavailable/503) from invalid or revoked grants (unauthorized/401). Endpoint-aware counter updates exist in source; metric-specific regression coverage is incomplete. See `crates/shamir-server/src/security/perimeter/http_rest/gate.rs` and `crates/shamir-server/src/security/tokens/model/token_counters.rs`.

Maintenance uses separate bounded 256-notice failure/exposure channels, a 256-entry pending-exposure buffer, and one writer with batches capped at 16. Failure summaries carry total, tracked-distinct lower bound (cap 256), up to four subnet samples, dropped notices, and untracked failures. Its failure limiter is capped at 4096 entries; first check anchors a 60-second warmup at 2 attempts/second, then 10/second fixed one-second windows (not token-bucket semantics). At capacity it may evict an entry idle more than one second; periodic GC runs every 60 seconds for entries idle five minutes. Shutdown drains for up to two seconds; a blocking append is not cancellable and may be detached, so this is not a durability/completion guarantee. See `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs`.

The REST handler validates/classifies requests, uses a per-token session cache capped at 4096, and invokes a scoped native handler with push disabled and connection ID 0. Cache cleanup at capacity removes sessions idle at least one second or no longer live. Scope is reclassified before native dispatch (duplicate classification remains); the native-call test verifies denied requests avoid dispatch, but the handler-double coverage is incomplete. See `crates/shamir-server/src/security/perimeter/http_rest/{executor.rs,bucket.rs,scope.rs}`.

T3b-3 integration must construct the handler inside Tokio, retain the gate through serving, and await `gate.shutdown()` before service/store teardown. The two-second maintenance drain does not guarantee a blocking OS append has completed.

## Progress by work item

1. PARTIAL — bounded channels, counted drops, capped pending exposures, and bounded batches; stalled-writer/failure-recovery coverage incomplete.
2. PARTIAL — bounded summary schema; runtime audit behavior unverified.
3. PARTIAL — bounded limiter, warmup/fixed windows, eviction, and GC; concurrency unverified.
4. PARTIAL — bounded activity-based cache eviction; request-budget behavior unverified.
5. PARTIAL — bounded shutdown wait; active blocking append can outlive deadline.
6. PARTIAL — endpoint-aware counters; metric-specific regression coverage incomplete.
7. PARTIAL — pre-dispatch scope revalidation; duplicate classification and handler-double coverage incomplete.
8. PARTIAL — rate-limit wire and native-call regression source present; further coverage and CI incomplete.

## Validation incidents and checks

Historical Revision 1 incidents (2026-11-01; not this session): two forbidden delegated test launches began compilation and were stopped before test execution. Reported dependency failures were not independently verified. No commands were run by workers in this session.

The parent reported final `cargo fmt --check`, layout, and `git diff --check` exit 0 at 06:22. A later maintenance-test edit means a fresh final recheck is pending. No compilation, clippy, or behavioral tests were run. No manifests, lockfiles, or launcher/config files changed; no commits or pushes.

Focused source regressions include `bucket_tests.rs::idle_bucket_is_evicted_to_admit_new_token`; `maintenance_tests.rs::failure_limiter_hard_caps_entries_and_preserves_existing_subnets`, `failure_limiter_gc_removes_idle_subnets_and_new_subnet_uses_post_warmup_rate`, and queued-shutdown cases; native-call denial in `executor_tests.rs`; and rate-limit wire coverage in `retry_after_tests.rs`. Source presence is not execution evidence.

Source anchors (subject to line movement): `maintenance.rs:1` (formatting pending), `bucket.rs:43`, `executor.rs:43` (native guard) and `:152` (duplicate classification), `gate.rs:192` (revalidation without metric increment), `token_counters.rs:44`, and `retry_after_tests.rs:212` (wire case).

## Revision 2 final report

```text
PARTIAL: revision 2 changes are present in REST maintenance, limiter/cache, auth counters, executor, and focused tests; duplicate classification and requested regression coverage remain incomplete. Formatting, layout, and whitespace checks passed; compilation, clippy, and behavioral tests were not run. No manifests, lockfiles, or launcher/config files changed; no commits or pushes.
```

The final report reflects the parent-observed checks; the fresh recheck after the last maintenance-test edit is pending. No claim of compilation or test success is made.
