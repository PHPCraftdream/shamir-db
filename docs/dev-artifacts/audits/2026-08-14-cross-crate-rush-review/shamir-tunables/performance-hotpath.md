<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tunables — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Production tunable operations remain constant-work and allocation-free by inspection. The two findings remain latent API issues, not measured performance regressions.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Runtime setters accept degenerate values with no clamping — latent busy-spin / zero-permit stall once the cascade goes live

Status: `confirmed-open`. Current risk: `low`.

Raw values and narrowing duration conversion remain. Shortened error backoff and oversized allocations are conditional future risks; current consumers are disconnected and the request cap is defensively floored.

Evidence: [crates/shamir-tunables/src/runtime.rs:56](../../../../../crates/shamir-tunables/src/runtime.rs#L56); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-server/src/connection/request_loop.rs:153](../../../../../crates/shamir-server/src/connection/request_loop.rs#L153); [crates/shamir-server/src/server/server_launcher.rs:1098](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1098).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `RuntimeTunables` is dead-wired: constructed and carried, but zero production readers — advertised runtime tuning is a no-op

Status: `confirmed-open`. Current risk: `low`.

Changing the exposed object cannot change the server's two initial buffer allocations or five constant-backed accept-error sleeps. Any claimed benchmark improvement from those setters would lack this causal mechanism.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:985](../../../../../crates/shamir-server/src/server/server_launcher.rs#L985); [crates/shamir-server/src/connection/handshake.rs:706](../../../../../crates/shamir-server/src/connection/handshake.rs#L706); [crates/shamir-server/src/connection/handshake.rs:708](../../../../../crates/shamir-server/src/connection/handshake.rs#L708); [crates/shamir-server/src/server/server_launcher.rs:1442](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1442).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

## Corrections and qualified non-findings

- Qualify 'no allocations under src': production implementation allocates nothing, but reads_are_shared_ref allocates an Arc (crates/shamir-tunables/src/tests/runtime_tests.rs:53).
- Single atomic load/store and no loops establish bounded structural work, not literally zero hardware overhead or unchanged cost versus a compiled constant. Those latency claims are unverified.
- The five sleeps run on accept failures, not periodic idle housekeeping. A cadence acceptance test needs a deterministic error-path seam.
- The no-lock/no-map/no-hidden-traversal guarantees remain source-supported. Scan batch constants bound individual batches, not total scan complexity or total system growth.
- The claimed universal sub-second vector restart replay is not substantiated by source or permitted measurements (crates/shamir-tunables/src/lib.rs:145). Snapshot thresholds trigger asynchronous work and are not hard replay/orphan-size caps.
- Existing max(1) defeats the claimed automatic zero-permit stall. Tokio timer rounding and cooperative polling prevent concluding mandatory 100%-core starvation from sleep(0) alone.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tunables -- Performance & O(x->0)

## Summary

The crate is almost entirely compile-time `const`s plus a three-field
`RuntimeTunables` of `AtomicUsize`/`AtomicU64` accessors — there are no loops,
no heap allocations, no collections, and no locks anywhere under `src/`, so the
lock-free (pillar 1) and O(x->0) (pillar 3) pillars are satisfied trivially and
the `scc::len()` ban has no surface to apply to. The only theme-relevant
exposure is at the runtime-override API boundary: the setters store degenerate
values (zero poll interval, zero in-flight cap) with no clamping — a latent
busy-spin / stall trap for the future config cascade — and a workspace-wide
grep shows the entire accessor layer currently has zero production readers, so
the advertised runtime-tuning surface is presently inert. Both findings are low
severity; the crate itself is clean for this theme.

## Findings

### 1. Runtime setters accept degenerate values with no clamping — latent busy-spin / zero-permit stall once the cascade goes live

- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-64` (`set_server_poll_interval`, `set_io_frame_buffer_cap`), `runtime.rs:73-75` (`set_conn_max_in_flight`)
- **Severity:** low
- **Issue:** All three setters store the given value verbatim with no
  validation. `set_server_poll_interval` narrows through
  `v.as_millis() as u64` (`runtime.rs:62-63`), so any sub-millisecond
  `Duration` — including `Duration::ZERO` — truncates to `0`, and
  `server_poll_interval()` (`runtime.rs:51-53`) then hands consumers a 0 ms
  interval. `SERVER_POLL_INTERVAL`'s documented contract is "sleep between
  non-blocking checks" (`lib.rs:37-39`); a 0 ms value turns every consumer
  housekeeping loop of the shape `tokio::time::sleep(interval).await`
  (today `server_launcher.rs:1008/1105/1210` on the const) into a
  yield-only busy spin — unbounded CPU burn triggered by one "tuning" call.
  Likewise `set_conn_max_in_flight(0)` stores 0, which as the per-connection
  semaphore/channel bound (`lib.rs:41-48`) would stall every pipelined
  request on new connections; `set_io_frame_buffer_cap(usize::MAX)` would
  panic at the consumer's `Vec::with_capacity` call-site
  (`shamir-server/src/connection/handshake.rs:705`).
- **Failure scenario:** Once the documented "later phase" promotes these knobs
  into the live cascade (`lib.rs:4-7`), an operator-set
  `set_server_poll_interval(Duration::from_millis(0))` (or any erroneous
  sub-ms value) converts idle 50 ms poll loops into 100%-core spin loops, and
  `set_conn_max_in_flight(0)` deadlocks all new connections. Latent today —
  the accessors have no production callers (finding 2) — but the trap ships
  in the public API now.
- **Suggested fix:** Clamp or reject at the setter: floor
  `server_poll_interval` at 1 ms (`v.max(Duration::from_millis(1))`), floor
  `conn_max_in_flight` at 1, and either cap `io_frame_buffer_cap` sanely or
  return `Result` — mirroring the codebase's existing pattern of clamping
  knob values at their mutation site (cf. `post_auth_bucket`'s refill-watermark
  guard in `shamir-connect/src/server/session.rs`). Extend
  `src/tests/runtime_tests.rs` (currently only round-trips valid values,
  lines 27-47) to pin the clamped behavior for zero/degenerate inputs.

### 2. `RuntimeTunables` is dead-wired: constructed and carried, but zero production readers — advertised runtime tuning is a no-op

- **File:line:** `crates/shamir-tunables/src/runtime.rs:36-76` (API surface); evidence: `crates/shamir-server/src/server/server_launcher.rs:900`, `crates/shamir-server/src/server/server_handle.rs:93`
- **Severity:** low
- **Issue:** A workspace-wide grep for all three accessors
  (`io_frame_buffer_cap` / `server_poll_interval` / `conn_max_in_flight`)
  finds call-sites only inside this crate's own tests
  (`src/tests/runtime_tests.rs`). The sole production references to the
  `runtime` module are construction (`server_launcher.rs:900`:
  `tunables: Arc::new(RuntimeTunables::new())`) and the field declaration on
  `ServerHandle` (`server_handle.rs:93`) — the `Arc` is stored and never
  dereferenced for a read. Every production consumer still reads the
  compile-time consts directly: `server_launcher.rs:958, 959, 1008, 1105,
  1210` and `connection/handshake.rs:705, 707` (plus the other crates' const
  uses). Performance-lens impact: the crate's core contract — "overrides ...
  take effect on the next read" (`runtime.rs:55, 60, 72`, doc header lines
  1-6) — cannot be exercised, so per-op hot-path costs that these knobs are
  meant to govern (the two per-connection frame-buffer allocations, the poll
  loop cadence) are not tunable at runtime, and any operator/bench run that
  "retunes" via the setters measures noise: the docs promise an effect the
  code cannot deliver, defeating the crate's stated purpose (avoid
  rebuild + re-bench cycles per knob change).
- **Failure scenario:** Operator calls
  `tunables.set_io_frame_buffer_cap(65536)` expecting smaller/larger
  per-connection frame buffers; every new connection still allocates the
  baked-in 4096 (`handshake.rs:705,707`). The observed throughput/latency
  delta is then misattributed to the knob.
- **Suggested fix:** Either wire the reads in — mechanically replace
  `shamir_tunables::instance_defaults::X` with `tunables.x()` at the
  `shamir-server` call-sites where the handle is in scope (each is an
  `#[inline]` relaxed atomic load, so the hot-path cost is unchanged) — or,
  if the plumbing is deliberately staged for the later cascade phase, mark
  the type/methods with a doc comment stating "not yet read by any
  production path; wiring tracked in <task>" so nobody benchmarks against a
  knob that cannot change behavior.

---

Checked and clean for this theme: no allocation-in-loop, no hidden O(N)/O(N²)
helpers, no `Mutex`/`RwLock`/`parking_lot`, no scc/dashmap (so the
`scc::len()` ban is vacuously satisfied), zero dependencies in `Cargo.toml`,
and the `const` set itself (`FULL_SCAN_BATCH`, `MAINT_SCAN_BATCH`,
`MAX_UNDRAINED_VERSIONS`, `JOURNAL_BACKFILL_LIMIT`,
`MAX_SUBSCRIPTIONS_PER_CONNECTION`, `WAL_SEGMENT_MAX_BYTES`, ...) is precisely
the anti-unbounded-growth bounding the theme asks about. Test coverage
(`src/tests/runtime_tests.rs`, 5 tests) pins defaults-vs-consts and set/read
round-trips for all three runtime knobs plus `Arc`-shareability; no test
covers degenerate/clamped inputs (gap noted in finding 1).

</details>
