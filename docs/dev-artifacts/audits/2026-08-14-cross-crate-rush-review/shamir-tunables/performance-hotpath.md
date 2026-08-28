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
