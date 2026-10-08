<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tunables — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The unwired API, unchecked setters, phantom environment override, missing boundary/effect tests, and stale documentation remain. Several predicted downstream failures need correction; the unwired API warrants medium rather than demonstrated runtime high severity.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 5 | 5 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Runtime override API is unwired — all three setters are silent no-ops for real behavior

Status: `confirmed-open`. Current risk: `medium`.

Only crate tests call the getters/setters. ServerHandle constructs the object after listener setup; contexts, frame allocations, and five accept-error backoffs still use constants. The server behavior mismatch remains, although deferral is explicit in the handle and roadmap.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:985](../../../../../crates/shamir-server/src/server/server_launcher.rs#L985); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048); [crates/shamir-server/src/connection/handshake.rs:706](../../../../../crates/shamir-server/src/connection/handshake.rs#L706); [crates/shamir-server/src/server/server_handle.rs:96](../../../../../crates/shamir-server/src/server/server_handle.rs#L96); [docs/dev-artifacts/roadmap/TUNABLES.md:175](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L175).

<a id="review-2"></a>

### Claim 2 — Setters accept degenerate values unvalidated; millisecond quantization silently truncates to zero

Status: `confirmed-open`. Current risk: `medium`.

The raw usize stores and narrowing duration cast are unchanged and undocumented. Sub-ms inputs become zero and sufficiently large durations wrap. Production consequences remain latent; the existing request-loop max(1) prevents the alleged zero-permit stall even if its input is switched to a getter.

Evidence: [crates/shamir-tunables/src/runtime.rs:56](../../../../../crates/shamir-tunables/src/runtime.rs#L56); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-tunables/src/runtime.rs:73](../../../../../crates/shamir-tunables/src/runtime.rs#L73); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153).

<a id="review-3"></a>

### Claim 3 — Phantom env override documented: `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` is read nowhere

Status: `confirmed-open`. Current risk: `medium`.

The startup-override promise remains, while VectorBackend initializes its threshold directly from the constant. Its mutation seam is cfg(test)-only, not an environment reader.

Evidence: [crates/shamir-tunables/src/lib.rs:149](../../../../../crates/shamir-tunables/src/lib.rs#L149); [crates/shamir-index/src/vector/vector_backend.rs:143](../../../../../crates/shamir-index/src/vector/vector_backend.rs#L143); [crates/shamir-index/src/vector/vector_backend.rs:160](../../../../../crates/shamir-index/src/vector/vector_backend.rs#L160).

<a id="review-4"></a>

### Claim 4 — TDD coverage gaps on the runtime surface: degenerate inputs, truncation, and override-effect never tested

Status: `confirmed-open`. Current risk: `low`.

The five reachable tests still cover defaults, ordinary round trips, and single-thread Arc callability only. No boundary, overflow, double-overwrite, or server-effect test exists. defaults_equal_consts would detect a zero-initializing derived Default.

Evidence: [crates/shamir-tunables/src/lib.rs:11](../../../../../crates/shamir-tunables/src/lib.rs#L11); [crates/shamir-tunables/src/tests/mod.rs:1](../../../../../crates/shamir-tunables/src/tests/mod.rs#L1); [crates/shamir-tunables/src/tests/runtime_tests.rs:7](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L7); [crates/shamir-tunables/src/tests/runtime_tests.rs:27](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L27); [crates/shamir-tunables/src/tests/runtime_tests.rs:52](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L52).

<a id="review-5"></a>

### Claim 5 — `lib.rs` header doc stale relative to shipped `runtime.rs`

Status: `confirmed-open`. Current risk: `nit`.

The opening line mentions runtime knobs, but the following paragraph still describes today's contents as plain constants and promotion as future. It does not explain the existing, deliberately unwired atomic foundation.

Evidence: [crates/shamir-tunables/src/lib.rs:1](../../../../../crates/shamir-tunables/src/lib.rs#L1); [crates/shamir-tunables/src/lib.rs:4](../../../../../crates/shamir-tunables/src/lib.rs#L4); [crates/shamir-tunables/src/lib.rs:9](../../../../../crates/shamir-tunables/src/lib.rs#L9).

## Corrections and qualified non-findings

- Downgrade finding 1 from high to medium: the misleading API is source-proven, but no live configuration/admin consumer or resulting production outage is demonstrated.
- Finding 2: Vec::with_capacity(0) is a valid allocation hint, not inherently invalid. The current 50 ms Default conversion is exact; only future unsuitable defaults would expose its narrowing.
- Finding 2: the max(1) at crates/shamir-server/src/connection/request_loop.rs:153 is value-based, not restricted to constant-fed input. Tokio 1.49 bounded mpsc rejects zero capacity; it does not hang or provide rendezvous semantics.
- Zero interval removes the intended 50 ms error backoff, but mandatory core saturation/starvation is not proven. These are accept-error branches, not continuously ticking idle housekeeping loops.
- Positive checks remain: consecutive push failures reset on success and terminate at >=100 (crates/shamir-server/src/subscriptions/push.rs:100,118); subscription reservation rejects at active >= cap using CAS (crates/shamir-server/src/subscriptions/registry.rs:77); the request cap feeds both primitives (crates/shamir-server/src/connection/request_loop.rs:153).
- Qualify the backpressure guarantee: high/2 hysteresis exists, but a stuck drain causes abandonment after five seconds, not unconditional waiting until the low watermark (crates/shamir-engine/src/tx/commit.rs:492,526,549).
- The suggested server cadence test must force the accept-error branch or use an injectable sleep/accept seam; an ordinary idle-server timing test cannot detect this wiring defect.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tunables -- Correctness & TDD-coverage

## Summary

The crate is small and internally sound: the `store_defaults`/`instance_defaults` consts are consumed exactly as documented (verified per-consumer: `SLOW_CONSUMER_THRESHOLD` fires on the 100th failure via `>=`, the subscription cap rejects at `active >= cap`, `MAX_UNDRAINED_VERSIONS` really brakes to `high/2` at `commit.rs:489`, `CONN_MAX_IN_FLIGHT` really feeds both the semaphore and the mpsc cap), and the manual `Default` impl + `defaults_equal_consts` test correctly guard against a would-be `#[derive(Default)]` regression to zeros. The substantive problems are in the runtime half: `RuntimeTunables` ships a public override API whose setters are silently ineffective (no consumer in the workspace ever reads the struct), the setters accept degenerate values (`0`, sub-millisecond durations) that consumers are already forced to defensively clamp elsewhere, and one documented env override (`SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD`) does not exist anywhere in the codebase. Test organization follows CLAUDE.md's `tests/` layout faithfully, but per the Red/Green discipline the suite never exercises the degenerate-input or override-takes-effect paths, so these gaps shipped green.

## Findings

### 1. Runtime override API is unwired — all three setters are silent no-ops for real behavior
- **File:line:** `crates/shamir-tunables/src/runtime.rs:36-76`; cross-refs `crates/shamir-server/src/server/server_launcher.rs:900,958-959,1008,1105,1210`, `crates/shamir-server/src/connection/handshake.rs:705,707`, `crates/shamir-server/src/server/server_handle.rs:89-93`
- **Severity:** high
- **Issue:** `runtime.rs` promises "Overrides are rare and just store a new atomic value, taking effect on the next read" and "Instance-level runtime-overridable tunables". In the workspace the only `RuntimeTunables` instance is constructed at `server_launcher.rs:900` (`Arc::new(RuntimeTunables::new())`), stored on the `pub` field `ServerHandle::tunables`, and its accessors (`io_frame_buffer_cap()` / `server_poll_interval()` / `conn_max_in_flight()`) have **zero** non-test call sites — grep confirms every production read goes straight to the compile-time consts (`instance_defaults::CONN_MAX_IN_FLIGHT`, `CONN_IDLE_TIMEOUT`, `SERVER_POLL_INTERVAL` at launcher 958/959/1008/1105/1210; `IO_FRAME_BUFFER_CAP` at handshake 705/707). `server_handle.rs:91-92` admits "Consumer wiring ... is deferred to a follow-up slice", but the crate-side setter API and the `pub` field are already shipped and look functional.
- **Failure scenario:** a caller (test, SDK consumer, future ops hook) does `server.tunables.set_conn_max_in_flight(8)`; the setter compiles, the getter dutifully returns 8, and actual server behavior stays at the const `32` — a silent, undetectable configuration divergence with no error or warning. TDD angle: the crate's entire test suite exercises exactly this unwired surface, and no test anywhere in the workspace asserts any consumer honors a runtime override — green tests over dead plumbing.
- **Suggested fix:** either land the wiring (pass the shared `Arc<RuntimeTunables>` into `ConnectionContext` and the three poll loops and replace the const reads — a small, mechanical diff), or until then mark the setters `#[doc(hidden)]` with a "not yet consumed by any call-site; overrides are no-ops" warning (or remove them), so the API cannot be mistaken for functional. Also reconcile `lib.rs:4-7` ("Today these are plain `const`s ... a later phase promotes") with `runtime.rs`'s present-tense claims.

### 2. Setters accept degenerate values unvalidated; millisecond quantization silently truncates to zero
- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-58` (`set_io_frame_buffer_cap`), `61-64` (`set_server_poll_interval`), `73-75` (`set_conn_max_in_flight`); truncation also on the Default path at `:29`
- **Severity:** medium
- **Issue:** `set_conn_max_in_flight(0)`, `set_io_frame_buffer_cap(0)` and `set_server_poll_interval(Duration::ZERO)` all store verbatim. The wired-path consumer already needs a defensive clamp — `request_loop.rs:153` does `ctx.max_in_flight.max(1)` — because `Semaphore::new(0)` + `mpsc::channel(0)` would block every request on the connection forever. Additionally `server_poll_interval` is stored as `v.as_millis() as u64`: `Duration::from_micros(900)` becomes `Duration::ZERO`, and absurd `Duration` values wrap on the `u128 → u64` cast. `SERVER_POLL_INTERVAL = 0` would turn the three housekeeping loops (`server_launcher.rs:1008/1105/1210`) into hot spins once wired.
- **Failure scenario:** latent until finding 1's wiring lands; then a single bad override (a `0` from a misparsed config, a sub-ms duration) wedges new connections or burns a core, with no validation error pointing at the setter.
- **Suggested fix:** clamp and document the policy inside the setters (e.g. `v.max(1)` for the two `usize` knobs; `Duration::from_millis(v.as_millis().min(u64::MAX as u128).max(1))` for the interval), then pin the policy with red/green tests (see finding 4).

### 3. Phantom env override documented: `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` is read nowhere
- **File:line:** `crates/shamir-tunables/src/lib.rs:149` (doc of `VECTOR_SNAPSHOT_DELTA_THRESHOLD`: "`SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` overrides at startup.")
- **Severity:** medium
- **Issue:** a workspace-wide search finds this identifier only in this doc comment. The only consumer, `shamir-index/src/vector/vector_backend.rs:143`, initializes from the const with no environment read anywhere on the path. The documented override mechanism is fiction, which contradicts this crate's stated role as the single truthful home for knob documentation.
- **Failure scenario:** an operator sets the env var at startup expecting to bound the restart-replay / orphan-chunk footprint; it is silently ignored and capacity planning built on the override is wrong.
- **Suggested fix:** either implement the startup env read at the consumer (and keep this line as its doc anchor) or delete the sentence; if planned-not-built, say "planned" explicitly.

### 4. TDD coverage gaps on the runtime surface: degenerate inputs, truncation, and override-effect never tested
- **File:line:** `crates/shamir-tunables/src/tests/runtime_tests.rs` (whole file)
- **Severity:** low
- **Issue:** the five tests cover defaults-equal-consts and happy-path set/get. `defaults_equal_consts` is genuinely load-bearing (it is what catches a future `#[derive(Default)]`, whose atomic defaults would be zeros) — but per CLAUDE.md's Red/Green/Refactor the edge cases that drove findings 1-2 were never written first: no test sets `0` / `Duration::ZERO` / sub-ms durations (which would have forced the clamp/truncation policy decision in Red), no double-overwrite test, and — the real vacuity — no test anywhere ties an override to observable consumer behavior (currently impossible, since no consumer reads the struct; that is finding 1's root). The current lossy `as_millis()` behavior is thus neither documented as contract nor pinned by a test.
- **Failure scenario:** the suite stays green while the override contract drifts; a future wiring change can silently alter truncation/clamping semantics with no failing test.
- **Suggested fix:** after findings 1-2: add tests asserting the clamp policy for `0`/sub-ms inputs and overwrite-twice semantics; add one shamir-server integration test asserting that lowering `tunables.server_poll_interval()` changes observed poll cadence (the test that would have caught finding 1 in Red).

### 5. `lib.rs` header doc stale relative to shipped `runtime.rs`
- **File:line:** `crates/shamir-tunables/src/lib.rs:3-7`
- **Severity:** nit
- **Issue:** "Today these are plain `const`s (change = edit here + rebuild + benchmark via /opti); a later phase promotes selected knobs to a runtime cascade" — the promotion already partially exists (`runtime.rs`, 3 knobs). Harmless alone, but combined with finding 1 it gives contradictory impressions of what is live.
- **Suggested fix:** one sentence: consts are authoritative today; the `RuntimeTunables` overrides exist but are not yet consumed (see finding 1).

</details>
