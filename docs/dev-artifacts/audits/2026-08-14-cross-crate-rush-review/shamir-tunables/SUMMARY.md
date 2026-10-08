<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tunables — SUMMARY revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

All original consolidated rows and plan items were revalidated. No implementation fix was found. The two main API roots and phantom env promise remain; two design/layout defect claims are refuted, and predicted deadlock/CPU consequences require correction.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 22 | 20 | 0 | 0 | 2 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1-1"></a>

### Claim 1.1 — Runtime override API is unwired: all three setters are silent no-ops for real behavior

Status: `confirmed-open`. Current risk: `medium`.

No production getters exist. The exposed object is created after listener setup, while contexts, buffers, and all five accept-error sleeps use constants. Deliberate deferral is documented elsewhere but not beside the API promise.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:985](../../../../../crates/shamir-server/src/server/server_launcher.rs#L985); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048); [crates/shamir-server/src/connection/handshake.rs:706](../../../../../crates/shamir-server/src/connection/handshake.rs#L706); [docs/dev-artifacts/roadmap/TUNABLES.md:179](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L179).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — Phantom env override documented: `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` is read nowhere

Status: `confirmed-open`. Current risk: `medium`.

The promise remains; construction directly uses the constant and the only override method is test-only.

Evidence: [crates/shamir-tunables/src/lib.rs:149](../../../../../crates/shamir-tunables/src/lib.rs#L149); [crates/shamir-index/src/vector/vector_backend.rs:143](../../../../../crates/shamir-index/src/vector/vector_backend.rs#L143); [crates/shamir-index/src/vector/vector_backend.rs:160](../../../../../crates/shamir-index/src/vector/vector_backend.rs#L160).

Grouping/duplicate: `correctness-tdd.md#3`. This row is not another independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — TDD coverage gaps on the runtime surface: degenerate inputs, truncation, and override-effect never tested

Status: `confirmed-open`. Current risk: `low`.

All five tests remain reachable but only check default values, normal round trips, and same-thread shared references. Boundary and server-effect regressions cannot be detected.

Evidence: [crates/shamir-tunables/src/lib.rs:11](../../../../../crates/shamir-tunables/src/lib.rs#L11); [crates/shamir-tunables/src/tests/mod.rs:1](../../../../../crates/shamir-tunables/src/tests/mod.rs#L1); [crates/shamir-tunables/src/tests/runtime_tests.rs:7](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L7); [crates/shamir-tunables/src/tests/runtime_tests.rs:52](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L52).

Grouping/duplicate: `correctness-tdd.md#4`. This row is not another independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — setters accept degenerate values unvalidated; millisecond quantization silently truncates to zero

Status: `confirmed-open`. Current risk: `medium`.

Unchecked stores and narrowing remain. Current Default is exactly 50 ms; zero-cap stalls are prevented by the existing request-loop floor.

Evidence: [crates/shamir-tunables/src/runtime.rs:29](../../../../../crates/shamir-tunables/src/runtime.rs#L29); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — `lib.rs` header doc stale relative to shipped `runtime.rs`

Status: `confirmed-open`. Current risk: `nit`.

The plain-constants/future-promotion paragraph still fails to explain the existing unwired runtime foundation.

Evidence: [crates/shamir-tunables/src/lib.rs:4](../../../../../crates/shamir-tunables/src/lib.rs#L4); [crates/shamir-tunables/src/lib.rs:9](../../../../../crates/shamir-tunables/src/lib.rs#L9).

Grouping/duplicate: `correctness-tdd.md#5`. This row is not another independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — the crate's only concurrency primitive has zero live consumers

Status: `confirmed-open`. Current risk: `medium`.

The handle owns the atomics, but production still reads constants. Atomic ordering is not the problem.

Evidence: [crates/shamir-server/src/server/server_handle.rs:100](../../../../../crates/shamir-server/src/server/server_handle.rs#L100); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-2-2"></a>

### Claim 2.2 — setters accept concurrency-breaking values (0 / sub-millisecond) with no validation or documented floor

Status: `confirmed-open`. Current risk: `low`.

The domain gap remains latent; current max(1) contradicts the asserted zero-permit failure after simple wiring, and mandatory starvation is unproven.

Evidence: [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-tunables/src/runtime.rs:73](../../../../../crates/shamir-tunables/src/runtime.rs#L73); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-2-3"></a>

### Claim 2.3 — No test pins cross-thread visibility of an override; `reads_are_shared_ref` runs single-threaded

Status: `confirmed-open`. Current risk: `nit`.

The only Arc test is still single-threaded; no join-based visibility regression or explicit Send + Sync assertion exists.

Evidence: [crates/shamir-tunables/src/tests/runtime_tests.rs:52](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L52).

Grouping/duplicate: `concurrency-lockfree.md#3`. This row is not another independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — the runtime layer is dead in production, so the "override takes effect" contract is false for every security-relevant knob it mirrors

Status: `confirmed-open`. Current risk: `low`.

Concurrency and backoff overrides remain inert, but existing constant-based controls operate. Idle timeout is not mirrored by this struct.

Evidence: [crates/shamir-tunables/src/runtime.rs:18](../../../../../crates/shamir-tunables/src/runtime.rs#L18); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048); [crates/shamir-server/src/server/server_launcher.rs:1049](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1049).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — setters accept unvalidated values that can disable security-relevant resource bounds

Status: `confirmed-open`. Current risk: `low`.

This remains trusted-caller boundary hardening, not a reachable network exploit. Production has no setter consumers and the request-loop cap floor remains.

Evidence: [crates/shamir-tunables/src/runtime.rs:56](../../../../../crates/shamir-tunables/src/runtime.rs#L56); [crates/shamir-tunables/src/runtime.rs:73](../../../../../crates/shamir-tunables/src/runtime.rs#L73); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — degenerate setter values are a latent busy-spin / zero-permit stall trap for the future config cascade

Status: `confirmed-open`. Current risk: `low`.

Unchecked values remain, but only future wiring can expose allocation/backoff effects. Current source does not support inevitable zero-cap stalls or 100%-core spins.

Evidence: [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153); [crates/shamir-server/src/server/server_launcher.rs:1098](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1098).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — the advertised runtime-tuning surface is presently inert, so bench "retunes" measure noise

Status: `confirmed-open`. Current risk: `low`.

The server does not read the object. Setters therefore cannot causally change these server operations; no benchmark result was inspected or generated.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:985](../../../../../crates/shamir-server/src/server/server_launcher.rs#L985); [crates/shamir-server/src/connection/handshake.rs:706](../../../../../crates/shamir-server/src/connection/handshake.rs#L706).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — public API documented as effective but unwired

Status: `confirmed-open`. Current risk: `medium`.

The setter claims are unchanged and still lack the explicit production-deferral note present on ServerHandle.

Evidence: [crates/shamir-tunables/src/runtime.rs:55](../../../../../crates/shamir-tunables/src/runtime.rs#L55); [crates/shamir-server/src/server/server_handle.rs:98](../../../../../crates/shamir-server/src/server/server_handle.rs#L98).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — setters accept out-of-domain values silently; millisecond truncation is undocumented

Status: `confirmed-open`. Current risk: `medium`.

No valid-domain, precision, saturation, or rejection policy has been added.

Evidence: [crates/shamir-tunables/src/runtime.rs:55](../../../../../crates/shamir-tunables/src/runtime.rs#L55); [crates/shamir-tunables/src/runtime.rs:60](../../../../../crates/shamir-tunables/src/runtime.rs#L60); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — Runtime knob selection is asymmetric within a single consumption site

Status: `refuted`. Current risk: —.

The roadmap explicitly chooses promotion on genuine need, not symmetry between neighboring context fields. No idle-timeout override is promised; future application timing belongs to the wiring design.

Evidence: [docs/dev-artifacts/roadmap/TUNABLES.md:172](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L172); [docs/dev-artifacts/roadmap/TUNABLES.md:179](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L179); [crates/shamir-tunables/src/runtime.rs:18](../../../../../crates/shamir-tunables/src/runtime.rs#L18).

Grouping/duplicate: `api-wire-protocol.md#3`. This row is not another independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Test directory placement deviates from the per-module `tests/` convention

Status: `refuted`. Current risk: —.

The crate root owns and registers a single manifest-only tests directory. Neither the rules nor the explicit historical migration require these root tests to move under runtime.

Evidence: [crates/shamir-tunables/src/lib.rs:11](../../../../../crates/shamir-tunables/src/lib.rs#L11); [crates/shamir-tunables/src/tests/mod.rs:1](../../../../../crates/shamir-tunables/src/tests/mod.rs#L1); [AGENTS.md:127](../../../../../AGENTS.md#L127).

Grouping/duplicate: `api-wire-protocol.md#4`. This row is not another independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — Infallible setters accept out-of-domain values: downstream panics, zero-permit deadlocks, truncating-cast wraps

Status: `confirmed-open`. Current risk: `medium`.

Unchecked usize inputs and lossy duration storage remain. Oversized future allocations/permit counts need upper policies; current max(1) blocks the zero-cap mechanism. Duration::MAX specifically narrows to u64::MAX, while other huge durations can wrap short.

Evidence: [crates/shamir-tunables/src/runtime.rs:56](../../../../../crates/shamir-tunables/src/runtime.rs#L56); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-tunables/src/runtime.rs:73](../../../../../crates/shamir-tunables/src/runtime.rs#L73); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153); [Cargo.lock:4196](../../../../../Cargo.lock#L4196).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — dead plumbing: runtime override path has zero readers

Status: `confirmed-open`. Current risk: `low`.

No production reads have been introduced; public handle storage and constant-fed consumers remain unchanged.

Evidence: [crates/shamir-server/src/server/server_handle.rs:100](../../../../../crates/shamir-server/src/server/server_handle.rs#L100); [crates/shamir-server/src/server/server_launcher.rs:985](../../../../../crates/shamir-server/src/server/server_launcher.rs#L985); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — no boundary/error-path tests for the runtime setters

Status: `confirmed-open`. Current risk: `low`.

The registered tests still omit all cited boundary cases. Such tests can be written red before implementing the policy.

Evidence: [crates/shamir-tunables/src/tests/mod.rs:1](../../../../../crates/shamir-tunables/src/tests/mod.rs#L1); [crates/shamir-tunables/src/tests/runtime_tests.rs:27](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L27).

Grouping/duplicate: `correctness-tdd.md#4`. This row is not another independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — `lib.rs` embeds two definition modules inline instead of the workspace's manifest-style `lib.rs`

Status: `confirmed-open`. Current risk: `low`.

The proposed split has not occurred. The observation is optional style consistency, not a hard violation of the literal mod.rs rule or closely-coupled export allowance.

Evidence: [crates/shamir-tunables/src/lib.rs:17](../../../../../crates/shamir-tunables/src/lib.rs#L17); [crates/shamir-tunables/src/lib.rs:31](../../../../../crates/shamir-tunables/src/lib.rs#L31); [CLAUDE.md:503](../../../../../CLAUDE.md#L503); [CLAUDE.md:505](../../../../../CLAUDE.md#L505).

Grouping/duplicate: `style-claude-md.md#1`. This row is not another independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — Crate-level doc is stale relative to the shipped `runtime` module

Status: `confirmed-open`. Current risk: `nit`.

The build-time/future-promotion paragraph and description remain; neither explains the existing inert runtime foundation.

Evidence: [crates/shamir-tunables/src/lib.rs:4](../../../../../crates/shamir-tunables/src/lib.rs#L4); [crates/shamir-tunables/Cargo.toml:6](../../../../../crates/shamir-tunables/Cargo.toml#L6).

Grouping/duplicate: `correctness-tdd.md#5`. This row is not another independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — `RuntimeTunables` struct doc duplicates the module doc nearly verbatim

Status: `confirmed-open`. Current risk: `nit`.

Both semantic documentation copies remain unchanged.

Evidence: [crates/shamir-tunables/src/runtime.rs:1](../../../../../crates/shamir-tunables/src/runtime.rs#L1); [crates/shamir-tunables/src/runtime.rs:13](../../../../../crates/shamir-tunables/src/runtime.rs#L13).

Grouping/duplicate: `style-claude-md.md#2`. This row is not another independent defect.

## Current fix-plan state

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 6 | 0 | 0 | 0 | 0 | 2 |

A source-fixed item closes only its stated mechanism. Partial items retain the obligations named below; proposed fixes must obey the corrections and current contracts, not merely copy the historical recipe.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`. Current risk: —.

Neither permitted resolution has landed: consumers remain unwired, and crate-side API documentation still lacks an inert-foundation warning. Wiring would require sharing the object before listener launch, covering five backoffs, and defining per-connection sampling; replacing only boot-time build_ctx reads is insufficient.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:735](../../../../../crates/shamir-server/src/server/server_launcher.rs#L735); [crates/shamir-server/src/server/server_launcher.rs:985](../../../../../crates/shamir-server/src/server/server_launcher.rs#L985); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048); [crates/shamir-server/src/server/server_launcher.rs:1442](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1442); [crates/shamir-tunables/src/lib.rs:4](../../../../../crates/shamir-tunables/src/lib.rs#L4).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`. Current risk: —.

The phantom startup environment promise was neither removed/reworded nor implemented.

Evidence: [crates/shamir-tunables/src/lib.rs:149](../../../../../crates/shamir-tunables/src/lib.rs#L149); [crates/shamir-index/src/vector/vector_backend.rs:143](../../../../../crates/shamir-index/src/vector/vector_backend.rs#L143); [crates/shamir-index/src/vector/vector_backend.rs:160](../../../../../crates/shamir-index/src/vector/vector_backend.rs#L160).

<a id="plan-p1-3"></a>

### Plan P1.3 — P1.3

Status: `confirmed-open`. Current risk: —.

No domain policy exists. Document or validate precision/range before wiring; include an upper request-cap limit compatible with Tokio. A mandatory nonzero initial Vec capacity is not justified, and lower floors alone do not address oversized inputs.

Evidence: [crates/shamir-tunables/src/runtime.rs:56](../../../../../crates/shamir-tunables/src/runtime.rs#L56); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-tunables/src/runtime.rs:73](../../../../../crates/shamir-tunables/src/runtime.rs#L73); [crates/shamir-server/src/connection/request_loop.rs:154](../../../../../crates/shamir-server/src/connection/request_loop.rs#L154); [Cargo.lock:4196](../../../../../Cargo.lock#L4196).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`. Current risk: —.

Boundary, wrap, repeat-update, cross-thread, Send + Sync, and consumer-effect assertions remain absent. The poll-effect test must exercise accept failures deterministically; ordinary idle cadence is not an oracle for these sleeps.

Evidence: [crates/shamir-tunables/src/tests/runtime_tests.rs:7](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L7); [crates/shamir-tunables/src/tests/runtime_tests.rs:52](../../../../../crates/shamir-tunables/src/tests/runtime_tests.rs#L52); [crates/shamir-server/src/server/server_launcher.rs:1093](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1093); [crates/shamir-server/src/server/server_launcher.rs:1098](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1098).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `not-applicable`. Current risk: —.

Mandatory idle-timeout promotion/symmetry is unsupported: selection criteria already explicitly say build on genuine need. If P0.1 chooses live wiring, define sampling/application timing then; no current idle-runtime contract requires implementing a new knob.

Evidence: [docs/dev-artifacts/roadmap/TUNABLES.md:172](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L172); [docs/dev-artifacts/roadmap/TUNABLES.md:179](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L179); [crates/shamir-tunables/src/runtime.rs:18](../../../../../crates/shamir-tunables/src/runtime.rs#L18); [crates/shamir-server/src/server/server_launcher.rs:1023](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1023).

<a id="plan-p2-6"></a>

### Plan P2.6 — P2.6

Status: `confirmed-open`. Current risk: —.

The namespace split remains unimplemented. Retain only as optional style consistency work, not a correctness/security prerequisite.

Evidence: [crates/shamir-tunables/src/lib.rs:17](../../../../../crates/shamir-tunables/src/lib.rs#L17); [crates/shamir-tunables/src/lib.rs:31](../../../../../crates/shamir-tunables/src/lib.rs#L31); [CLAUDE.md:505](../../../../../CLAUDE.md#L505).

<a id="plan-p2-7"></a>

### Plan P2.7 — P2.7

Status: `confirmed-open`. Current risk: —.

The duplicate module/struct documentation has not been consolidated.

Evidence: [crates/shamir-tunables/src/runtime.rs:1](../../../../../crates/shamir-tunables/src/runtime.rs#L1); [crates/shamir-tunables/src/runtime.rs:13](../../../../../crates/shamir-tunables/src/runtime.rs#L13).

<a id="plan-p2-8"></a>

### Plan P2.8 — P2.8

Status: `not-applicable`. Current risk: —.

Mandatory relocation is unnecessary: the crate-root module owns correctly wired, topic-split tests with one manifest-only directory. No amendment or migration is required to remedy a demonstrated registration/layout defect.

Evidence: [crates/shamir-tunables/src/lib.rs:11](../../../../../crates/shamir-tunables/src/lib.rs#L11); [crates/shamir-tunables/src/tests/mod.rs:1](../../../../../crates/shamir-tunables/src/tests/mod.rs#L1); [AGENTS.md:127](../../../../../AGENTS.md#L127); [CLAUDE.md:575](../../../../../CLAUDE.md#L575).

## Corrections and qualified non-findings

- Plan IDs preserve the original global item numbering: P0 items 1–2, P1 items 3–5, P2 items 6–8. Lens documents have no separate Fix Plan sections.
- The original 23 lens rows and 22 SUMMARY sections are all represented. SUMMARY 6.1 combines two lifecycle findings. Two of the original ten deduplicated groups are refuted design/layout claims; eight groups remain, including optional style/test nits.
- Remove the sole high classification: no production reader, remote setter route, or demonstrated production outage supports high runtime severity for the unwired scaffold.
- Correct SUMMARY 6.1: Tokio bounded mpsc zero capacity panics rather than becoming rendezvous; max(1) covers any input, not just constants; zero initial Vec capacity is valid; Duration::MAX narrows to u64::MAX rather than a randomly short interval.
- Qualify CPU/latency claims: zero sleep does not prove mandatory saturation/starvation, atomic reads are not literally cost-free, and universal sub-second replay or unchanged hot-path latency remains unverified.
- Remove all claims that idle timeout can be changed through RuntimeTunables. Its three fields are buffer capacity, poll interval, and request concurrency only (crates/shamir-tunables/src/runtime.rs:18).
- Update consumer references: construction at server_launcher.rs:985; cap/idle snapshots at :1048/:1049; backoffs at :1098/:1199/:1276/:1337/:1442; buffer capacities at handshake.rs:706/:708; handle field at server_handle.rs:100.
- Non-finding guarantees remain narrow: dependency-free, wire-free, no production locks/awaits/allocations/panic sites/resources; tests are reachable. Atomic independence supports Relaxed ordering, but not stronger cross-thread freshness promises.
- All constants retain production consumers. WAL threshold decoupling is still source-proven (crates/shamir-engine/src/repo/repo_instance.rs:825,830; crates/shamir-wal/src/segment_set.rs:88).
- Backpressure and vector thresholds are soft mechanisms, not unconditional memory/disk bounds: the brake abandons after five seconds, and vector snapshots are asynchronous (crates/shamir-engine/src/tx/commit.rs:526; crates/shamir-index/src/vector/vector_backend.rs:837).
- Historical CE1 completion means the atomic foundation was added, not that consumer wiring was completed. Current source and roadmap explicitly preserve that deferral; no test execution is claimed.

## Current follow-up order

1. Make the RuntimeTunables contract honest: document inert scaffolding alongside the public API, or deliberately implement shared live wiring with defined sampling semantics across all five accept-error paths and per-connection consumers.
2. Remove/reword or implement the phantom SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD startup override.
3. Before live wiring, define setter precision/ranges and oversized-input behavior, preserving the existing consumer cap floor and respecting Tokio's maximum permit count.
4. Add reachable boundary, general duration-wrap, repeat-update, cross-thread, and deterministic consumer-effect tests; retain the defaults-equal-constants guard.
5. Correct overstated failure scenarios and optional symmetry/layout claims in the review documents; treat namespace splitting and doc deduplication as low-priority style work.

## Coverage and limitations

- All eight Markdown files were read completely; all 23 original lens findings, 22 SUMMARY finding sections, and eight Fix Plan items were assessed. No TASK_GROUPS.md exists in this directory.
- Source-only revalidation: no files changed, child agents launched, git mutations performed, or builds, tests, benchmarks, scripts, or reproductions executed.
- HEAD matched the requested commit and the checkout was clean at both inspections. The crate has no source changes between the original review-document commit b7574932 and the requested HEAD.
- CPU utilization, latency, universal sub-second replay, and unchanged hot-path cost are unverified experimentally.
- Tokio behavior was checked against locally available source for the Cargo.lock-pinned 1.49.0 version; that third-party source is not vendored in the repository.

## Reviewed document inventory

- [correctness-tdd.md](./correctness-tdd.md) — 5 claim decisions; 0 explicit plan items.
- [concurrency-lockfree.md](./concurrency-lockfree.md) — 3 claim decisions; 0 explicit plan items.
- [security-crypto.md](./security-crypto.md) — 2 claim decisions; 0 explicit plan items.
- [performance-hotpath.md](./performance-hotpath.md) — 2 claim decisions; 0 explicit plan items.
- [api-wire-protocol.md](./api-wire-protocol.md) — 4 claim decisions; 0 explicit plan items.
- [error-handling-lifecycle.md](./error-handling-lifecycle.md) — 4 claim decisions; 0 explicit plan items.
- [style-claude-md.md](./style-claude-md.md) — 3 claim decisions; 0 explicit plan items.
- [SUMMARY.md](./SUMMARY.md) — 22 claim decisions; 8 explicit plan items.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tunables — Synthesized 7-lens review (follow-up to the 2026-08-14 cross-crate review)

Crate: `crates/shamir-tunables/` — the workspace's single home for tunable knobs: two
compile-time `const` modules (`store_defaults`, `instance_defaults`) plus one
runtime-overridable struct (`RuntimeTunables`, three relaxed-atomic accessors);
zero dependencies, no locks, no `unsafe`, no I/O.

Review basis: the seven 2026-08-14 lens files under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — synthesized into one deduplicated document. Structure, tone,
and dedup convention calibrated on the two completed exemplars
(`../shamir-client-node/SUMMARY.md`, `../shamir-transport-ipc/SUMMARY.md`) and the
workspace `../SUMMARY.md` (whose "Per-crate breakdown" row for this crate — 23
lens-tagged findings — is re-derived independently below, and matches). Read-only
synthesis: no build/test/lint commands, no source modifications. Spot-checks during
synthesis re-verified the load-bearing claims against source: the only
getter/setter call sites in the entire workspace are the crate's own 5 tests
(`src/tests/runtime_tests.rs`), and `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` occurs
in exactly one place — the doc comment that documents it. No new defects surfaced
during spot-checking.

## Executive summary

Structurally this is one of the cleanest crates in the workspace — zero dependencies,
no banned primitives, no panic sites, exemplary test layout — but it ships one
genuinely misleading public API: **`RuntimeTunables` is constructed and published on
`ServerHandle::tunables` yet read by nothing, so all three setters are silent no-ops
while the getters dutifully confirm any override** (the crate's only high finding,
flagged by 6 of the 7 lenses). Fix that first — wire the three consumer sites or mark
the surface explicitly as inert scaffolding — together with the **phantom
`SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` env override** that is documented but read
nowhere. Third: before any wiring lands, the setters must stop accepting
degenerate values (`0`, sub-ms durations that truncate/wrap to `0`, `usize::MAX`),
since validation is cheap today and becomes a production hazard the moment a consumer
exists.

---

## 1. correctness-tdd

### 1.1 — high — Runtime override API is unwired: all three setters are silent no-ops for real behavior *(primary home; also flagged by §2.1, §3.1, §4.2, §5.1, §6.2)*
- File:line: `crates/shamir-tunables/src/runtime.rs:36-76` (accessors), `:1-7`/`:13-16`
  (doc claims); evidence: `crates/shamir-server/src/server/server_handle.rs:89-93`
  (`pub tunables: Arc<RuntimeTunables>`, "wiring ... deferred to a follow-up slice"),
  `server_launcher.rs:900` (constructed) vs. the const reads every production site
  actually makes — `server_launcher.rs:958-959` (`CONN_MAX_IN_FLIGHT`/`CONN_IDLE_TIMEOUT`
  into `build_ctx`), `:1008`, `:1105`, `:1210` (all three poll loops on const
  `SERVER_POLL_INTERVAL`), `crates/shamir-server/src/connection/handshake.rs:704-707`
  (const `IO_FRAME_BUFFER_CAP`).
- Issue: `runtime.rs` promises "Overrides are rare and just store a new atomic value,
  taking effect on the next read" and "Instance-level runtime-overridable tunables".
  In the workspace the sole `RuntimeTunables` instance is constructed, stored on the
  public `ServerHandle::tunables` field, and its accessors have **zero** non-test call
  sites — grep across the workspace finds getter/setter calls only in
  `src/tests/runtime_tests.rs` (re-verified during synthesis: 11 matches, all in that
  file). Two sources of truth exist and the runtime one is already disconnected from
  all three wired behaviors. The deferral is documented only in the *consumer* crate
  (`server_handle.rs:91-92`), while this crate's own docs use present tense that
  implies a touched instance changes behavior. The lens-specific angles all reduce to
  the same root: security-relevant controls (idle timeout closing *authenticated*
  connections, per-connection concurrency bound) appear operable but are not
  (§3.1); the perf-facing purpose — avoiding rebuild+re-bench per knob change — is
  unfulfillable, and any "retune + bench" run measures noise (§4.2); the hot paths
  still read the compile-time consts, so the lock-free primitive is dead scaffolding
  (§2.1); and the API/state-vs-behavior split is the worst failure mode a knob API
  can have (§5.1).
- Failure scenario: a caller (test, SDK consumer, future ops hook) does
  `server.tunables.set_conn_max_in_flight(8)`; the setter compiles, the getter
  dutifully returns 8, and actual server behavior stays at the const `32` — a silent,
  undetectable configuration divergence with no error or warning. A bench harness
  "retunes" `io_frame_buffer_cap` and misattributes the (zero) delta to the knob; an
  operator "tunes" the idle timeout and the security control never moves.
- Suggested fix: either land the wiring (pass the shared `Arc<RuntimeTunables>` into
  `ConnectionContext` and the three poll loops and replace the const reads — small,
  mechanical; each read is one `#[inline]` relaxed load, hot-path cost unchanged), or
  until then mark the setters `#[doc(hidden)]` with a "not yet consumed by any
  call-site; overrides are no-ops" warning (or remove them / the `tunables` field), so
  the API cannot be mistaken for functional. Also reconcile `lib.rs:3-7` with
  `runtime.rs`'s present-tense claims (finding 7.2).

### 1.2 — medium — Phantom env override documented: `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` is read nowhere
- File:line: `crates/shamir-tunables/src/lib.rs:149` (doc of `VECTOR_SNAPSHOT_DELTA_THRESHOLD`:
  "`SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` overrides at startup.")
- Issue: a workspace-wide search finds this identifier only in this doc comment
  (re-verified during synthesis). The only consumer,
  `crates/shamir-index/src/vector/vector_backend.rs:143`, initializes from the const
  with no environment read anywhere on the path. The documented override mechanism is
  fiction, which contradicts this crate's stated role as the single truthful home for
  knob documentation.
- Failure scenario: an operator sets the env var at startup expecting to bound the
  restart-replay / orphan-chunk footprint; it is silently ignored and capacity
  planning built on the override is wrong.
- Suggested fix: either implement the startup env read at the consumer (and keep this
  line as its doc anchor) or delete the sentence; if planned-not-built, say "planned"
  explicitly.

### 1.3 — low — TDD coverage gaps on the runtime surface: degenerate inputs, truncation, and override-effect never tested *(primary home; also flagged by §6.3)*
- File:line: `crates/shamir-tunables/src/tests/runtime_tests.rs` (whole file — 5 tests)
- Issue: the five tests cover defaults-equal-consts and happy-path set/get.
  `defaults_equal_consts` is genuinely load-bearing (it is what catches a future
  `#[derive(Default)]`, whose atomic defaults would be zeros) — but per CLAUDE.md's
  Red/Green/Refactor the edge cases that drove findings 1.1/6.1 were never written
  first: no test sets `0` / `Duration::ZERO` / sub-ms durations / `Duration::MAX`
  (which would have forced the clamp/truncation policy decision in Red), no
  double-overwrite test, and — the real vacuity — no test anywhere ties an override to
  observable consumer behavior (currently impossible, since no consumer reads the
  struct; that is finding 1.1's root). The error-handling lens adds: the missing
  boundary tests are unwritable until finding 6.1 gives the setters defined behavior —
  a direct consequence of the API having no error paths. The current lossy
  `as_millis()` behavior is thus neither documented as contract nor pinned by a test.
- Failure scenario: the suite stays green while the override contract drifts; a
  refactor of the storage representation (millis→micros, `AtomicU64`→`AtomicUsize`)
  can silently change truncation/wrap semantics with no failing test.
- Suggested fix: after findings 1.1/6.1: add tests asserting the clamp policy for
  `0`/sub-ms/overflow inputs per knob and overwrite-twice semantics; add one
  shamir-server integration test asserting that lowering
  `tunables.server_poll_interval()` changes observed poll cadence (the Red test that
  would have caught finding 1.1); keep `defaults_equal_consts` as the drift guard it
  already is.

### 1.4 — medium — *(primary: same as 6.1)* — setters accept degenerate values unvalidated; millisecond quantization silently truncates to zero
- (Full write-up at 6.1; listed here because correctness-tdd flagged it independently —
  the truncation also exists on the `Default` path, `runtime.rs:29`.)

### 1.5 — nit — *(primary: same as 7.2)* — `lib.rs` header doc stale relative to shipped `runtime.rs`
- (Full write-up at 7.2; flagged by both lenses as the same root — the "(future)"
  framing contradicts the module declared at `lib.rs:9`.)

## 2. concurrency-lockfree

*General verdict: exemplary.* Pure-`const` module plus one three-field atomic struct —
zero locks, zero `.await`s, no hash-keyed state (no dependencies at all), so none of
the banned hot-path primitives can exist here. Every atomic access is correctly
`Ordering::Relaxed` (independent knobs, no cross-variable ordering promised), and the
"zero-overhead read" contention model is documented inline as the ideology requires.
The substantive issues are adjacent rather than pillar violations:

### 2.1 — medium — *(primary: same as 1.1)* — the crate's only concurrency primitive has zero live consumers
- (Full write-up at 1.1; concurrency-specific framing: the lock-free
  `RuntimeTunables` atomics are constructed once in `shamir-server` and never read —
  two sources of truth, the runtime one disconnected from all three wired behaviors.
  Memory-ordering itself is correct; the defect is wiring, not atomics.)

### 2.2 — low — *(primary: same as 6.1)* — setters accept concurrency-breaking values (0 / sub-millisecond) with no validation or documented floor
- (Full write-up at 6.1; concurrency-specific framing: a zero-permit semaphore makes
  every pipelined read `acquire()` forever — precisely the "deadlock" class
  CLAUDE.md's test discipline treats as a bug — and a 0-ms poll sleep busy-spins a
  tokio worker. The hazard is purely value-domain, not memory-ordering.)

### 2.3 — nit — No test pins cross-thread visibility of an override; `reads_are_shared_ref` runs single-threaded
- File:line: `crates/shamir-tunables/src/tests/runtime_tests.rs:52-58`
- Issue: the claim this crate exists to make is "Reads are a single atomic load
  (instant, cached, lock-free, non-blocking)" with overrides visible "on the next
  read" (`runtime.rs:1-6,13-16`). The suite covers defaults, set-then-read, and `Arc`
  callability, but never a store issued in one thread observed by a load in another.
  (A spawn-then-join pattern makes this deterministic — join establishes
  happens-before even for `Relaxed` — no flaky spin loop needed.) Coverage is
  otherwise appropriate for so small a crate, and the test layout conforms to the
  repo's `tests/` organization rules.
- Suggested fix: one test that spawns a writer thread storing a value, joins, then
  asserts the reader sees it, plus a compile-time
  `fn assert_send_sync<T: Send + Sync>()` for `RuntimeTunables`, so the lock-free
  sharing contract is pinned rather than incidental.

## 3. security-crypto

*No security surface of its own:* dependency-free, no `unsafe`, no I/O, no parsing, no
secrets handling, no timing-sensitive comparisons — only plain `const`s and relaxed
atomic loads/stores. No auth/HMAC/TLS code lives here, hence no critical/high from
this lens. The two findings are hardening-level aspects of the two deduped roots:

### 3.1 — low — *(primary: same as 1.1)* — the runtime layer is dead in production, so the "override takes effect" contract is false for every security-relevant knob it mirrors
- (Full write-up at 1.1; security-specific framing: `CONN_IDLE_TIMEOUT` is the control
  that closes abandoned *authenticated* connections (its doc frames it as a
  session-slot/socket-lifetime bound) and `CONN_MAX_IN_FLIGHT` is a per-connection
  resource-concurrency bound — an operator "tunes" either through
  `ServerHandle::tunables` and observes no effect: a security control that appears
  operable but is not.)

### 3.2 — low — *(primary: same as 6.1)* — setters accept unvalidated values that can disable security-relevant resource bounds
- (Full write-up at 6.1; security-specific framing: the moment a config/admin surface
  calls `set_conn_max_in_flight(0)`, every request on each new connection stalls
  permanently; a zero/sub-ms poll interval turns the accept-error backoff loop into
  busy-spin CPU burn — a self-inflicted DoS from a misconfigured knob rather than a
  validated error.)

## 4. performance-hotpath

*Clean, as expected:* no loops, no heap allocations, no collections, no locks under
`src/`; pillars 1/3 satisfied trivially, the `scc::len()` ban has no surface. The
`const` set itself (`FULL_SCAN_BATCH`, `MAX_UNDRAINED_VERSIONS`,
`JOURNAL_BACKFILL_LIMIT`, `MAX_SUBSCRIPTIONS_PER_CONNECTION`, `WAL_SEGMENT_MAX_BYTES`,
…) is precisely the anti-unbounded-growth bounding the theme asks about. Both findings
are aspects of the deduped roots:

### 4.1 — low — *(primary: same as 6.1)* — degenerate setter values are a latent busy-spin / zero-permit stall trap for the future config cascade
- (Full write-up at 6.1; performance-specific framing: `set_io_frame_buffer_cap(usize::MAX)`
  would panic at the consumer's `Vec::with_capacity` (`handshake.rs:705`); a 0-ms poll
  interval converts idle 50 ms loops into 100%-core spins.)

### 4.2 — low — *(primary: same as 1.1)* — the advertised runtime-tuning surface is presently inert, so bench "retunes" measure noise
- (Full write-up at 1.1; performance-specific framing: the crate's stated purpose —
  avoid rebuild + re-bench cycles per knob change — cannot be exercised; an operator
  sets `set_io_frame_buffer_cap(65536)`, every new connection still allocates the
  baked-in 4096, and the observed throughput/latency delta is misattributed to the
  knob.)

## 5. api-wire-protocol

*Wire exposure is nil by design:* zero-dependency crate (no serde at all), nothing
serialized, no wire formats defined; the builder-only query-construction rule is
trivially satisfied (no `serde_json`/`json!`/`from_value` anywhere under `src/`). Also
verified clean: every const has at least one live consumer (except via the runtime
gap — finding 1.1), and `WAL_SEGMENT_MAX_BYTES`' doc (`lib.rs:113-115`) records that
`shamir-wal` takes the bound as a parameter to avoid a dependency — good decoupling.
Findings:

### 5.1 — medium — *(primary: same as 1.1)* — public API documented as effective but unwired
- (Full write-up at 1.1; API-lens framing: the deferral is documented only in the
  *consumer* crate while this crate's own docs imply a touched instance does change —
  the worst quadrant for a knob API.)

### 5.2 — medium — *(primary: same as 6.1)* — setters accept out-of-domain values silently; millisecond truncation is undocumented
- (Full write-up at 6.1; API-lens framing: infallible `()`-returning setters that
  silently coerce invalid input sidestep the project's `Result<T, E>` error-handling
  rule; at minimum the valid domain — whole milliseconds, ≥ 1 — belongs in each
  setter's doc.)

### 5.3 — low — Runtime knob selection is asymmetric within a single consumption site
- File:line: `crates/shamir-tunables/src/runtime.rs:18-22` vs
  `instance_defaults::CONN_IDLE_TIMEOUT` (`crates/shamir-tunables/src/lib.rs:50-55`)
- Issue: `RuntimeTunables` promotes `conn_max_in_flight` but not `conn_idle_timeout`,
  although the sole consumer (`build_ctx`, `server_launcher.rs:958-959`) reads both
  side by side and both are instance-level defaults. When wiring lands, idle timeout
  stays compile-time-only while its sibling becomes runtime-tunable — an arbitrary
  split from an API consumer's perspective. Related semantics gap worth closing at the
  same time: the context/semaphore are snapshotted once per listener at boot, so
  "override takes effect on the next read" needs a defined rule ("applies to
  connections accepted after the override") for `conn_max_in_flight` to be honest.
- Suggested fix: promote `conn_idle_timeout` (millis in an `AtomicU64`, mirroring the
  existing pattern) alongside `conn_max_in_flight`, or document the criteria by which
  knobs are selected for the runtime cascade; define the override-effect timing.

### 5.4 — nit — Test directory placement deviates from the per-module `tests/` convention
- File:line: `crates/shamir-tunables/src/tests/runtime_tests.rs` (manifest `src/tests/mod.rs`)
- Issue: CLAUDE.md prescribes one `tests/` directory per module (e.g.
  `src/types/tests/`); the `runtime` module's tests live in a crate-root `src/tests/`
  instead. The layout is otherwise compliant — manifest-only `mod.rs`, one
  topic-split file, wired via `#[cfg(test)] mod tests;` in `lib.rs`, no inline test
  blocks — and with a single testable module it is harmless today, but it will
  fragment as knobs get promoted. Coverage itself is appropriate for what the API
  currently does (see finding 1.3 for the actual gaps).
- Suggested fix: move to `src/runtime/tests/` on the next touch, or amend the
  convention to sanction crate-root `src/tests/` for single-module crates.

## 6. error-handling-lifecycle

*Near-trivial surface:* no `unwrap`/`expect`/`panic!`/`todo!` in `src/` (only test
asserts); no `anyhow`/`Box<dyn Error>` leakage; no OS or task resources held, so
error-path cleanup and `Drop` needs are nil. The theme's real exposure is API-shaped —
the runtime setters are infallible yet accept values that arm downstream panics,
deadlocks, or busy-spins (latent today precisely because of finding 1.1):

### 6.1 — medium — Infallible setters accept out-of-domain values: downstream panics, zero-permit deadlocks, truncating-cast wraps *(primary home — flagged by 6 of 7 lenses: also §1.4, §2.2, §3.2, §4.1, §5.2)*
- File:line: `crates/shamir-tunables/src/runtime.rs:56-58` (`set_io_frame_buffer_cap`),
  `:61-64` (`set_server_poll_interval`), `:73-75` (`set_conn_max_in_flight`);
  truncation also on the `Default` path at `:29`. Consumer semantics:
  `crates/shamir-server/src/connection/handshake.rs:704-707` (`Vec::with_capacity`),
  `crates/shamir-server/src/connection/request_loop.rs:153-155` (`Semaphore::new` +
  `mpsc::channel`), `crates/shamir-server/src/server/server_launcher.rs:1008/1105/1210`
  (poll/backoff sleeps).
- Issue: all three setters store the given value verbatim — no validation, no
  documented valid range, no `Result` — contra the CLAUDE.md error-handling pillar
  ("Return `Result<T, E>`"; panics are for programmer bugs). The values' only known
  consumers are allocation/concurrency primitives: `io_frame_buffer_cap` feeds
  `Vec::with_capacity(...)` (where `usize::MAX` is a hard capacity-overflow panic),
  and `conn_max_in_flight` feeds a per-connection `Semaphore::new(cap)` +
  `mpsc::channel(cap)` (where `0` permits stall every pipelined request forever, and
  `0`-capacity mpsc degrades to rendezvous semantics) — the existing defensive
  `.max(1)` clamp at `request_loop.rs:153` lives in the *wrong crate* and covers only
  the const-fed path. `set_server_poll_interval` narrows through
  `v.as_millis() as u64`: sub-millisecond durations (e.g. `Duration::from_micros(999)`)
  silently become `0`, `Duration::MAX.as_millis()` (~1.8e22) exceeds `u64::MAX` by
  ~1000× so the `as` cast wraps to an arbitrary garbage interval, and `Duration::ZERO`
  is accepted outright — a 0-ms sleep turns every consumer housekeeping/backoff loop
  into a yield-only busy spin (unbounded CPU burn in a loop whose purpose is to *not*
  burn CPU after accept errors).
- Failure scenario: latent today — every server call-site reads the compile-time
  consts (finding 1.1). The moment the already-plumbed `ServerHandle.tunables` reads
  go live, a single misparsed-config `0` or sub-ms value wedges all new connections
  (zero-permit semaphore) or pins a tokio worker (0-ms spin), and a wrapped
  `Duration::MAX` yields a randomly-small retry interval instead of "very long" —
  with no validation error pointing at the setter. The setters' first live callers
  would also be their first testers.
- Suggested fix: make the setters honest per the house rules — either return
  `Result<(), TunablesError>` (a small `thiserror` enum; the crate currently has
  none) or validate-and-clamp at the boundary: `max(1)` floors for the two `usize`
  knobs, a sane upper ceiling for the buffer cap, saturate instead of truncate
  (`u64::try_from(v.as_millis()).unwrap_or(u64::MAX)`), floor the interval at 1 ms
  (`max(1, v.as_millis())`), and document the millisecond precision. Alternative
  shape: take `NonZeroUsize`/`NonZeroU64` and store saturating values. The ≥ 1
  invariant must be owned by this type, not re-derived per consumer. Land this with
  (or before) finding 1.1's wiring.

### 6.2 — low — *(primary: same as 1.1)* — dead plumbing: runtime override path has zero readers
- (Full write-up at 1.1; lifecycle framing: not a runtime failure but a
  hygiene/lifecycle one — the validation gaps in 6.1 stay invisible until someone
  flips the call-sites over, and the scaffolding status is unmarked.)

### 6.3 — low — *(primary: same as 1.3)* — no boundary/error-path tests for the runtime setters
- (Full write-up at 1.3; error-lens framing: the missing error-path tests are
  unwritable until 6.1 gives the setters defined behavior — the API has no error
  paths at all today.)

## 7. style-claude-md

*Largely exemplary:* `src/tests/mod.rs` is a manifest-only re-export wired via
`#[cfg(test)] mod tests;`; tests are topic-split with no inline `#[cfg(test)]` blocks
in implementation files; every `use` sits at a file/module header (including
`use super::Duration;` at the `instance_defaults` module header — the sanctioned
enclosing-module exception); test coverage matches the crate surface. Findings:

### 7.1 — low — `lib.rs` embeds two definition modules inline instead of the workspace's manifest-style `lib.rs`
- File:line: `crates/shamir-tunables/src/lib.rs:17-160`
- Issue: CLAUDE.md's discipline rules state "`mod.rs` files contain re-exports only.
  Types and logic live in sibling files" and "One file = one primary export ... This
  keeps diffs atomic and `git blame` meaningful." `lib.rs` plays the crate-root
  `mod.rs` role, yet instead of declaring `pub mod store_defaults;` /
  `pub mod instance_defaults;` it defines both modules inline (~140 lines, 17 consts).
  Sampled sibling crates follow the sibling-file pattern: `shamir-numa/src/lib.rs` and
  `shamir-query-types/src/lib.rs` are pure `mod` + `pub use` manifests. The rule's
  letter names `mod.rs` (not `lib.rs`) and the two namespaces are a closely-coupled
  group, so this is not a hard violation — but this is the only crate root sampled
  that carries definitions, and it is the documented growth surface ("a later phase
  promotes selected knobs to a runtime cascade"), so it will keep accreting.
- Failure scenario: as tunables are added, `lib.rs` diffs mix unrelated knob families,
  eroding the atomic-diff/blame rationale behind the rule, and the crate becomes the
  off-pattern template copied by future crates.
- Suggested fix: split verbatim into `src/instance_defaults.rs` and
  `src/store_defaults.rs`, leaving `lib.rs` as `pub mod runtime;` + the two module
  declarations + `#[cfg(test)] mod tests;`, matching `shamir-numa`/`shamir-query-types`.
  Land it as a standalone `style:`/`chore:` commit per the code-quality rules.

### 7.2 — nit — Crate-level doc is stale relative to the shipped `runtime` module *(primary home; also flagged by §1.5)*
- File:line: `crates/shamir-tunables/src/lib.rs:1-7` (same framing in the `Cargo.toml`
  `description`)
- Issue: the `//!` crate doc says "Today these are plain `const`s ... a later phase
  promotes selected knobs to a runtime cascade" and never mentions `pub mod runtime;`
  declared directly below it (line 9). `runtime::RuntimeTunables` already is that
  promotion for three instance-level knobs, so the crate doc understates the crate's
  contents — and combined with finding 1.1 gives contradictory impressions of what is
  live.
- Failure scenario: a consumer reading only the crate docs concludes runtime overrides
  don't exist yet and hard-codes a redundant const-copy workaround; the "(future)"
  framing misleads contributors about the module's status.
- Suggested fix: add one sentence: consts are authoritative today; the
  `RuntimeTunables` overrides exist but are not yet consumed (see finding 1.1).
  Optionally refresh the Cargo.toml description.

### 7.3 — nit — `RuntimeTunables` struct doc duplicates the module doc nearly verbatim
- File:line: `crates/shamir-tunables/src/runtime.rs:13-16` (vs. module doc `runtime.rs:1-7`)
- Issue: the struct doc repeats the `//!` module doc's three sentences ("Reads are a
  single atomic load (instant, cached, lock-free ...); overrides store a new value.
  Initialized from the compiled `instance_defaults` consts ...") almost word-for-word.
  Redundant copies drift independently.
- Failure scenario: a future change to override semantics (ordering, visibility,
  invalidation) updated in one copy but not the other leaves contradictory docs that
  rustdoc renders on the same page.
- Suggested fix: keep the semantics in one place (module doc) and reduce the struct
  doc to a single line, e.g. "Instance-level runtime-overridable tunables; see module
  docs."

---

## Finding counts

Raw = every explicitly severity-tagged finding across the 7 lens files, as filed
(matches the workspace SUMMARY's pre-dedup per-crate row of 23 for this crate).
Deduped = distinct root-cause defects after merging same-root findings under their
primary lens (dedup groups noted in each finding title).

| Severity | Raw lens-tagged | Deduped distinct | Raw findings (section refs) | Deduped findings |
|---|---|---|---|---|
| critical | 0 | 0 | — | — |
| high | 1 | 1 | §1.1 | 1.1 (raw ×6: §1.1, §2.1, §3.1, §4.2, §5.1, §6.2) |
| medium | 7 | 2 | §1.2, §1.4, §2.1, §5.1, §5.2, §6.1 (×2) | 1.2 (×1) · 6.1 (raw ×7: §6.1 ×2, §1.4, §2.2, §3.2, §4.1, §5.2) |
| low | 10 | 3 | §1.3, §2.2, §3.1, §3.2, §4.1, §4.2, §5.3, §6.2, §6.3, §7.1 | 1.3 (raw ×2: §1.3, §6.3) · 5.3 (×1) · 7.1 (×1) |
| nit | 5 | 4 | §1.5, §2.3, §5.4, §7.2, §7.3 | 2.3 (×1) · 5.4 (×1) · 7.2 (raw ×2: §7.2, §1.5) · 7.3 (×1) |
| **total** | **23** | **10** | 1 high · 7 medium · 10 low · 5 nit | 1 high · 2 medium · 3 low · 4 nit |

*Cross-check: the raw column re-derives the workspace SUMMARY's pre-dedup count for
shamir-tunables (0c / 1h / 7m / 10l / 5n = 23) exactly; the deduped column is this
document's distinct-defect view. Severity of a dedup group = the highest severity any
lens tagged it with.*

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Resolve the unwired `RuntimeTunables` contract (1.1):** either wire the three
   consumer sites (replace the const reads at `server_launcher.rs:958-959/1008/1105/1210`
   and `handshake.rs:704-707` with `tunables.*()` reads where the handle is in scope —
   mechanical, hot-path cost unchanged) **or**, if the cascade phase is still distant,
   mark the surface honestly (`#[doc(hidden)]` on the setters / explicit
   "no consumer yet; overrides are currently inert" doc on the struct, or remove the
   `tunables` field until wiring lands). Either way, fix the crate doc (7.2) in the
   same commit so docs and reality agree. Closes: 1.1, 7.2 (and the lens aspects
   logged at §2.1, §3.1, §4.2, §5.1, §6.2).
2. **Kill the phantom env override (1.2):** implement the startup env read at the
   `shamir-index` consumer, or delete/reword the `lib.rs:149` sentence — one-line
   docs-only fix in the worst case; never leave an operator-facing knob documented
   but nonexistent. Closes: 1.2.

**P1 — soon**

3. **Setter validation/domain policy (6.1)** — clamp-or-`Result` at the boundary:
   `max(1)` floors for both `usize` knobs, sane ceiling for the buffer cap, saturate
   the `as_millis() as u64` cast, floor the poll interval at 1 ms, document the
   domain. Must land **with or before** item 1's wiring — validation is latent today
   and becomes load-bearing the moment a consumer exists. Closes: 6.1 (and §1.4,
   §2.2, §3.2, §4.1, §5.2).
4. **Boundary + effect tests (1.3, 2.3)** per Red/Green: degenerate-input tests
   (`0`, sub-ms, `Duration::MAX`, double-overwrite) pinning the item-3 policy; one
   shamir-server integration test asserting an override changes observed poll cadence
   (the Red test for 1.1); a spawn-then-join cross-visibility test plus a compile-time
   `Send + Sync` assertion. Closes: 1.3, 2.3 (and §6.3).
5. **Knob-selection symmetry + override timing (5.3):** promote
   `conn_idle_timeout` (AtomicU64 millis) alongside `conn_max_in_flight`, or document
   the selection criteria; define "takes effect on the next read" as "applies to
   connections accepted after the override". Natural to fold into item 1's wiring
   diff. Closes: 5.3.

**P2 — backlog**

6. **Manifest-style `lib.rs` (7.1):** split verbatim into
   `src/instance_defaults.rs` + `src/store_defaults.rs` as a standalone
   `style:`/`chore:` commit. Closes: 7.1.
7. **Doc dedup (7.3):** reduce the struct doc to one line pointing at the module doc.
   Closes: 7.3.
8. **Test-directory placement (5.4):** move `src/tests/` → `src/runtime/tests/` on
   the next touch, or amend CLAUDE.md to sanction crate-root `src/tests/` for
   single-module crates. Closes: 5.4.

</details>
