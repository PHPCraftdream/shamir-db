# TOKEN-T3b-2 REST gate and executor — current design

This note records Revision 2 source observations, not compile or test results.

## Current architecture

`RestGate` verifies one Bearer credential through `TokenService` for REST. Missing credentials do not enter the failure limiter; admitted invalid failures are grouped by peer subnet. Channel denial and prohibited remote-plaintext read-write grants are forbidden. Revalidation distinguishes clock failure (unavailable/503) from invalid or revoked grants (unauthorized/401). Endpoint-aware auth and request-denial counters have source regressions; they have not been executed. See `crates/shamir-server/src/security/perimeter/http_rest/gate.rs` and `crates/shamir-server/src/security/perimeter/http_rest/executor.rs`.

Maintenance uses separate bounded 256-notice failure/exposure channels, a 256-entry pending-exposure buffer, and one writer with batches capped at 16. Failure summaries carry total, tracked-distinct lower bound (cap 256), up to four subnet samples, failure dropped notices, untracked failures, and a separate exposure dropped count. The failure limiter is capped at 4096 entries; its warm-up anchor is sampled at `RestMaintenance::new` when the clock is available, otherwise first check initializes it. It permits 2 attempts/second during the first 60 seconds, then 10/second in fixed one-second windows (not token-bucket semantics). At capacity it may evict an entry idle more than one second; periodic GC runs every 60 seconds for entries idle five minutes. Shutdown drains for up to two seconds; a blocking append is not cancellable and may be detached, so this is not a durability/completion guarantee. See `crates/shamir-server/src/security/perimeter/http_rest/maintenance.rs`.

The REST handler validates/classifies requests, uses a per-token session cache capped at 4096, and invokes a scoped native handler with push disabled and connection ID 0. Cache cleanup at capacity removes sessions idle at least one second or no longer live; sweep attempts are throttled to once per second. Scope is classified at executor admission and reclassified before native dispatch. A production closure boundary has panic-on-denied and allowed-Ping test doubles; no native call occurs for denied requests. See `crates/shamir-server/src/security/perimeter/http_rest/{executor.rs,bucket.rs,scope.rs}`.

T3b-3 integration must construct the handler inside Tokio, retain the gate through serving, and await `gate.shutdown()` before service/store teardown. The two-second maintenance drain does not guarantee a blocking OS append has completed.

## Progress by work item

1. DONE (source) — bounded channels, counted drops, capped pending exposures, and 16-entry batches; shutdown drains until EOF/deadline. A blocking writer may outlive shutdown; no durability guarantee. Tests cover summary/drain/deadline paths; not executed.
2. DONE (source) — summary schema includes tracked distinct, samples, failure drops, untracked failures, and separate exposure drops. Tests cover bounded schema and overflow; not executed.
3. DONE (source) — bounded limiter, startup warm-up anchoring/lazy fallback, fixed windows, eviction and GC; concurrency tests cover the hard cap. Occupied-entry race remains an explicit coverage limitation; not executed.
4. DONE (source) — bounded activity-based cache eviction and throttled sweeps; tests cover capacity, cached hits, hit/Vacant guard release, idle eviction and sweep throttling. Occupied-entry race remains untested; not executed.
5. DONE (source) — shared disabled state is used by producers/flusher; tests cover failing persistence, draining, final drops, and deadline behavior. Blocking writes may outlive shutdown; tests not executed.
6. DONE (source) — REST denial counter is distinct from auth-result counters; recorder regression checks success, denial, and unavailable-clock revalidation. Not executed.
7. DONE (source) — scope is reclassified before native dispatch; denied and allowed handler-double paths exercise production dispatch. Not executed.
8. DONE (source) — REST rate-limit wire and native-handler boundary regressions are present; not executed.
9. NOT STARTED — outside requested scope.

## Validation incidents and checks

Historical Revision 1 incidents (2026-11-01; not this session): two forbidden delegated test launches began compilation and were stopped before test execution. Reported dependency failures were not independently verified. No commands were run by workers in this session.

Parent reports `cargo fmt -p shamir-server -p shamir-transport-http -- --check` passed at 08:22, layout passed at 08:21, and `git diff --check` failed only on the extra final blank line subsequently removed. Compilation, clippy, and behavioral tests were not run; no Revision 3 runtime verification is claimed.

Source regression inventory: `bucket_tests.rs` covers cap, cached hit, hit/Vacant guard-release probes, idle eviction, and sweep throttling (Occupied-entry race absent). `maintenance_tests.rs` covers summary schema/overflow, exact warm-up limits, shutdown drain, writer failure/deadline, and exposure-ID logging. `executor_tests.rs` covers metric separation and production closure dispatch. Source presence is not execution evidence.

Source anchors (subject to line movement): `maintenance.rs` (startup anchor and flusher), `bucket.rs` (cache lifecycle), `executor.rs` (production closure boundary and revalidation), `gate.rs` (authentication), and `executor_tests.rs` / `maintenance_tests.rs` (focused regressions).

## Revision 2 final report

```text
PARTIAL: revision 2 changes are present in REST maintenance, limiter/cache, auth counters, executor, and focused tests; duplicate classification and requested regression coverage remain incomplete. Formatting, layout, and whitespace checks passed; compilation, clippy, and behavioral tests were not run. No manifests, lockfiles, or launcher/config files changed; no commits or pushes.
```

This historical report predates Revision 3 changes and must not be read as current validation. No Revision 3 compilation, formatting, or behavioral test results are claimed.

### Revision 3 item 1

DONE — source changes for bounded cache growth, throttled maintenance sweeps, and idle cleanup are present. The original six bucket regressions were reconstituted; tests cover capacity preservation, cached hits, hit/Vacant guard-release probes, and sweeps. Occupied-entry race remains untested. Not executed.

### Revision 3 item 2

DONE — failure-limiter capacity eviction scans use a 100 ms CAS throttle and test-only scan counting; the focused test fills the limiter and checks scan counts inside and beyond the throttle interval. Not executed.

### Revision 3 item 3

DONE — REST scope denials increment a separate endpoint-labelled denial counter; recorder coverage checks success, denial, and unavailable-clock revalidation without conflating auth-result counters. Not executed.

### Revision 3 item 4

DONE — exposure drops are a distinct trailing summary field; logs include token IDs for full, closed, disabled, and persistence-failure paths. Captured-log and summary coverage is present. Not executed.

### Revision 3 item 5

DONE — the shared disabled flag, failing-appender shutdown, 17-exposure drain, and late-drop accounting assertions are present in source. No commands or tests were run; validation remains unverified.


### Revision 3 item 6

DONE — failure-limiter startup anchors from the maintenance clock when available, with lazy first-check fallback otherwise; tests assert the 2-attempt warmup and 10-attempt post-warmup limits. No commands or tests were run.

### Revision 3 item 7

DONE — protocol audit event uses `api_token_auth_failures`; duplicate Revision 2 report was reconciled without losing the single historical report, and stale formatting-pending anchor removed. Not executed.

```

### Final response

Revision 3 items 1–8 implemented in REST cache, gate, executor, maintenance, and their tests; TOKEN_API.md and DESIGN.md updated.
Checks: cargo fmt --check and layout passed; compilation, clippy, and behavioral tests were not run, per instruction.
Optional item 9 skipped; duplicate classification remains. Occupied-entry race and periodic-GC integration need additional CI coverage.
No commits, git mutations, manifest/lockfile changes, or transport-HTTP changes.
